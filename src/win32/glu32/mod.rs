//! GLU32.DLL: the OpenGL Utility Library, Microsoft's GLU 1.2 as
//! Windows 98 SE has it (1.2.2.0), on the built-in OPENGL32.DLL.
//!
//! Its image functions, gluScaleImage and gluBuild1DMipmaps and
//! gluBuild2DMipmaps, scale images as Microsoft's GLU does, the SGI
//! sample implementation's box filter (`image.rs`), and give OPENGL32
//! the levels of the texture. Its matrix functions, gluOrtho2D,
//! gluPerspective, gluLookAt and gluPickMatrix, multiply OPENGL32's
//! current matrix, and gluProject and gluUnProject map points between
//! object and window coordinates (`matrix.rs`). gluErrorString,
//! gluErrorUnicodeStringEXT and gluGetString give Windows 98 SE's
//! texts.
//!
//! The quadrics, the NURBS renderer and the tessellator aren't here:
//! their functions do nothing and log their first call, and the ones
//! that make an object (gluNewQuadric, gluNewNurbsRenderer, gluNewTess)
//! return NULL, as Windows' do when they are out of memory. OPENGL32 has
//! no 1D textures, so gluBuild1DMipmaps checks its arguments as
//! Windows' does and then logs that its levels go nowhere.
//!
//! The ordinals and arguments are Windows 98 SE's GLU32.DLL's; the
//! texts are those in its string table.

mod image;
mod matrix;

use super::stubs::{Api, Conv, Dll, Var};
use crate::cpu::Cpu;

pub static DLL: Dll = Dll {
    name: "GLU32.DLL",
    base: 0xBF76_0000,
    apis: APIS,
    vars: VARS,
    init: None,
    term: None,
    vtables: &[],
};

const fn stdcall(
    name: &'static str,
    ordinal: u16,
    args: u8,
    handler: super::stubs::Handler,
) -> Api {
    Api {
        name,
        ordinal,
        args,
        conv: Conv::Stdcall,
        handler,
    }
}

/// The functions, with Windows 98 SE's ordinals. Append only (see
/// `stubs`).
static APIS: &[Api] = &[
    stdcall("gluBeginCurve", 1, 1, missing),
    stdcall("gluBeginPolygon", 2, 1, missing),
    stdcall("gluBeginSurface", 3, 1, missing),
    stdcall("gluBeginTrim", 4, 1, missing),
    stdcall("gluBuild1DMipmaps", 5, 6, image::build_1d_mipmaps),
    stdcall("gluBuild2DMipmaps", 6, 7, image::build_2d_mipmaps),
    stdcall("gluCylinder", 7, 9, missing),
    stdcall("gluDeleteNurbsRenderer", 8, 1, missing),
    stdcall("gluDeleteQuadric", 9, 1, missing),
    stdcall("gluDeleteTess", 10, 1, missing),
    stdcall("gluDisk", 11, 7, missing),
    stdcall("gluEndCurve", 12, 1, missing),
    stdcall("gluEndPolygon", 13, 1, missing),
    stdcall("gluEndSurface", 14, 1, missing),
    stdcall("gluEndTrim", 15, 1, missing),
    stdcall("gluErrorString", 16, 1, error_string),
    stdcall("gluErrorUnicodeStringEXT", 17, 1, error_unicode_string),
    stdcall("gluGetNurbsProperty", 18, 3, missing),
    stdcall("gluGetString", 19, 1, get_string),
    stdcall("gluGetTessProperty", 20, 3, missing),
    stdcall("gluLoadSamplingMatrices", 21, 4, missing),
    stdcall("gluLookAt", 22, 18, matrix::look_at),
    stdcall("gluNewNurbsRenderer", 23, 0, missing),
    stdcall("gluNewQuadric", 24, 0, missing),
    stdcall("gluNewTess", 25, 0, missing),
    stdcall("gluNextContour", 26, 2, missing),
    stdcall("gluNurbsCallback", 27, 3, missing),
    stdcall("gluNurbsCurve", 28, 7, missing),
    stdcall("gluNurbsProperty", 29, 3, missing),
    stdcall("gluNurbsSurface", 30, 11, missing),
    stdcall("gluOrtho2D", 31, 8, matrix::ortho_2d),
    stdcall("gluPartialDisk", 32, 11, missing),
    stdcall("gluPerspective", 33, 8, matrix::perspective),
    stdcall("gluPickMatrix", 34, 9, matrix::pick_matrix_api),
    stdcall("gluProject", 35, 12, matrix::project_api),
    stdcall("gluPwlCurve", 36, 5, missing),
    stdcall("gluQuadricCallback", 37, 3, missing),
    stdcall("gluQuadricDrawStyle", 38, 2, missing),
    stdcall("gluQuadricNormals", 39, 2, missing),
    stdcall("gluQuadricOrientation", 40, 2, missing),
    stdcall("gluQuadricTexture", 41, 2, missing),
    stdcall("gluScaleImage", 42, 9, image::scale_image),
    stdcall("gluSphere", 43, 5, missing),
    stdcall("gluTessBeginContour", 44, 1, missing),
    stdcall("gluTessBeginPolygon", 45, 2, missing),
    stdcall("gluTessCallback", 46, 3, missing),
    stdcall("gluTessEndContour", 47, 1, missing),
    stdcall("gluTessEndPolygon", 48, 1, missing),
    stdcall("gluTessNormal", 49, 7, missing),
    stdcall("gluTessProperty", 50, 4, missing),
    stdcall("gluTessVertex", 51, 3, missing),
    stdcall("gluUnProject", 52, 12, matrix::un_project_api),
];

