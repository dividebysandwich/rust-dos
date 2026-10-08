//! OpenGL through EGL (XR_MNDX_egl_enable), which the openxr crate has no
//! binding for: the context current on the headset's thread, made by SDL
//! under Wayland (or under X11 with SDL_VIDEO_X11_FORCE_EGL), as Monado
//! takes it. The swapchains' images are OpenGL textures, as with GLX.

use openxr as xr;
use openxr::sys;
use openxr::sys::Handle as _;
use std::ffi::{c_int, c_void};
use std::ptr;

/// The EGL binding, for `xr::Session<Egl>`.
pub enum Egl {}

/// The current context's EGL handles.
pub struct Handles {
    get_proc_address: sys::platform::EglGetProcAddressMNDX,
    display: *mut c_void,
    config: *mut c_void,
    context: *mut c_void,
}

const EGL_CONFIG_ID: c_int = 0x3028;
const EGL_NONE: c_int = 0x3038;

/// The EGL handles of the context current on this thread, and libEGL,
/// to be kept loaded while they are used.
pub fn current() -> Result<(Handles, libloading::Library), String> {
    // SAFETY: libEGL is the library SDL draws with, already loaded; the
    // functions are EGL's, with EGL's signatures.
    unsafe {
        let library = libloading::Library::new("libEGL.so.1").map_err(|e| format!("EGL can't be loaded: {}", e))?;
        let display = *library.get::<unsafe extern "C" fn() -> *mut c_void>(b"eglGetCurrentDisplay\0").map_err(|e| e.to_string())?;
        let context = *library.get::<unsafe extern "C" fn() -> *mut c_void>(b"eglGetCurrentContext\0").map_err(|e| e.to_string())?;
        let query = *library
            .get::<unsafe extern "C" fn(*mut c_void, *mut c_void, c_int, *mut c_int) -> u32>(b"eglQueryContext\0")
            .map_err(|e| e.to_string())?;
        let choose = *library
            .get::<unsafe extern "C" fn(*mut c_void, *const c_int, *mut *mut c_void, c_int, *mut c_int) -> u32>(
                b"eglChooseConfig\0",
            )
            .map_err(|e| e.to_string())?;
        let get_proc_address = *library
            .get::<sys::platform::EglGetProcAddressMNDX>(b"eglGetProcAddress\0")
            .map_err(|e| e.to_string())?;
        let (display, context) = (display(), context());
        if display.is_null() || context.is_null() {
            return Err("OpenGL doesn't draw through EGL".into());
        }
        let mut id = 0;
        if query(display, context, EGL_CONFIG_ID, &mut id) == 0 {
            return Err("EGL doesn't say the context's configuration".into());
        }
        let attributes = [EGL_CONFIG_ID, id, EGL_NONE];
        let (mut config, mut count) = (ptr::null_mut(), 0);
        if choose(display, attributes.as_ptr(), &mut config, 1, &mut count) == 0 || count < 1 {
            return Err("EGL can't find the context's configuration".into());
        }
        Ok((Handles { get_proc_address, display, config, context }, library))
    }
}

impl xr::Graphics for Egl {
    type Requirements = xr::opengl::Requirements;
    type SessionCreateInfo = Handles;
    type Format = u32;
    type SwapchainImage = u32;

    fn raise_format(x: i64) -> u32 {
        x as _
    }

    fn lower_format(x: u32) -> i64 {
        x.into()
    }

    /// OpenGL's, which XR_MNDX_egl_enable asks for too.
    fn requirements(instance: &xr::Instance, system: xr::SystemId) -> xr::Result<Self::Requirements> {
        <xr::OpenGL as xr::Graphics>::requirements(instance, system)
    }

    unsafe fn create_session(
        instance: &xr::Instance,
        system: xr::SystemId,
        info: &Handles,
    ) -> xr::Result<sys::Session> {
        let binding = sys::GraphicsBindingEGLMNDX {
            ty: sys::GraphicsBindingEGLMNDX::TYPE,
            next: ptr::null(),
            get_proc_address: Some(info.get_proc_address),
            display: info.display,
            config: info.config,
            context: info.context,
        };
        let info = sys::SessionCreateInfo {
            ty: sys::SessionCreateInfo::TYPE,
            next: &binding as *const _ as *const _,
            create_flags: Default::default(),
            system_id: system,
        };
        let mut out = sys::Session::NULL;
        // SAFETY: the binding is the context current on this thread
        // (`current`).
        let result = unsafe { (instance.fp().create_session)(instance.as_raw(), &info, &mut out) };
        if result.into_raw() < 0 { Err(result) } else { Ok(out) }
    }

    fn enumerate_swapchain_images(swapchain: &xr::Swapchain<Self>) -> xr::Result<Vec<u32>> {
        let enumerate = |capacity: u32, count: &mut u32, images: *mut sys::SwapchainImageOpenGLKHR| {
            // SAFETY: `images` has room for `capacity` of them.
            unsafe {
                (swapchain.instance().fp().enumerate_swapchain_images)(swapchain.as_raw(), capacity, count, images.cast())
            }
        };
        let empty = sys::SwapchainImageOpenGLKHR { ty: sys::SwapchainImageOpenGLKHR::TYPE, next: ptr::null_mut(), image: 0 };
        let mut count = 0;
        let result = enumerate(0, &mut count, ptr::null_mut());
        if result.into_raw() < 0 {
            return Err(result);
        }
        let mut images = vec![empty; count as usize];
        let result = enumerate(count, &mut count, images.as_mut_ptr());
        if result.into_raw() < 0 {
            return Err(result);
        }
        images.truncate(count as usize);
        Ok(images.into_iter().map(|i| i.image).collect())
    }
}

/// Whether an EGL context is current on this thread (rather than GLX's).
pub fn is_current() -> bool {
    // SAFETY: as in `current`.
    unsafe {
        let Ok(library) = libloading::Library::new("libEGL.so.1") else { return false };
        let Ok(context) = library.get::<unsafe extern "C" fn() -> *mut c_void>(b"eglGetCurrentContext\0") else {
            return false;
        };
        !context().is_null()
    }
}
