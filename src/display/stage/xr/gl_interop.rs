//! OpenGL's side of memory shared with Vulkan (GL_EXT_memory_object,
//! GL_EXT_memory_object_fd), which glow has none of: a texture whose
//! storage is memory Vulkan allocated, and the GPU and driver OpenGL draws
//! with, which must be Vulkan's for that.

use std::ffi::{CString, c_int, c_void};

const GL_TEXTURE_TILING_EXT: u32 = 0x9580;
const GL_DEDICATED_MEMORY_OBJECT_EXT: u32 = 0x9581;
const GL_OPTIMAL_TILING_EXT: i32 = 0x9584;
const GL_HANDLE_TYPE_OPAQUE_FD_EXT: u32 = 0x9586;
const GL_DEVICE_UUID_EXT: u32 = 0x9597;
const GL_DRIVER_UUID_EXT: u32 = 0x9598;

/// The extensions' functions, as the current context has them.
pub struct Interop {
    create_memory_objects: unsafe extern "system" fn(i32, *mut u32),
    delete_memory_objects: unsafe extern "system" fn(i32, *const u32),
    memory_object_parameter: unsafe extern "system" fn(u32, u32, *const i32),
    import_memory_fd: unsafe extern "system" fn(u32, u64, u32, c_int),
    tex_storage_mem_2d: unsafe extern "system" fn(u32, i32, u32, i32, i32, u32, u64),
    get_unsigned_byte: unsafe extern "system" fn(u32, *mut u8),
    get_unsigned_byte_i: unsafe extern "system" fn(u32, u32, *mut u8),
}

impl Interop {
    /// The functions, if the context current on this thread has the
    /// extensions (`extensions`, as glow lists them).
    #[allow(clippy::missing_transmute_annotations)]
    pub fn load(extensions: &std::collections::HashSet<String>) -> Result<Self, String> {
        for wanted in ["GL_EXT_memory_object", "GL_EXT_memory_object_fd"] {
            if !extensions.contains(wanted) {
                return Err(format!("OpenGL has no {}", wanted));
            }
        }
        let get = |name: &str| -> Result<*const c_void, String> {
            let c = CString::new(name).unwrap_or_default();
            // SAFETY: a context is current on this thread.
            let address = unsafe { sdl2::sys::SDL_GL_GetProcAddress(c.as_ptr()) };
            if address.is_null() { Err(format!("OpenGL has no {}", name)) } else { Ok(address as *const c_void) }
        };
        // SAFETY: the functions are the extensions', with their signatures.
        unsafe {
            Ok(Interop {
                create_memory_objects: std::mem::transmute(get("glCreateMemoryObjectsEXT")?),
                delete_memory_objects: std::mem::transmute(get("glDeleteMemoryObjectsEXT")?),
                memory_object_parameter: std::mem::transmute(get("glMemoryObjectParameterivEXT")?),
                import_memory_fd: std::mem::transmute(get("glImportMemoryFdEXT")?),
                tex_storage_mem_2d: std::mem::transmute(get("glTexStorageMem2DEXT")?),
                get_unsigned_byte: std::mem::transmute(get("glGetUnsignedBytevEXT")?),
                get_unsigned_byte_i: std::mem::transmute(get("glGetUnsignedBytei_vEXT")?),
            })
        }
    }

    /// The GPU's and the driver's UUIDs, to compare with Vulkan's.
    pub fn uuids(&self) -> ([u8; 16], [u8; 16]) {
        let (mut device, mut driver) = ([0; 16], [0; 16]);
        // SAFETY: each is 16 bytes, GL_UUID_SIZE_EXT.
        unsafe {
            (self.get_unsigned_byte_i)(GL_DEVICE_UUID_EXT, 0, device.as_mut_ptr());
            (self.get_unsigned_byte)(GL_DRIVER_UUID_EXT, driver.as_mut_ptr());
        }
        (device, driver)
    }

    /// A memory object of `size` bytes, dedicated to one image, of the
    /// memory `fd` is, which OpenGL takes and closes.
    pub fn import(&self, size: u64, fd: c_int) -> u32 {
        let mut memory = 0;
        // SAFETY: a context is current; `fd` is the memory's.
        unsafe {
            (self.create_memory_objects)(1, &mut memory);
            let dedicated = 1;
            (self.memory_object_parameter)(memory, GL_DEDICATED_MEMORY_OBJECT_EXT, &dedicated);
            (self.import_memory_fd)(memory, size, GL_HANDLE_TYPE_OPAQUE_FD_EXT, fd);
        }
        memory
    }

    /// Give the texture bound to TEXTURE_2D `memory` as its storage: one
    /// level of `format`, `size` big, tiled as Vulkan's optimal images are.
    pub fn tex_storage(&self, gl: &glow::Context, format: u32, size: (u32, u32), memory: u32) {
        use glow::HasContext;
        // SAFETY: a context is current with the texture bound.
        unsafe {
            gl.tex_parameter_i32(glow::TEXTURE_2D, GL_TEXTURE_TILING_EXT, GL_OPTIMAL_TILING_EXT);
            (self.tex_storage_mem_2d)(glow::TEXTURE_2D, 1, format, size.0 as i32, size.1 as i32, memory, 0);
        }
    }

    pub fn delete(&self, memory: u32) {
        // SAFETY: a context is current.
        unsafe { (self.delete_memory_objects)(1, &memory) };
    }
}
