//! The native handles of the window's OpenGL context, which OpenXR draws
//! with too: GLX's on Linux, WGL's on Windows.

pub use imp::binding;
use rust_dos::vr::ContextKind;

/// The kind of the OpenGL context current on this thread.
pub fn current_kind() -> ContextKind {
    #[cfg(windows)]
    return ContextKind::Wgl;
    #[cfg(target_os = "linux")]
    {
        // SAFETY: as in `binding`.
        let glx = unsafe {
            libloading::Library::new("libGL.so.1").ok().is_some_and(|library| {
                library
                    .get::<unsafe extern "C" fn() -> *mut std::ffi::c_void>(b"glXGetCurrentContext\0")
                    .is_ok_and(|context| !context().is_null())
            })
        };
        if glx || !super::egl::is_current() { ContextKind::Glx } else { ContextKind::Egl }
    }
}

#[cfg(target_os = "linux")]
mod imp {
    use openxr as xr;
    use std::ffi::{c_int, c_ulong, c_void};

    const GLX_SCREEN: c_int = 0x800C;
    const GLX_VISUAL_ID: c_int = 0x800B;
    const GLX_FBCONFIG_ID: c_int = 0x8013;

    pub fn binding() -> Result<(xr::opengl::SessionCreateInfo, libloading::Library), String> {
        // SAFETY: libGL is the library SDL draws with, already loaded; the
        // functions are GLX's, with GLX's signatures.
        unsafe {
            let library = libloading::Library::new("libGL.so.1")
                .or_else(|_| libloading::Library::new("libGLX.so.0"))
                .map_err(|e| format!("GLX can't be loaded: {}", e))?;
            let display = *library.get::<unsafe extern "C" fn() -> *mut c_void>(b"glXGetCurrentDisplay\0").map_err(|e| e.to_string())?;
            let drawable = *library.get::<unsafe extern "C" fn() -> c_ulong>(b"glXGetCurrentDrawable\0").map_err(|e| e.to_string())?;
            let context = *library.get::<unsafe extern "C" fn() -> *mut c_void>(b"glXGetCurrentContext\0").map_err(|e| e.to_string())?;
            let query = *library
                .get::<unsafe extern "C" fn(*mut c_void, *mut c_void, c_int, *mut c_int) -> c_int>(b"glXQueryContext\0")
                .map_err(|e| e.to_string())?;
            let configs = *library
                .get::<unsafe extern "C" fn(*mut c_void, c_int, *mut c_int) -> *mut *mut c_void>(b"glXGetFBConfigs\0")
                .map_err(|e| e.to_string())?;
            let attrib = *library
                .get::<unsafe extern "C" fn(*mut c_void, *mut c_void, c_int, *mut c_int) -> c_int>(b"glXGetFBConfigAttrib\0")
                .map_err(|e| e.to_string())?;
            let (x_display, glx_context) = (display(), context());
            if x_display.is_null() || glx_context.is_null() {
                return Err("OpenGL doesn't draw through GLX: start rust-dos under X11 (or XWayland)".into());
            }
            let (mut id, mut screen) = (0, 0);
            query(x_display, glx_context, GLX_FBCONFIG_ID, &mut id);
            query(x_display, glx_context, GLX_SCREEN, &mut screen);
            let mut count = 0;
            let list = configs(x_display, screen, &mut count);
            let mut glx_fb_config = std::ptr::null_mut();
            let mut visualid = 0;
            for i in 0..count.max(0) as usize {
                let config = *list.add(i);
                let mut value = 0;
                if attrib(x_display, config, GLX_FBCONFIG_ID, &mut value) == 0 && value == id {
                    glx_fb_config = config;
                    attrib(x_display, config, GLX_VISUAL_ID, &mut value);
                    visualid = value as u32;
                    break;
                }
            }
            // (The list is left to the end of the program: XFree is
            // Xlib's, which this doesn't load.)
            let info = xr::opengl::SessionCreateInfo::Xlib {
                x_display: x_display.cast(),
                visualid,
                glx_fb_config,
                glx_drawable: drawable(),
                glx_context,
            };
            Ok((info, library))
        }
    }
}

#[cfg(windows)]
mod imp {
    use openxr as xr;

    pub fn binding() -> Result<(xr::opengl::SessionCreateInfo, libloading::Library), String> {
        // SAFETY: opengl32.dll is the library SDL draws with, already
        // loaded; the functions take nothing and return handles.
        unsafe {
            let library = libloading::Library::new("opengl32.dll").map_err(|e| format!("opengl32.dll: {}", e))?;
            let dc = *library.get::<unsafe extern "system" fn() -> isize>(b"wglGetCurrentDC\0").map_err(|e| e.to_string())?;
            let context =
                *library.get::<unsafe extern "system" fn() -> isize>(b"wglGetCurrentContext\0").map_err(|e| e.to_string())?;
            let (h_dc, h_glrc) = (dc(), context());
            if h_dc == 0 || h_glrc == 0 {
                return Err("there is no current WGL context".into());
            }
            Ok((xr::opengl::SessionCreateInfo::Windows { h_dc, h_glrc }, library))
        }
    }
}
