//! The Vulkan bridge, for an OpenXR runtime that takes Vulkan only
//! (XR_KHR_vulkan_enable2): each eye is drawn with OpenGL, as with any
//! other binding, into an image of Vulkan's whose memory OpenGL imports
//! (GL_EXT_memory_object_fd), and Vulkan copies it into the image of the
//! runtime's swapchain, turned the right way up (OpenGL's rows go up,
//! Vulkan's down).
//!
//! The runtime allocates its swapchains' images itself, and their memory
//! can't be shared, hence the copy. OpenGL and Vulkan must draw on the same
//! GPU with the same driver (their UUIDs say so). OpenGL finishes an eye
//! before Vulkan copies it, and Vulkan finishes the copy before the image
//! is released.

use super::err;
use super::gl_frames::{Acquired, Failure};
use super::gl_interop::Interop;
use ash::vk;
use ash::vk::Handle as _;
use glow::HasContext;
use openxr as xr;

/// Vulkan's instance and device, made through the runtime: destroyed after
/// the session.
pub struct Device {
    device: ash::Device,
    instance: ash::Instance,
    _entry: ash::Entry,
}

impl Drop for Device {
    fn drop(&mut self) {
        // SAFETY: nothing of the device's is left (`Bridge` drops first,
        // and the session before this).
        unsafe {
            let _ = self.device.device_wait_idle();
            self.device.destroy_device(None);
            self.instance.destroy_instance(None);
        }
    }
}

struct Eye {
    swapchain: xr::Swapchain<xr::Vulkan>,
    /// The copy into each of the swapchain's images.
    copies: Vec<vk::CommandBuffer>,
    acquired: Option<usize>,
    size: (u32, u32),
    /// The image OpenGL draws into, and its memory.
    image: vk::Image,
    memory: vk::DeviceMemory,
    /// OpenGL's memory object of it, its texture and framebuffer.
    gl_memory: u32,
    texture: Option<glow::Texture>,
    framebuffer: Option<glow::Framebuffer>,
}

pub struct Bridge {
    eyes: Vec<Eye>,
    stream: xr::FrameStream<xr::Vulkan>,
    device: ash::Device,
    queue: vk::Queue,
    family: u32,
    pool: vk::CommandPool,
    fence: vk::Fence,
    srgb: bool,
    interop: Interop,
    /// The formats the session offered.
    formats: Vec<u32>,
}

/// The swapchains' format out of the session's `formats`: whether it is
/// sRGB, and the format of the images OpenGL draws into (Vulkan's and
/// OpenGL's), which the copy converts from.
pub fn pick_format(formats: &[u32]) -> Option<(vk::Format, bool, vk::Format, u32)> {
    let srgb = (vk::Format::R8G8B8A8_SRGB, glow::SRGB8_ALPHA8);
    let linear = (vk::Format::R8G8B8A8_UNORM, glow::RGBA8);
    [
        (vk::Format::R8G8B8A8_SRGB, true, srgb),
        (vk::Format::B8G8R8A8_SRGB, true, srgb),
        (vk::Format::R8G8B8A8_UNORM, false, linear),
        (vk::Format::B8G8R8A8_UNORM, false, linear),
    ]
    .into_iter()
    .find(|(format, ..)| formats.contains(&(format.as_raw() as u32)))
    .map(|(format, is_srgb, (shared, gl_format))| (format, is_srgb, shared, gl_format))
}

fn vk_err(what: &str) -> impl Fn(vk::Result) -> String + '_ {
    move |e| format!("{}: {}", what, e)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{:02x}", b)).collect()
}