// GLU's errors.
pub const GLU_INVALID_ENUM: u32 = 100_900;
pub const GLU_INVALID_VALUE: u32 = 100_901;
pub const GLU_OUT_OF_MEMORY: u32 = 100_902;

// gluGetString's names.
const GLU_VERSION: u32 = 100_800;
const GLU_EXTENSIONS: u32 = 100_801;
const GL_EXTENSIONS: u32 = 0x1F03;

/// The texts of Windows 98 SE's GLU32 that the program gets a pointer
/// to: gluGetString's version and its two answers about extensions,
/// then the errors of OpenGL and of GLU, its tessellator and its NURBS
/// renderer, by their codes.
const T_VERSION: usize = 0;
const T_BGRA: usize = 1;
const T_NONE: usize = 2;
const TEXTS: &[(Option<u32>, &str)] = &[
    (None, "1.2.2.0 Microsoft Corporation"),
    (None, "GL_EXT_bgra"),
    (None, ""),
    (Some(0), "no error"),
    (Some(0x0500), "invalid enumerant"),
    (Some(0x0501), "invalid value"),
    (Some(0x0502), "invalid operation"),
    (Some(0x0503), "stack overflow"),
    (Some(0x0504), "stack underflow"),
    (Some(0x0505), "out of memory"),
    (Some(GLU_INVALID_ENUM), "invalid enumerant"),
    (Some(GLU_INVALID_VALUE), "invalid value"),
    (Some(GLU_OUT_OF_MEMORY), "out of memory"),
    // GLU_TESS_ERROR1 to GLU_TESS_ERROR6.
    (Some(100_151), "missing gluTessBeginPolygon"),
    (Some(100_152), "missing gluTessBeginContour"),
    (Some(100_153), "missing gluTessEndPolygon"),
    (Some(100_154), "missing gluTessEndContour"),
    (Some(100_155), "tesselation coordinate too large"),
    (Some(100_156), "need combine callback"),
    // GLU_NURBS_ERROR1 to GLU_NURBS_ERROR37.
    (Some(100_251), "spline order un-supported"),
    (Some(100_252), "too few knots"),
    (Some(100_253), "valid knot range is empty"),
    (Some(100_254), "decreasing knot sequence knot"),
    (
        Some(100_255),
        "knot multiplicity greater than order of spline",
    ),
    (Some(100_256), "endcurve() must follow bgncurve()"),
    (Some(100_257), "bgncurve() must precede endcurve()"),
    (Some(100_258), "missing or extra geometric data"),
    (Some(100_259), "can't draw pwlcurves"),
    (Some(100_260), "missing or extra domain data"),
    (Some(100_261), "missing or extra domain data"),
    (Some(100_262), "endtrim() must precede endsurface()"),
    (Some(100_263), "bgnsurface() must precede endsurface()"),
    (Some(100_264), "curve of improper type passed as trim curve"),
    (Some(100_265), "bgnsurface() must precede bgntrim()"),
    (Some(100_266), "endtrim() must follow bgntrim()"),
    (Some(100_267), "bgntrim() must precede endtrim()"),
    (Some(100_268), "invalid or missing trim curve"),
    (Some(100_269), "bgntrim() must precede pwlcurve()"),
    (Some(100_270), "pwlcurve referenced twice"),
    (Some(100_271), "pwlcurve and nurbscurve mixed"),
    (Some(100_272), "improper usage of trim data type"),
    (Some(100_273), "nurbscurve referenced twice"),
    (Some(100_274), "nurbscurve and pwlcurve mixed"),
    (Some(100_275), "nurbssurface referenced twice"),
    (Some(100_276), "invalid property"),
    (Some(100_277), "endsurface() must follow bgnsurface()"),
    (Some(100_278), "intersecting or misoriented trim curves"),
    (Some(100_279), "intersecting trim curves"),
    (Some(100_280), "UNUSED"),
    (Some(100_281), "unconnected trim curves"),
    (Some(100_282), "unknown knot error"),
    (Some(100_283), "negative vertex count encountered"),
    (Some(100_284), "negative byte-stride encounteed"),
    (Some(100_285), "unknown type descriptor"),
    (Some(100_286), "null control point reference"),
    (Some(100_287), "duplicate point on pwlcurve"),
];

/// The bytes each text has in the DLL's data: in ANSI, and in UTF-16
/// for gluErrorUnicodeStringEXT.
const TEXT_SIZE: u32 = 64;
const WIDE_SIZE: u32 = 2 * TEXT_SIZE;

// The DLL's variables: the texts, in ANSI and in UTF-16.
const V_TEXTS: usize = 0;
const V_WIDE: usize = 1;

static VARS: &[Var] = &[
    Var {
        name: "texts",
        ordinal: 0,
        size: TEXTS.len() as u32 * TEXT_SIZE,
    },
    Var {
        name: "wide",
        ordinal: 0,
        size: TEXTS.len() as u32 * WIDE_SIZE,
    },
];

fn dll_index() -> usize {
    super::stubs::DLLS
        .iter()
        .position(|d| std::ptr::eq(*d, &DLL))
        .expect("GLU32 is a built-in DLL")
}

/// The text of the error `code`, written into the DLL's data, in
/// UTF-16 with `wide`: its address, or NULL for a code with none.
fn error_text(cpu: &mut Cpu, code: u32, wide: bool) -> u32 {
    match TEXTS.iter().position(|&(c, _)| c == Some(code)) {
        Some(i) => text(cpu, i, wide),
        None => 0,
    }
}

/// TEXTS[i], written into the DLL's data, in UTF-16 with `wide`: its
/// address.
fn text(cpu: &mut Cpu, i: usize, wide: bool) -> u32 {
    let s = TEXTS[i].1;
    let (var, size) = if wide {
        (V_WIDE, WIDE_SIZE)
    } else {
        (V_TEXTS, TEXT_SIZE)
    };
    let at = super::stubs::var_address(dll_index(), var) + size * i as u32;
    let mut bytes: Vec<u8> = if wide {
        s.encode_utf16().flat_map(u16::to_le_bytes).collect()
    } else {
        s.as_bytes().to_vec()
    };
    bytes.resize(bytes.len() + if wide { 2 } else { 1 }, 0);
    cpu.bus.guest_write_bytes(at, &bytes);
    at
}

/// gluErrorString(error): the text of an error of OpenGL or GLU, NULL
/// for a code that isn't one. Windows' reads past the end of its table
/// for GLU_TESS_ERROR7 and 8, which have no text; here they have none.
fn error_string(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(error_text(cpu, args[0], false))
}

/// gluErrorUnicodeStringEXT(error): gluErrorString's text in UTF-16.
fn error_unicode_string(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(error_text(cpu, args[0], true))
}

/// gluGetString(name): the version, or the extensions, GL_EXT_bgra
/// where OpenGL has it, which the built-in OPENGL32 doesn't; NULL for
/// another name.
fn get_string(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(match args[0] {
        GLU_VERSION => text(cpu, T_VERSION, false),
        GLU_EXTENSIONS => {
            let ext = super::opengl32::get_string(cpu, &[GL_EXTENSIONS]).unwrap_or(0);
            let has = ext != 0 && read_c_string(cpu, ext).contains("GL_EXT_bgra");
            text(cpu, if has { T_BGRA } else { T_NONE }, false)
        }
        _ => 0,
    })
}

/// The zero-ended string at `at`, at most 4 KB of it.
fn read_c_string(cpu: &mut Cpu, at: u32) -> String {
    let mut out = Vec::new();
    for i in 0..4096 {
        let b = cpu.bus.guest_read_8(at + i);
        if b == 0 {
            break;
        }
        out.push(b);
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Log the first time `id` says `what`.
fn log_once(cpu: &mut Cpu, id: u16, what: &str) {
    let logged = &mut cpu.bus.win32.opengl32.glu_logged;
    if !logged.contains(&id) {
        logged.push(id);
        cpu.bus.log_string(&format!("[W32] GLU32.{what}"));
    }
}

/// A function that isn't here: it does nothing and returns 0, and its
/// first call is logged. Its name is the stub's that called it.
fn missing(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    let base = DLL.base + 0x1000;
    let stub = cpu.eip().wrapping_sub(4).wrapping_sub(base) / super::stubs::STUB;
    let name = APIS.get(stub as usize).map_or("?", |a| a.name);
    log_once(
        cpu,
        stub as u16,
        &format!("{name}: not implemented, ignored"),
    );
    Some(0)
}