/// A session drawing through Vulkan, made through the runtime, the eyes'
/// images `sizes` big; with OpenGL's context current on this thread.
pub fn open(
    gl: &glow::Context,
    instance: &xr::Instance,
    system: xr::SystemId,
    sizes: &[(u32, u32)],
) -> Result<(xr::Session<xr::AnyGraphics>, xr::FrameWaiter, Bridge, Device), String> {
    let interop = Interop::load(gl.supported_extensions())?;
    let requirements = instance.graphics_requirements::<xr::Vulkan>(system).map_err(err("OpenXR"))?;
    let least = requirements.min_api_version_supported;
    let api = vk::make_api_version(0, 1, 1, 0).max(vk::make_api_version(0, least.major() as u32, least.minor() as u32, 0));
    // SAFETY: Vulkan's loader, a conforming one.
    let entry = unsafe { ash::Entry::load() }.map_err(|e| format!("Vulkan can't be loaded: {}", e))?;
    let gipa = entry.static_fn().get_instance_proc_addr;
    // SAFETY: the function pointers are the loader's, of the same
    // signatures as OpenXR's declarations of them.
    let get_instance_proc_addr = unsafe { std::mem::transmute::<vk::PFN_vkGetInstanceProcAddr, xr::sys::platform::VkGetInstanceProcAddr>(gipa) };
    let app = vk::ApplicationInfo::default().application_name(c"Rust-DOS").api_version(api);
    let instance_info = vk::InstanceCreateInfo::default().application_info(&app);
    // SAFETY: the create info is valid for the call.
    let vk_instance = unsafe {
        instance.create_vulkan_instance(system, get_instance_proc_addr, &instance_info as *const _ as *const _)
    }
    .map_err(err("Vulkan's instance"))?
    .map_err(|e| format!("Vulkan's instance: {}", vk::Result::from_raw(e)))?;
    let vk_instance = vk::Instance::from_raw(vk_instance as u64);
    // SAFETY: the instance was just made with this loader.
    let ash_instance = unsafe { ash::Instance::load(entry.static_fn(), vk_instance) };
    // From here the instance is destroyed if anything fails: `Device` with
    // a null device does that.
    let destroy_instance = |message: String| {
        // SAFETY: nothing was made of it.
        unsafe { ash_instance.destroy_instance(None) };
        message
    };
    // SAFETY: the instance is the runtime's.
    let physical = match unsafe { instance.vulkan_graphics_device(system, vk_instance.as_raw() as _) } {
        Ok(physical) => vk::PhysicalDevice::from_raw(physical as u64),
        Err(e) => return Err(destroy_instance(format!("Vulkan's GPU: {}", e))),
    };
    // OpenGL draws into Vulkan's memory: on the same GPU, with the same
    // driver, or the memory means something else to each.
    let mut ids = vk::PhysicalDeviceIDProperties::default();
    let mut properties = vk::PhysicalDeviceProperties2::default().push_next(&mut ids);
    // SAFETY: 1.1's, core.
    unsafe { ash_instance.get_physical_device_properties2(physical, &mut properties) };
    let name = properties.properties.device_name_as_c_str().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let (device_uuid, driver_uuid) = (ids.device_uuid, ids.driver_uuid);
    let (gl_device, gl_driver) = interop.uuids();
    super::log::line(format!(
        "Vulkan's GPU: {} (device {}, driver {}); OpenGL's: device {}, driver {}",
        name,
        hex(&device_uuid),
        hex(&driver_uuid),
        hex(&gl_device),
        hex(&gl_driver)
    ));
    if device_uuid[..] != gl_device[..] || driver_uuid[..] != gl_driver[..] {
        return Err(destroy_instance(format!("OpenGL and Vulkan don't draw with the same GPU and driver ({})", name)));
    }
    // SAFETY: as above.
    let families = unsafe { ash_instance.get_physical_device_queue_family_properties(physical) };
    let Some(family) = families.iter().position(|f| f.queue_flags.contains(vk::QueueFlags::GRAPHICS)) else {
        return Err(destroy_instance("Vulkan has no graphics queue".into()));
    };
    let family = family as u32;
    let priorities = [1.0];
    let queues = [vk::DeviceQueueCreateInfo::default().queue_family_index(family).queue_priorities(&priorities)];
    let extensions = [ash::khr::external_memory_fd::NAME.as_ptr()];
    let device_info = vk::DeviceCreateInfo::default().queue_create_infos(&queues).enabled_extension_names(&extensions);
    // SAFETY: as for the instance.
    let made = unsafe {
        instance.create_vulkan_device(system, get_instance_proc_addr, physical.as_raw() as _, &device_info as *const _ as *const _)
    };
    let vk_device = match made {
        Ok(Ok(device)) => vk::Device::from_raw(device as u64),
        Ok(Err(e)) => return Err(destroy_instance(format!("Vulkan's device: {}", vk::Result::from_raw(e)))),
        Err(e) => return Err(destroy_instance(format!("Vulkan's device: {}", e))),
    };
    // SAFETY: the device was just made with this instance.
    let ash_device = unsafe { ash::Device::load(ash_instance.fp_v1_0(), vk_device) };
    let device = Device { device: ash_device.clone(), instance: ash_instance.clone(), _entry: entry };
    // SAFETY: OpenXR's binding of the device just made.
    let (session, waiter, stream) = unsafe {
        instance.create_session::<xr::Vulkan>(
            system,
            &xr::vulkan::SessionCreateInfo {
                instance: vk_instance.as_raw() as _,
                physical_device: physical.as_raw() as _,
                device: vk_device.as_raw() as _,
                queue_family_index: family,
                queue_index: 0,
            },
        )
    }
    .map_err(err("the headset's session"))?;
    let formats = session.enumerate_swapchain_formats().map_err(err("the headset's formats"))?;
    let (format, srgb, shared, gl_format) =
        pick_format(&formats).ok_or_else(|| format!("the headset takes none of Vulkan's RGBA8 formats ({:?})", formats))?;
    for (wanted, which, tiling) in [(vk::FormatFeatureFlags::BLIT_SRC, shared, "drawn"), (vk::FormatFeatureFlags::BLIT_DST, format, "shown")] {
        // SAFETY: as above.
        let features = unsafe { ash_instance.get_physical_device_format_properties(physical, which) };
        if !features.optimal_tiling_features.contains(wanted) {
            return Err(format!("Vulkan can't copy the {} images (format {})", tiling, which.as_raw()));
        }
    }
    let memory_types = unsafe { ash_instance.get_physical_device_memory_properties(physical) };
    // SAFETY: as above.
    let queue = unsafe { ash_device.get_device_queue(family, 0) };
    let memory_fd = ash::khr::external_memory_fd::Device::new(&ash_instance, &ash_device);
    let mut bridge = Bridge {
        eyes: Vec::new(),
        stream,
        device: ash_device.clone(),
        queue,
        family,
        pool: vk::CommandPool::null(),
        fence: vk::Fence::null(),
        srgb,
        interop,
        formats,
    };
    // SAFETY: the device's own objects, made and destroyed with it
    // (`Bridge`'s drop).
    unsafe {
        let pool_info = vk::CommandPoolCreateInfo::default().queue_family_index(family);
        bridge.pool = ash_device.create_command_pool(&pool_info, None).map_err(vk_err("Vulkan's commands"))?;
        bridge.fence = ash_device.create_fence(&vk::FenceCreateInfo::default(), None).map_err(vk_err("Vulkan's fence"))?;
    }
    for &size in sizes {
        let swapchain = session
            .create_swapchain(&xr::SwapchainCreateInfo {
                create_flags: xr::SwapchainCreateFlags::EMPTY,
                usage_flags: xr::SwapchainUsageFlags::COLOR_ATTACHMENT | xr::SwapchainUsageFlags::TRANSFER_DST,
                format: format.as_raw() as u32,
                sample_count: 1,
                width: size.0,
                height: size.1,
                face_count: 1,
                array_size: 1,
                mip_count: 1,
            })
            .map_err(err("the headset's swapchain"))?;
        let images = swapchain.enumerate_images().map_err(err("the headset's swapchain"))?;
        let mut eye = Eye {
            swapchain,
            copies: Vec::new(),
            acquired: None,
            size,
            image: vk::Image::null(),
            memory: vk::DeviceMemory::null(),
            gl_memory: 0,
            texture: None,
            framebuffer: None,
        };
        let result = bridge.share(gl, &ash_device, &memory_fd, &memory_types, &mut eye, shared, gl_format);
        let result = result.and_then(|()| {
            let images: Vec<vk::Image> = images.into_iter().map(vk::Image::from_raw).collect();
            bridge.record(&ash_device, &mut eye, &images)
        });
        // (Deleted with the others if this one failed.)
        bridge.eyes.push(eye);
        if let Err(e) = result {
            bridge.delete(gl);
            return Err(e);
        }
    }
    // The images OpenGL draws into start out in GENERAL, which they stay in.
    if let Err(e) = bridge.prepare() {
        bridge.delete(gl);
        return Err(e);
    }
    Ok((session.into_any_graphics(), waiter, bridge, device))
}

impl Bridge {
    /// Make the eye's image OpenGL draws into, and OpenGL's texture and
    /// framebuffer of its memory.
    #[allow(clippy::too_many_arguments)]
    fn share(
        &self,
        gl: &glow::Context,
        device: &ash::Device,
        memory_fd: &ash::khr::external_memory_fd::Device,
        types: &vk::PhysicalDeviceMemoryProperties,
        eye: &mut Eye,
        format: vk::Format,
        gl_format: u32,
    ) -> Result<(), String> {
        let (width, height) = eye.size;
        let mut external = vk::ExternalMemoryImageCreateInfo::default().handle_types(vk::ExternalMemoryHandleTypeFlags::OPAQUE_FD);
        let image_info = vk::ImageCreateInfo::default()
            .image_type(vk::ImageType::TYPE_2D)
            .format(format)
            .extent(vk::Extent3D { width, height, depth: 1 })
            .mip_levels(1)
            .array_layers(1)
            .samples(vk::SampleCountFlags::TYPE_1)
            .tiling(vk::ImageTiling::OPTIMAL)
            .usage(vk::ImageUsageFlags::COLOR_ATTACHMENT | vk::ImageUsageFlags::TRANSFER_SRC | vk::ImageUsageFlags::SAMPLED)
            .initial_layout(vk::ImageLayout::UNDEFINED)
            .push_next(&mut external);
        // SAFETY: the device's own objects; the fd goes to OpenGL, which
        // closes it.
        unsafe {
            eye.image = device.create_image(&image_info, None).map_err(vk_err("Vulkan's image"))?;
            let needs = device.get_image_memory_requirements(eye.image);
            let kind = (0..types.memory_type_count)
                .find(|&i| {
                    needs.memory_type_bits & (1 << i) != 0
                        && types.memory_types[i as usize].property_flags.contains(vk::MemoryPropertyFlags::DEVICE_LOCAL)
                })
                .ok_or("Vulkan has no memory for the image")?;
            let mut export = vk::ExportMemoryAllocateInfo::default().handle_types(vk::ExternalMemoryHandleTypeFlags::OPAQUE_FD);
            let mut dedicated = vk::MemoryDedicatedAllocateInfo::default().image(eye.image);
            let allocate = vk::MemoryAllocateInfo::default()
                .allocation_size(needs.size)
                .memory_type_index(kind)
                .push_next(&mut export)
                .push_next(&mut dedicated);
            eye.memory = device.allocate_memory(&allocate, None).map_err(vk_err("Vulkan's memory"))?;
            device.bind_image_memory(eye.image, eye.memory, 0).map_err(vk_err("Vulkan's memory"))?;
            let fd_info = vk::MemoryGetFdInfoKHR::default()
                .memory(eye.memory)
                .handle_type(vk::ExternalMemoryHandleTypeFlags::OPAQUE_FD);
            let fd = memory_fd.get_memory_fd(&fd_info).map_err(vk_err("sharing Vulkan's memory"))?;
            eye.gl_memory = self.interop.import(needs.size, fd);
            let texture = gl.create_texture()?;
            eye.texture = Some(texture);
            gl.bind_texture(glow::TEXTURE_2D, Some(texture));
            self.interop.tex_storage(gl, gl_format, eye.size, eye.gl_memory);
            gl.bind_texture(glow::TEXTURE_2D, None);
            let framebuffer = gl.create_framebuffer()?;
            eye.framebuffer = Some(framebuffer);
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(framebuffer));
            gl.framebuffer_texture_2d(glow::FRAMEBUFFER, glow::COLOR_ATTACHMENT0, glow::TEXTURE_2D, Some(texture), 0);
            let complete = gl.check_framebuffer_status(glow::FRAMEBUFFER) == glow::FRAMEBUFFER_COMPLETE;
            gl.bind_framebuffer(glow::FRAMEBUFFER, None);
            if gl.get_error() != glow::NO_ERROR || !complete {
                return Err("OpenGL can't draw into Vulkan's image".into());
            }
        }
        Ok(())
    }

    /// Record the copy of the eye's image into each of the swapchain's
    /// `images`: from OpenGL to Vulkan, upside down into the runtime's
    /// image, which ends up as the runtime takes it, and back to OpenGL.
    fn record(&self, device: &ash::Device, eye: &mut Eye, images: &[vk::Image]) -> Result<(), String> {
        let (w, h) = (eye.size.0 as i32, eye.size.1 as i32);
        let color = vk::ImageSubresourceRange::default()
            .aspect_mask(vk::ImageAspectFlags::COLOR)
            .level_count(1)
            .layer_count(1);
        let layers = vk::ImageSubresourceLayers::default().aspect_mask(vk::ImageAspectFlags::COLOR).layer_count(1);
        let family = vk::QUEUE_FAMILY_IGNORED;
        let allocate = vk::CommandBufferAllocateInfo::default()
            .command_pool(self.pool)
            .level(vk::CommandBufferLevel::PRIMARY)
            .command_buffer_count(images.len() as u32);
        // SAFETY: the device's own objects, recorded once.
        unsafe {
            eye.copies = device.allocate_command_buffers(&allocate).map_err(vk_err("Vulkan's commands"))?;
            for (&commands, &target) in eye.copies.iter().zip(images) {
                device
                    .begin_command_buffer(commands, &vk::CommandBufferBeginInfo::default())
                    .map_err(vk_err("Vulkan's commands"))?;
                let from_gl = vk::ImageMemoryBarrier::default()
                    .src_access_mask(vk::AccessFlags::empty())
                    .dst_access_mask(vk::AccessFlags::TRANSFER_READ)
                    .old_layout(vk::ImageLayout::GENERAL)
                    .new_layout(vk::ImageLayout::GENERAL)
                    .src_queue_family_index(vk::QUEUE_FAMILY_EXTERNAL)
                    .dst_queue_family_index(self.family())
                    .image(eye.image)
                    .subresource_range(color);
                let to_copy = vk::ImageMemoryBarrier::default()
                    .src_access_mask(vk::AccessFlags::empty())
                    .dst_access_mask(vk::AccessFlags::TRANSFER_WRITE)
                    .old_layout(vk::ImageLayout::UNDEFINED)
                    .new_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
                    .src_queue_family_index(family)
                    .dst_queue_family_index(family)
                    .image(target)
                    .subresource_range(color);
                device.cmd_pipeline_barrier(
                    commands,
                    vk::PipelineStageFlags::TOP_OF_PIPE,
                    vk::PipelineStageFlags::TRANSFER,
                    vk::DependencyFlags::empty(),
                    &[],
                    &[],
                    &[from_gl, to_copy],
                );
                // OpenGL's bottom row first, to the top.
                let blit = vk::ImageBlit::default()
                    .src_subresource(layers)
                    .src_offsets([vk::Offset3D { x: 0, y: h, z: 0 }, vk::Offset3D { x: w, y: 0, z: 1 }])
                    .dst_subresource(layers)
                    .dst_offsets([vk::Offset3D { x: 0, y: 0, z: 0 }, vk::Offset3D { x: w, y: h, z: 1 }]);
                device.cmd_blit_image(
                    commands,
                    eye.image,
                    vk::ImageLayout::GENERAL,
                    target,
                    vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                    &[blit],
                    vk::Filter::NEAREST,
                );
                let to_runtime = vk::ImageMemoryBarrier::default()
                    .src_access_mask(vk::AccessFlags::TRANSFER_WRITE)
                    .dst_access_mask(vk::AccessFlags::empty())
                    .old_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
                    .new_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)
                    .src_queue_family_index(family)
                    .dst_queue_family_index(family)
                    .image(target)
                    .subresource_range(color);
                let to_gl = vk::ImageMemoryBarrier::default()
                    .src_access_mask(vk::AccessFlags::TRANSFER_READ)
                    .dst_access_mask(vk::AccessFlags::empty())
                    .old_layout(vk::ImageLayout::GENERAL)
                    .new_layout(vk::ImageLayout::GENERAL)
                    .src_queue_family_index(self.family())
                    .dst_queue_family_index(vk::QUEUE_FAMILY_EXTERNAL)
                    .image(eye.image)
                    .subresource_range(color);
                device.cmd_pipeline_barrier(
                    commands,
                    vk::PipelineStageFlags::TRANSFER,
                    vk::PipelineStageFlags::BOTTOM_OF_PIPE,
                    vk::DependencyFlags::empty(),
                    &[],
                    &[],
                    &[to_runtime, to_gl],
                );
                device.end_command_buffer(commands).map_err(vk_err("Vulkan's commands"))?;
            }
        }
        Ok(())
    }

    fn family(&self) -> u32 {
        self.family
    }

    /// Run `record`'s commands once and wait for them.
    fn once(&self, record: impl FnOnce(vk::CommandBuffer)) -> Result<(), String> {
        let allocate = vk::CommandBufferAllocateInfo::default()
            .command_pool(self.pool)
            .level(vk::CommandBufferLevel::PRIMARY)
            .command_buffer_count(1);
        // SAFETY: the device's own objects.
        unsafe {
            let commands = self.device.allocate_command_buffers(&allocate).map_err(vk_err("Vulkan's commands"))?[0];
            let begin = vk::CommandBufferBeginInfo::default().flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT);
            let result = self
                .device
                .begin_command_buffer(commands, &begin)
                .and_then(|()| {
                    record(commands);
                    self.device.end_command_buffer(commands)
                })
                .and_then(|()| self.submit(commands));
            self.device.free_command_buffers(self.pool, &[commands]);
            result.map_err(vk_err("Vulkan's commands"))
        }
    }

    /// Submit `commands` and wait for them.
    fn submit(&self, commands: vk::CommandBuffer) -> Result<(), vk::Result> {
        let buffers = [commands];
        let submit = vk::SubmitInfo::default().command_buffers(&buffers);
        // SAFETY: the device's own objects; the queue is used by this thread
        // only, between the runtime's calls.
        unsafe {
            self.device.queue_submit(self.queue, &[submit], self.fence)?;
            let waited = self.device.wait_for_fences(&[self.fence], true, u64::MAX);
            self.device.reset_fences(&[self.fence])?;
            waited
        }
    }

    /// The images OpenGL draws into, from nothing into GENERAL, OpenGL's.
    fn prepare(&self) -> Result<(), String> {
        let color = vk::ImageSubresourceRange::default()
            .aspect_mask(vk::ImageAspectFlags::COLOR)
            .level_count(1)
            .layer_count(1);
        let barriers: Vec<_> = self
            .eyes
            .iter()
            .map(|eye| {
                vk::ImageMemoryBarrier::default()
                    .old_layout(vk::ImageLayout::UNDEFINED)
                    .new_layout(vk::ImageLayout::GENERAL)
                    .src_queue_family_index(self.family)
                    .dst_queue_family_index(vk::QUEUE_FAMILY_EXTERNAL)
                    .image(eye.image)
                    .subresource_range(color)
            })
            .collect();
        self.once(|commands| {
            // SAFETY: recording.
            unsafe {
                self.device.cmd_pipeline_barrier(
                    commands,
                    vk::PipelineStageFlags::TOP_OF_PIPE,
                    vk::PipelineStageFlags::BOTTOM_OF_PIPE,
                    vk::DependencyFlags::empty(),
                    &[],
                    &[],
                    &barriers,
                )
            }
        })
    }

    pub fn eye_format(&self) -> ((u32, u32), bool) {
        (self.eyes.first().map_or((0, 0), |e| e.size), self.srgb)
    }

    pub fn formats(&self) -> &[u32] {
        &self.formats
    }

    pub fn eyes(&self) -> usize {
        self.eyes.len()
    }

    pub fn begin(&mut self) -> xr::Result<()> {
        self.stream.begin().map(|_| ())
    }

    /// Acquire an image of eye `index` and wait for it: the framebuffer of
    /// the image OpenGL draws into, and its size.
    pub fn acquire(&mut self, index: usize) -> Acquired {
        let eye = &mut self.eyes[index];
        let image = eye.swapchain.acquire_image().map_err(|e| ("acquiring the headset's image", e))?;
        if let Err(e) = eye.swapchain.wait_image(xr::Duration::INFINITE) {
            let _ = eye.swapchain.release_image();
            return Err(("waiting for the headset's image", e));
        }
        eye.acquired = Some(image as usize);
        match eye.framebuffer {
            Some(framebuffer) if (image as usize) < eye.copies.len() => Ok((framebuffer, eye.size)),
            _ => {
                let _ = eye.swapchain.release_image();
                eye.acquired = None;
                Err(("the headset's image", xr::sys::Result::ERROR_VALIDATION_FAILURE))
            }
        }
    }

    /// Eye `index` is drawn, with `gl`: OpenGL finishes, Vulkan copies it
    /// into the runtime's image, which is released.
    pub fn release(&mut self, gl: &glow::Context, index: usize) -> Result<(), Failure> {
        let Some(image) = self.eyes[index].acquired.take() else { return Ok(()) };
        // SAFETY: see `GlScreen`.
        unsafe { gl.finish() };
        let copied = self.submit(self.eyes[index].copies[image]);
        let released = self.eyes[index].swapchain.release_image().map_err(|e| ("releasing the headset's image", e));
        if let Err(e) = copied {
            super::log::line(format!("Copying the eye into the headset's image: {}", e));
            return Err(("copying into the headset's image", xr::sys::Result::ERROR_RUNTIME_FAILURE));
        }
        released
    }

    /// End the frame: the eyes drawn from `views`, or nothing.
    pub fn end(
        &mut self,
        time: xr::Time,
        blend: xr::EnvironmentBlendMode,
        space: &xr::Space,
        views: Option<&[xr::View]>,
    ) -> xr::Result<()> {
        let Some(views) = views else { return self.stream.end(time, blend, &[]) };
        let projection_views: Vec<_> = views
            .iter()
            .zip(&self.eyes)
            .map(|(view, eye)| {
                let rect = xr::Rect2Di {
                    offset: xr::Offset2Di { x: 0, y: 0 },
                    extent: xr::Extent2Di { width: eye.size.0 as i32, height: eye.size.1 as i32 },
                };
                xr::CompositionLayerProjectionView::new().pose(view.pose).fov(view.fov).sub_image(
                    xr::SwapchainSubImage::new().swapchain(&eye.swapchain).image_array_index(0).image_rect(rect),
                )
            })
            .collect();
        let layer = xr::CompositionLayerProjection::new().space(space).views(&projection_views);
        self.stream.end(time, blend, &[&layer])
    }

    /// Delete OpenGL's side of the images, with `gl` current: before the
    /// memory goes (`Drop`).
    pub fn delete(&mut self, gl: &glow::Context) {
        // SAFETY: see `GlScreen`.
        unsafe {
            gl.finish();
            for eye in &mut self.eyes {
                if let Some(framebuffer) = eye.framebuffer.take() {
                    gl.delete_framebuffer(framebuffer);
                }
                if let Some(texture) = eye.texture.take() {
                    gl.delete_texture(texture);
                }
                if eye.gl_memory != 0 {
                    self.interop.delete(std::mem::take(&mut eye.gl_memory));
                }
            }
        }
    }
}

/// Vulkan's side, once OpenGL's is gone (`delete`), before the swapchains,
/// the session and the device.
impl Drop for Bridge {
    fn drop(&mut self) {
        // SAFETY: the device's own objects, no longer used.
        unsafe {
            let _ = self.device.device_wait_idle();
            for eye in &self.eyes {
                self.device.destroy_image(eye.image, None);
                self.device.free_memory(eye.memory, None);
            }
            self.device.destroy_command_pool(self.pool, None);
            self.device.destroy_fence(self.fence, None);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn srgb_rgba_is_taken_first() {
        let raw = |f: vk::Format| f.as_raw() as u32;
        let pick = |formats: &[u32]| pick_format(formats).map(|(f, srgb, shared, gl)| (raw(f), srgb, raw(shared), gl));
        let all = [raw(vk::Format::B8G8R8A8_UNORM), raw(vk::Format::B8G8R8A8_SRGB), raw(vk::Format::R8G8B8A8_SRGB)];
        let (rgba_srgb, bgra_srgb, bgra) = (raw(vk::Format::R8G8B8A8_SRGB), raw(vk::Format::B8G8R8A8_SRGB), all[0]);
        assert_eq!(pick(&all), Some((rgba_srgb, true, rgba_srgb, glow::SRGB8_ALPHA8)));
        assert_eq!(pick(&all[..2]), Some((bgra_srgb, true, rgba_srgb, glow::SRGB8_ALPHA8)));
        assert_eq!(pick(&all[..1]), Some((bgra, false, raw(vk::Format::R8G8B8A8_UNORM), glow::RGBA8)));
        assert_eq!(pick(&[raw(vk::Format::R16G16B16A16_SFLOAT)]), None);
    }
}
