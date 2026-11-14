//! Display lists (OpenGL 1.1, section 5.4): glNewList, glEndList,
//! glCallList, glCallLists, glListBase, glGenLists, glIsList and
//! glDeleteLists.
//!
//! A list is a recording of the calls the program made between
//! glNewList and glEndList. Every call of an OPENGL32 export comes
//! through `call`, from the stub that read its arguments off the
//! program's stack (`stubs::call`). While the current context has a
//! list open, a call that OpenGL compiles is added to the list as the
//! function's place in the exports and its arguments, and with
//! GL_COMPILE_AND_EXECUTE its handler runs as well. glCallList gives
//! each recorded call to the handler the program's own call reaches,
//! so a function has one implementation, and one that does nothing
//! yet (`missing`) starts working in lists when it gets its handler.
//!
//! What a function's pointer arguments point at is copied when the
//! call is compiled, as OpenGL has it: `describe` tells for every
//! export which arguments are pointers and how many bytes each has
//! (`Size`), by the call's own rules: a vector's length, the values
//! of a parameter name, an image's rows under the unpack state. A
//! replay writes the copy into a block of the process's memory that
//! the context keeps for it and passes its address, so the handlers
//! read it as they read the program's. An image is copied as tight
//! rows and replayed with the unpack state at its defaults of one
//! byte's alignment and nothing skipped.
//!
//! The functions OpenGL executes at once inside a list are `When::Now`:
//! the queries, the pixel store, texture names, selection and feedback
//! buffers, and the client's state (the arrays and their pointers).
//! glArrayElement, glDrawArrays and glDrawElements are compiled as the
//! vertices the arrays have at that moment: glBegin, one glColor,
//! glNormal, glTexCoord and glVertex call of the array's size and
//! type for each element, and glEnd.
//!
//! A list is three flat arrays (`List`): its calls, their argument
//! words and the bytes copied for them.
//!
//! A run of calls from a glBegin to its glEnd that only give vertices
//! (glVertex, glColor, glNormal, glTexCoord and glMultiTexCoordARB) is
//! also kept as those vertices (`Batch`), made once when the list is
//! compiled. A replay draws a batch in one step, as glDrawArrays draws
//! the arrays' vertices (`draw::draw_run`), in place of a call for
//! each value: the vertices are lit and transformed as the context is
//! at the replay, those given before the run set a color, a normal or
//! texture coordinates take the current ones, and the current values
//! after it are the run's last. The recording stays what the list is:
//! the saved state has it alone, a batch is made from it again after a
//! load, and the calls are replayed one by one where a batch can't
//! stand for them (inside a glBegin that is still open, and under the
//! 3D trace, which lists every call).
//!
//! Lists belong to a group of contexts (`Shared`): a context starts
//! with a group of its own, and wglShareLists puts a context into
//! another's. A group goes with the last context that has it.

use super::super::kernel32;
use super::super::stubs::{self, Api};
use super::exports::APIS;
use super::texture::{self, GL_TEXTURE_2D, GL_TEXTURE0, IMAGE_MOST};
use super::{
    Array, CAP_COLOR_MATERIAL, Context, GL_INVALID_ENUM, GL_INVALID_OPERATION, GL_INVALID_VALUE,
    GL_OUT_OF_MEMORY, State, UNITS, ctx, current, draw, error, imm, matrix, pname, state,
};
use crate::cpu::Cpu;
use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, OnceLock};

/// How deep lists may call lists (GL_MAX_LIST_NESTING): OpenGL 1.1's
/// least.
pub const MAX_NESTING: u32 = 64;
/// The most names one glGenLists gives.
const MOST_NAMES: u32 = 0x10_0000;

const GL_COMPILE: u32 = 0x1300;
const GL_COMPILE_AND_EXECUTE: u32 = 0x1301;
const GL_LIST_MODE: u32 = 0x0B30;
const GL_MAX_LIST_NESTING: u32 = 0x0B31;
const GL_LIST_BASE: u32 = 0x0B32;
const GL_LIST_INDEX: u32 = 0x0B33;
const GL_POLYGON: u32 = 9;
const GL_BYTE: u32 = 0x1400;
const GL_UNSIGNED_BYTE: u32 = 0x1401;
const GL_UNSIGNED_SHORT: u32 = 0x1403;
const GL_UNSIGNED_INT: u32 = 0x1405;
const GL_DOUBLE: u32 = 0x140A;
const GL_BITMAP: u32 = 0x1A00;
const GL_RGBA: u32 = 0x1908;

/// A recorded call: the function's place in the exports, the
/// arguments that are copies (a bit each, by the argument's place),
/// and where its argument words start in the list's `words`. The
/// word of a copied argument is where its bytes are in the list's
/// `data`: their length in four bytes, then the bytes.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Call {
    pub api: u16,
    pub copied: u16,
    pub args: u32,
}

/// A display list: its calls in order, their argument words, and the
/// bytes copied for their pointer arguments. `batches` are its runs of
/// vertices, made from those the first time they are asked for.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct List {
    pub calls: Vec<Call>,
    pub words: Vec<u32>,
    pub data: Vec<u8>,
    batches: OnceLock<Vec<Batch>>,
}

impl List {
    /// Its runs of vertices, in the order of their calls.
    fn batches(&self) -> &[Batch] {
        self.batches.get_or_init(|| batches(self))
    }
}

/// A run of a list's calls kept as its vertices: the calls from
/// `first`, a glBegin of `kind` primitives, to its glEnd, `calls` of
/// them, all of the others one of `imm`'s functions. `inherit` tells
/// which vertices came before the run's first color, normal and
/// texture coordinates, and `last` are the ones it set last, which
/// are the current ones after it.
#[derive(Clone, Debug, Default, PartialEq)]
struct Batch {
    first: u32,
    calls: u32,
    kind: u32,
    vertices: Vec<draw::Whole>,
    inherit: draw::Inherit,
    last: Last,
}

/// The current values a run leaves: those it set.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Last {
    color: Option<[f32; 4]>,
    normal: Option<[f32; 3]>,
    tex: [Option<[f32; 4]>; UNITS],
}

/// The list a context has open between glNewList and glEndList: its
/// name and mode, whether a recorded glBegin has no glEnd yet, and
/// the calls so far.
#[derive(Clone, Debug, Default)]
pub struct Open {
    pub name: u32,
    pub mode: u32,
    begun: bool,
    list: List,
}

/// A context's part of the lists: the group whose lists it has (0
/// until it needs one), glListBase's base, the list it has open, and
/// the block of the process's memory its replays put copied data in.
#[derive(Clone, Debug, Default)]
pub struct Own {
    pub group: u32,
    pub base: u32,
    pub open: Option<Open>,
    scratch: u32,
    scratch_size: u32,
}

impl Own {
    /// glGet's values of the lists' state.
    pub fn get(&self, p: u32) -> Option<f64> {
        let open = self.open.as_ref();
        Some(match p {
            GL_LIST_BASE => self.base as f64,
            GL_LIST_INDEX => open.map_or(0, |o| o.name) as f64,
            GL_LIST_MODE => open.map_or(0, |o| o.mode) as f64,
            GL_MAX_LIST_NESTING => MAX_NESTING as f64,
            _ => return None,
        })
    }
}

/// The lists of a group, by their names.
type Names = BTreeMap<u32, Arc<List>>;

/// The lists of every context: the groups by their numbers, the last
/// number given, and how many contexts have a list open (`call`
/// records nothing while it is 0). `depth` counts the lists running
/// inside one another, and `replaying` is the function a replay is
/// calling; both are set only inside one call of the program's.
#[derive(Clone, Debug, Default)]
pub struct Shared {
    groups: Vec<(u32, Names)>,
    last_group: u32,
    compiling: u32,
    depth: u32,
    replaying: Option<u16>,
}

impl Shared {
    /// The export a replay is calling, for a handler that tells which
    /// function it stands for by the stub that called it.
    pub fn replaying(&self) -> Option<u16> {
        self.replaying
    }
}

crate::state_fields!(Call { api, copied, args });
crate::state_fields!(List { calls, words, data } skip { batches });
crate::state_fields!(Open {
    name,
    mode,
    begun,
    list
});
crate::state_fields!(Own {
    group,
    base,
    open,
    scratch,
    scratch_size
});
crate::state_fields!(Shared {
    groups,
    last_group,
    compiling
} skip { depth, replaying });

// What each export is to a list.

/// How many bytes a pointer argument points at.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Size {
    /// This many.
    Bytes(u32),
    /// As many values as the parameter name in the argument before it
    /// has (`pname::values`), of this many bytes each.
    Pname(u32),
    /// As many values as the count in the argument at this place, of
    /// this many bytes each.
    Count(u8, u32),
    /// glCallLists' names: the count in the first argument, of the type
    /// in the second.
    Names,
    /// An image under the unpack state: the places of its width, of
    /// its height (one row without) and of its format, with the type
    /// in the argument after the format.
    Image {
        width: u8,
        height: Option<u8>,
        format: u8,
    },
    /// A bitmap under the unpack state: the places of its width and
    /// height, or 32 by 32 without (glPolygonStipple).
    Bitmap(Option<(u8, u8)>),
}

/// A pointer argument: its place and what it points at.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Pointer {
    arg: u8,
    size: Size,
}

/// What a call inside a list does.
#[derive(Clone, Copy, Debug, PartialEq)]
enum When {
    /// It runs at once and isn't recorded.
    Now,
    /// It is recorded, and runs as well with GL_COMPILE_AND_EXECUTE.
    Compiled,
    /// glArrayElement, glDrawArrays and glDrawElements: recorded as
    /// the vertices the arrays have.
    Element,
    Arrays,
    Elements,
}

/// An export for the lists: when it runs, and its pointer arguments.
/// The other arguments are values, as many as the export has
/// (`Api::args`).
#[derive(Clone, Copy, Debug, PartialEq)]
struct Desc {
    when: When,
    pointers: [Option<Pointer>; 2],
}

impl Desc {
    const fn new(when: When) -> Desc {
        Desc {
            when,
            pointers: [None; 2],
        }
    }

    /// A compiled function whose argument at `arg` points at `size`.
    fn pointer(arg: usize, size: Size) -> Desc {
        let p = Pointer {
            arg: arg as u8,
            size,
        };
        Desc {
            when: When::Compiled,
            pointers: [Some(p), None],
        }
    }

    /// The same with a second pointer.
    fn and(mut self, arg: usize, size: Size) -> Desc {
        self.pointers[1] = Some(Pointer {
            arg: arg as u8,
            size,
        });
        self
    }

    /// Whether its data is an image, which a replay gives with the
    /// unpack state at its defaults.
    fn image(&self) -> bool {
        let image = |p: &Pointer| matches!(p.size, Size::Image { .. } | Size::Bitmap(_));
        self.pointers.iter().flatten().any(image)
    }
}

/// Whether OpenGL executes the function `name` at once inside a list
/// (section 5.4): the queries, glIsList and its like, the list
/// functions that aren't compiled, glFinish and glFlush, glReadPixels,
/// the pixel store, texture names, the selection and feedback buffers
/// and glRenderMode, and the client's state. The WGL functions aren't
/// OpenGL's.
fn runs_now(name: &str) -> bool {
    const NAMES: &[&str] = &[
        "glIsEnabled",
        "glIsList",
        "glIsTexture",
        "glNewList",
        "glEndList",
        "glGenLists",
        "glDeleteLists",
        "glFinish",
        "glFlush",
        "glReadPixels",
        "glPixelStoref",
        "glPixelStorei",
        "glGenTextures",
        "glDeleteTextures",
        "glAreTexturesResident",
        "glFeedbackBuffer",
        "glSelectBuffer",
        "glRenderMode",
        "glEnableClientState",
        "glDisableClientState",
        "glInterleavedArrays",
        "glPushClientAttrib",
        "glPopClientAttrib",
        "glClientActiveTextureARB",
        "glLockArraysEXT",
        "glUnlockArraysEXT",
    ];
    name.starts_with("wgl")
        || name.starts_with("glGet")
        || name.ends_with("Pointer")
        || NAMES.contains(&name)
}

/// The values a function named as a vector takes, and the bytes of
/// one: glVertex3fv has three of four bytes, glFogiv as many as its
/// parameter name has (None) of four. None for a name that isn't one.
fn vector(name: &str) -> Option<(Option<u32>, u32)> {
    const TYPES: [(&str, u32); 8] = [
        ("ub", 1),
        ("us", 2),
        ("ui", 4),
        ("b", 1),
        ("s", 2),
        ("i", 4),
        ("f", 4),
        ("d", 8),
    ];
    let name = name.strip_suffix("ARB").unwrap_or(name);
    let rest = name.strip_suffix('v')?;
    let (rest, bytes) = TYPES
        .iter()
        .find_map(|&(t, bytes)| Some((rest.strip_suffix(t)?, bytes)))?;
    let count = rest.chars().next_back().and_then(|c| c.to_digit(10));
    Some((count, bytes))
}

/// What the export `api` is to a list.
fn describe(api: &Api) -> Desc {
    let name = api.name;
    let last = (api.args as usize).saturating_sub(1);
    if runs_now(name) {
        return Desc::new(When::Now);
    }
    let image = |width: u8, height: Option<u8>, format: u8| Size::Image {
        width,
        height,
        format,
    };
    match name {
        "glArrayElement" => Desc::new(When::Element),
        "glDrawArrays" => Desc::new(When::Arrays),
        "glDrawElements" => Desc::new(When::Elements),
        "glLoadMatrixf" | "glMultMatrixf" => Desc::pointer(0, Size::Bytes(64)),
        "glLoadMatrixd" | "glMultMatrixd" => Desc::pointer(0, Size::Bytes(128)),
        "glClipPlane" => Desc::pointer(1, Size::Bytes(32)),
        "glEdgeFlagv" => Desc::pointer(0, Size::Bytes(1)),
        "glCallLists" => Desc::pointer(2, Size::Names),
        "glPrioritizeTextures" => Desc::pointer(1, Size::Count(0, 4)).and(2, Size::Count(0, 4)),
        "glPixelMapfv" | "glPixelMapuiv" => Desc::pointer(2, Size::Count(1, 4)),
        "glPixelMapusv" => Desc::pointer(2, Size::Count(1, 2)),
        "glPolygonStipple" => Desc::pointer(0, Size::Bitmap(None)),
        "glBitmap" => Desc::pointer(6, Size::Bitmap(Some((0, 1)))),
        "glDrawPixels" => Desc::pointer(4, image(0, Some(1), 2)),
        "glTexImage1D" => Desc::pointer(7, image(3, None, 5)),
        "glTexImage2D" => Desc::pointer(8, image(3, Some(4), 6)),
        "glTexSubImage1D" => Desc::pointer(6, image(3, None, 4)),
        "glTexSubImage2D" => Desc::pointer(8, image(4, Some(5), 6)),
        // glMap1 and glMap2 do nothing yet, so a list keeps their
        // control points' address and none of the points. They need a
        // size here when they get a handler.
        _ => match vector(name) {
            // glRectfv(v1, v2): two corners.
            Some((None, bytes)) if name.starts_with("glRect") => {
                Desc::pointer(0, Size::Bytes(2 * bytes)).and(1, Size::Bytes(2 * bytes))
            }
            Some((None, bytes)) if name.starts_with("glIndex") => {
                Desc::pointer(last, Size::Bytes(bytes))
            }
            Some((None, bytes)) => Desc::pointer(last, Size::Pname(bytes)),
            Some((Some(n), bytes)) => Desc::pointer(last, Size::Bytes(n * bytes)),
            None => Desc::new(When::Compiled),
        },
    }
}

/// Every export's description, by its place.
fn descs() -> &'static [Desc] {
    static DESCS: OnceLock<Vec<Desc>> = OnceLock::new();
    DESCS.get_or_init(|| APIS.iter().map(describe).collect())
}

/// The place of the export `name`.
fn place_of(name: &str) -> Option<usize> {
    static PLACES: OnceLock<HashMap<&'static str, u16>> = OnceLock::new();
    let places = PLACES.get_or_init(|| {
        let places = APIS.iter().enumerate();
        places.map(|(at, api)| (api.name, at as u16)).collect()
    });
    places.get(name).map(|&at| at as usize)
}

/// Which of `imm`'s functions each export is, by its place.
fn attributes() -> &'static [Option<imm::Spec>] {
    static SPECS: OnceLock<Vec<Option<imm::Spec>>> = OnceLock::new();
    SPECS.get_or_init(|| APIS.iter().map(|api| imm::spec(api.name)).collect())
}

/// The places of glBegin and glEnd.
fn begin_end() -> (usize, usize) {
    static PLACES: OnceLock<(usize, usize)> = OnceLock::new();
    *PLACES.get_or_init(|| {
        let of = |name| place_of(name).expect("an export of OPENGL32");
        (of("glBegin"), of("glEnd"))
    })
}

/// The stub id of the export at `at`, as the built-in Windows numbers
/// its functions.
fn stub_id(at: usize) -> u16 {
    static DLL: OnceLock<usize> = OnceLock::new();
    let dll = *DLL.get_or_init(|| {
        let dlls = stubs::DLLS.iter();
        let found = dlls
            .into_iter()
            .position(|dll| std::ptr::eq(*dll, &super::DLL));
        found.expect("OPENGL32 is a built-in DLL")
    });
    stubs::id(dll, at)
}

// The sizes of what pointers point at.

/// The bytes of one of glCallLists' names of the type `kind`: GL_BYTE
/// to GL_4_BYTES.
fn name_size(kind: u32) -> Option<usize> {
    const SIZES: [usize; 10] = [1, 1, 2, 2, 4, 4, 4, 2, 3, 4];
    SIZES.get(kind.checked_sub(GL_BYTE)? as usize).copied()
}

/// The bytes of a pixel of `format` and `kind`: as the textures read
/// them (`texture::pixel_layout`), and for the stencil and depth
/// formats, which glDrawPixels alone takes, one value of the type.
fn pixel_bytes(cpu: &mut Cpu, format: u32, kind: u32) -> Option<u32> {
    let types = super::wgl::packed_pixels(cpu);
    match texture::pixel_layout(format, kind, types) {
        Ok((pixel, _, _)) => Some(pixel),
        // GL_STENCIL_INDEX, GL_DEPTH_COMPONENT.
        Err(_) if matches!(format, 0x1901 | 0x1902) => {
            draw::type_size(kind).filter(|_| kind != GL_DOUBLE)
        }
        Err(_) => None,
    }
}

/// Add the `len` bytes at `at` of the program's memory to `out`.
/// Nothing is added when there are none or they can't all be read.
fn read(cpu: &mut Cpu, at: u32, len: usize, out: &mut Vec<u8>) -> bool {
    if len == 0 || len > IMAGE_MOST || !cpu.bus.guest_probe(at, len, false) {
        return false;
    }
    let from = out.len();
    out.resize(from + len, 0);
    cpu.bus.guest_read_bytes(at, &mut out[from..]);
    true
}

/// Add to `out` the image at `at`, `width` by `height`, as rows with
/// nothing between or before them: the pixels the unpack state in
/// force picks. A bitmap's rows start at a byte's first bit. An image
/// with no pixels has nothing to copy: its rows would be zero bytes
/// apart, and the pixels skipped could still make one to read.
fn copy_image(
    cpu: &mut Cpu,
    at: u32,
    (width, height): (u32, u32),
    layout: Option<(u32, u32)>,
    out: &mut Vec<u8>,
) -> bool {
    let unpack = ctx(cpu).map_or([4, 0, 0, 0], |c| c.unpack);
    if width == 0 || height == 0 || (width as i32) < 0 || (height as i32) < 0 {
        return false;
    }
    let mut bytes = Vec::new();
    let Some((format, kind)) = layout.filter(|l| l.1 != GL_BITMAP) else {
        // Bits: a row is the row length's bits, in whole bytes of the
        // alignment, and the pixels skipped are bits of the first.
        let place = || {
            let row_bits = if unpack[1] > 0 { unpack[1] } else { width };
            let stride = row_bits
                .div_ceil(8)
                .checked_next_multiple_of(unpack[0].max(1))?;
            let start = at.checked_add(unpack[2].checked_mul(stride)?)?;
            let last = unpack[3].checked_add(width)?.div_ceil(8);
            let rows = height.saturating_sub(1).checked_mul(stride)?;
            Some((stride as usize, start, rows.checked_add(last)? as usize))
        };
        let Some((stride, start, len)) = place() else {
            return false;
        };
        if !read(cpu, start, len, &mut bytes) {
            return false;
        }
        let row = width.div_ceil(8) as usize;
        let from = out.len();
        out.resize(from + row * height as usize, 0);
        for y in 0..height as usize {
            for x in 0..width as usize {
                let bit = unpack[3] as usize + x;
                let on = bytes[y * stride + bit / 8] >> (7 - bit % 8) & 1;
                out[from + y * row + x / 8] |= on << (7 - x % 8);
            }
        }
        return true;
    };
    let Some(pixel) = pixel_bytes(cpu, format, kind) else {
        return false;
    };
    let Some((stride, skip, len)) = texture::span(unpack, width, height, pixel) else {
        return false;
    };
    let Some(start) = at.checked_add(skip) else {
        return false;
    };
    let row = (width * pixel) as usize;
    if stride as usize == row {
        return read(cpu, start, len, out);
    }
    if !read(cpu, start, len, &mut bytes) {
        return false;
    }
    for y in 0..height as usize {
        out.extend_from_slice(&bytes[y * stride as usize..][..row]);
    }
    true
}

/// Add to `out` what the pointer `p` of a call with `args` points at,
/// read now. Nothing is added for a null pointer, a size the call's
/// arguments don't give, or memory that can't be read: the call is
/// then recorded with the pointer as it is, and a replay does with it
/// what the program's own call would.
fn copy(cpu: &mut Cpu, p: &Pointer, args: &[u32], out: &mut Vec<u8>) -> bool {
    let arg = |i: u8| args.get(i as usize).copied().unwrap_or(0);
    let at = arg(p.arg);
    if at == 0 {
        return false;
    }
    let count = |n: u32, bytes: usize| (n as usize).checked_mul(bytes).filter(|_| (n as i32) > 0);
    let len = match p.size {
        Size::Bytes(n) => Some(n as usize),
        Size::Pname(bytes) => Some(pname::values(arg(p.arg.wrapping_sub(1))) * bytes as usize),
        Size::Count(n, bytes) => count(arg(n), bytes as usize),
        Size::Names => name_size(arg(1)).and_then(|bytes| count(arg(0), bytes)),
        Size::Image {
            width,
            height,
            format,
        } => {
            let size = (arg(width), height.map_or(1, arg));
            let layout = (arg(format), arg(format + 1));
            return copy_image(cpu, at, size, Some(layout), out);
        }
        Size::Bitmap(size) => {
            let size = size.map_or((32, 32), |(w, h)| (arg(w), arg(h)));
            return copy_image(cpu, at, size, None, out);
        }
    };
    len.is_some_and(|len| read(cpu, at, len, out))
}

// Recording.

/// Add the call of the export at `at` with `args` to the open list,
/// with what its pointers point at: `host`'s bytes for its first
/// pointer when the call is the built-in Windows' own, and otherwise
/// the program's memory. With `strict`, a call with a pointer whose
/// data can't be copied isn't added, and the result says so.
fn push(
    cpu: &mut Cpu,
    open: &mut Open,
    at: usize,
    args: &[u32],
    host: Option<&[u8]>,
    strict: bool,
) -> bool {
    let Some(desc) = descs().get(at) else {
        return false;
    };
    let list = &mut open.list;
    let (words, data) = (list.words.len(), list.data.len());
    list.words.extend_from_slice(args);
    let mut copied = 0;
    for p in desc.pointers.iter().flatten() {
        let offset = list.data.len();
        list.data.extend_from_slice(&[0; 4]);
        let fits = u32::try_from(offset).is_ok() && (p.arg as usize) < args.len();
        let done = fits
            && match host {
                Some(bytes) => {
                    list.data.extend_from_slice(bytes);
                    true
                }
                None => copy(cpu, p, args, &mut list.data),
            };
        if done {
            let len = (list.data.len() - offset - 4) as u32;
            list.data[offset..offset + 4].copy_from_slice(&len.to_le_bytes());
            list.words[words + p.arg as usize] = offset as u32;
            copied |= 1 << p.arg;
        } else if strict {
            list.words.truncate(words);
            list.data.truncate(data);
            return false;
        } else {
            list.data.truncate(offset);
        }
    }
    list.calls.push(Call {
        api: at as u16,
        copied,
        args: words as u32,
    });
    let (begin, end) = begin_end();
    if at == begin {
        open.begun = true;
    } else if at == end {
        open.begun = false;
    }
    true
}

/// The arrays that are on and the export that gives one element of
/// each: glVertex3fv for a vertex array of three floats.
struct Elements {
    vertex: (Array, usize),
    others: Vec<(Array, usize, Option<u32>)>,
}

/// The arrays of the current context as immediate-mode functions.
/// None without a vertex array, which no element draws without. An
/// array of a size and type no function takes gives nothing.
fn elements(cpu: &mut Cpu) -> Option<Elements> {
    let c = ctx(cpu)?;
    let export = |family: &str, a: &Array, unit: Option<u32>| {
        const TYPES: [&str; 7] = ["b", "ub", "s", "us", "i", "ui", "f"];
        if !a.enabled {
            return None;
        }
        let kind = match a.kind {
            GL_DOUBLE => "d",
            kind => TYPES.get(kind.checked_sub(GL_BYTE)? as usize)?,
        };
        let arb = if unit.is_some() { "ARB" } else { "" };
        let at = place_of(&format!("gl{family}{}{kind}v{arb}", a.size))?;
        Some((*a, at, unit.map(|u| GL_TEXTURE0 + u)))
    };
    let (vertex, at, _) = export("Vertex", &c.vertex_array, None)?;
    let mut others = Vec::new();
    others.extend(export("Color", &c.color_array, None));
    others.extend(export("Normal", &c.normal_array, None));
    for (unit, a) in c.tex_array.iter().enumerate().take(UNITS) {
        others.extend(match unit {
            0 => export("TexCoord", a, None),
            _ => export("MultiTexCoord", a, Some(unit as u32)),
        });
    }
    Some(Elements {
        vertex: (vertex, at),
        others,
    })
}

/// Record the element `i` of the arrays: its color, normal and
/// texture coordinates, then its vertex. One that can't be read is
/// left out.
fn push_element(cpu: &mut Cpu, open: &mut Open, e: &Elements, i: u32) {
    let address = |a: &Array| {
        let (_, stride) = draw::element_size(a)?;
        a.at.checked_add(i.checked_mul(stride)?)
    };
    let (vertex, at) = e.vertex;
    let Some(position) = address(&vertex) else {
        return;
    };
    if !cpu.bus.guest_probe(position, 1, false) {
        return;
    }
    for (a, at, unit) in &e.others {
        match (address(a), unit) {
            (Some(p), Some(unit)) => push(cpu, open, *at, &[*unit, p], None, true),
            (Some(p), None) => push(cpu, open, *at, &[p], None, true),
            (None, _) => false,
        };
    }
    push(cpu, open, at, &[position], None, true);
}

/// Record glDrawArrays or glDrawElements with `args` as glBegin, the
/// elements and glEnd, or glArrayElement as its element. A call whose
/// arguments are an error is recorded as it is: its handler reads no
/// array before it gives the error, so a replay gives the same.
fn push_arrays(cpu: &mut Cpu, open: &mut Open, at: usize, when: When, args: &[u32]) {
    if when == When::Element {
        if let Some(e) = elements(cpu) {
            push_element(cpu, open, &e, args[0]);
        }
        return;
    }
    let (mode, count) = match when {
        When::Arrays => (args[0], args[2]),
        _ => (args[0], args[1]),
    };
    let index_size = match (when, args[2]) {
        (When::Arrays, _) => Some(0),
        (_, GL_UNSIGNED_BYTE) => Some(1),
        (_, GL_UNSIGNED_SHORT) => Some(2),
        (_, GL_UNSIGNED_INT) => Some(4),
        _ => None,
    };
    let Some(index_size) = index_size.filter(|_| mode <= GL_POLYGON && (count as i32) >= 0) else {
        push(cpu, open, at, args, None, false);
        return;
    };
    // As the handlers have it: nothing between glBegin and glEnd, and
    // nothing without vertices.
    if count == 0 || open.begun {
        return;
    }
    let Some(e) = elements(cpu) else {
        return;
    };
    let indices: Vec<u32> = if when == When::Arrays {
        // The whole range has to be there, which also stops a count
        // that is a program's error.
        let (vertex, _) = e.vertex;
        let range = || {
            let (bytes, stride) = draw::element_size(&vertex)?;
            let start = vertex.at.checked_add(args[1].checked_mul(stride)?)?;
            let len = (count - 1).checked_mul(stride)?.checked_add(bytes)?;
            args[1].checked_add(count - 1)?;
            Some((start, len as usize))
        };
        match range() {
            Some((start, len)) if len <= 0x1000_0000 && cpu.bus.guest_probe(start, len, false) => {
                (args[1]..=args[1] + (count - 1)).collect()
            }
            _ => return,
        }
    } else {
        let mut bytes = Vec::new();
        if !read(cpu, args[3], count as usize * index_size, &mut bytes) {
            return;
        }
        let index = |b: &[u8]| b.iter().rev().fold(0, |v, &b| v << 8 | b as u32);
        bytes.chunks_exact(index_size).map(index).collect()
    };
    let (begin, end) = begin_end();
    push(cpu, open, begin, &[mode], None, false);
    for i in indices {
        push_element(cpu, open, &e, i);
    }
    push(cpu, open, end, &[], None, false);
}

/// The current context's open list, taken out of it while a call is
/// added.
fn take_open(cpu: &mut Cpu) -> Option<Open> {
    ctx(cpu)?.lists.open.take()
}

fn put_back(cpu: &mut Cpu, open: Open) {
    if let Some(c) = ctx(cpu) {
        c.lists.open = Some(open);
    }
}

/// A call of the export at `at` by the program, with the arguments its
/// stub read: the handler runs, unless the current context has a list
/// open and the function is one OpenGL compiles. That call is added
/// to the list, and its handler runs only with
/// GL_COMPILE_AND_EXECUTE.
pub fn call(cpu: &mut Cpu, at: usize, api: &Api, args: &[u32]) -> Option<u32> {
    if state(cpu).lists.compiling == 0 {
        return (api.handler)(cpu, args);
    }
    let when = descs().get(at).map_or(When::Now, |d| d.when);
    let open = if when == When::Now {
        None
    } else {
        take_open(cpu)
    };
    let Some(mut open) = open else {
        return (api.handler)(cpu, args);
    };
    match when {
        When::Compiled => {
            push(cpu, &mut open, at, args, None, false);
        }
        _ => push_arrays(cpu, &mut open, at, when, args),
    }
    let execute = open.mode == GL_COMPILE_AND_EXECUTE;
    put_back(cpu, open);
    if execute {
        (api.handler)(cpu, args)
    } else {
        Some(0)
    }
}

/// A call of the export `name` that a built-in DLL makes for the
/// program, as GLU32 makes OpenGL's: with a list open it is added to
/// the list, `data` being what its pointer points at. Whether that is
/// all there is to do: false when the caller has to do the call
/// itself, with no list open or with GL_COMPILE_AND_EXECUTE.
fn compiles(cpu: &mut Cpu, name: &str, args: &[u32], data: Option<&[u8]>) -> bool {
    if state(cpu).lists.compiling == 0 {
        return false;
    }
    let Some(at) = place_of(name) else {
        return false;
    };
    let Some(mut open) = take_open(cpu) else {
        return false;
    };
    push(cpu, &mut open, at, args, data, false);
    let only = open.mode != GL_COMPILE_AND_EXECUTE;
    put_back(cpu, open);
    only
}

/// Multiply the current matrix by `m` on its right, as glMultMatrixf
/// does, for GLU32's matrices.
pub fn mult_matrix(cpu: &mut Cpu, m: &[f32; 16]) {
    let bytes: Vec<u8> = m.iter().flat_map(|v| v.to_le_bytes()).collect();
    if !compiles(cpu, "glMultMatrixf", &[0], Some(&bytes)) {
        matrix::mult(cpu, m);
    }
}

/// glOrtho with the arguments `args`, for gluOrtho2D.
pub fn ortho(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if compiles(cpu, "glOrtho", args, None) {
        return Some(0);
    }
    matrix::ortho(cpu, args)
}

/// glTexImage2D of a level whose texels are red, green, blue and
/// alpha bytes (`texture::tex_image_rgba`), for gluBuild2DMipmaps.
pub fn tex_image_rgba(
    cpu: &mut Cpu,
    level: u32,
    internal: u32,
    width: u32,
    height: u32,
    texels: Vec<[u8; 4]>,
) {
    let args = [
        GL_TEXTURE_2D,
        level,
        internal,
        width,
        height,
        0,
        GL_RGBA,
        GL_UNSIGNED_BYTE,
        0,
    ];
    if !compiles(cpu, "glTexImage2D", &args, Some(texels.as_flattened())) {
        texture::tex_image_rgba(cpu, level, internal, width, height, texels);
    }
}

// The runs of vertices.

/// The run that starts with the glBegin at `first` of `list`'s calls.
/// None when the calls up to the glEnd aren't all vertices' values
/// with their data in the list, for a primitive there isn't, a
/// texture unit there isn't, and without a glEnd: those are replayed
/// call by call, with the errors and the open glBegin they leave.
fn batch_at(list: &List, first: usize) -> Option<Batch> {
    let (_, end) = begin_end();
    let attributes = attributes();
    let kind = *list.words.get(list.calls[first].args as usize)?;
    if kind > GL_POLYGON {
        return None;
    }
    let mut current = draw::Whole::default();
    let mut vertices = Vec::new();
    let mut last = Last::default();
    // How many vertices came before each value was first set.
    let (mut color, mut normal, mut tex) = (None, None, [None; UNITS]);
    for (n, call) in list.calls[first + 1..].iter().enumerate() {
        let at = call.api as usize;
        let count = vertices.len() as u32;
        if at == end {
            let inherit = draw::Inherit {
                color: color.unwrap_or(count),
                normal: normal.unwrap_or(count),
                tex: tex.map(|t| t.unwrap_or(count)),
            };
            return Some(Batch {
                first: first as u32,
                calls: n as u32 + 2,
                kind,
                vertices,
                inherit,
                last,
            });
        }
        let spec = attributes.get(at)?.as_ref()?;
        let from = call.args as usize;
        let words = list.words.get(from..from + spec.words())?;
        let array = match spec.pointer() {
            Some(arg) if call.copied >> arg & 1 != 0 => Some(copied(list, words[arg])),
            Some(_) => return None,
            None => None,
        };
        match imm::given(spec, words, array)? {
            imm::Given::Vertex(position) => vertices.push(draw::Whole {
                position,
                ..current
            }),
            imm::Given::Color(c) => {
                current.color = c;
                last.color = Some(c);
                color.get_or_insert(count);
            }
            imm::Given::Normal(n) => {
                current.normal = n;
                last.normal = Some(n);
                normal.get_or_insert(count);
            }
            imm::Given::TexCoord(unit, t) => {
                current.tex[unit] = t;
                last.tex[unit] = Some(t);
                tex[unit].get_or_insert(count);
            }
        }
    }
    None
}

/// The runs of vertices of `list`.
fn batches(list: &List) -> Vec<Batch> {
    let (begin, _) = begin_end();
    let mut out = Vec::new();
    let mut at = 0;
    while at < list.calls.len() {
        let batch = (list.calls[at].api as usize == begin).then(|| batch_at(list, at));
        match batch.flatten() {
            Some(batch) => {
                at += batch.calls as usize;
                out.push(batch);
            }
            None => at += 1,
        }
    }
    out
}

/// Draw the run `batch` in place of its calls. False when its calls
/// have to be replayed instead: under the 3D trace, which lists each
/// of them, and where `draw::draw_run` doesn't draw.
fn run_batch(cpu: &mut Cpu, batch: &Batch) -> bool {
    if super::super::trace3d::on(cpu) {
        return false;
    }
    if !draw::draw_run(cpu, batch.kind, &batch.vertices, &batch.inherit) {
        return false;
    }
    let Some(c) = ctx(cpu) else {
        return true;
    };
    let last = &batch.last;
    if let Some(color) = last.color {
        c.color = color;
        // As glColor: the material properties glColorMaterial tracks
        // follow the current color. The vertices were lit with their
        // own colors in those properties, so the last one is all the
        // materials keep.
        if c.caps & CAP_COLOR_MATERIAL != 0 {
            c.lighting.track(color);
        }
    }
    if let Some(normal) = last.normal {
        c.normal = normal;
    }
    for (unit, t) in last.tex.iter().enumerate() {
        if let Some(t) = *t {
            c.tex_coord[unit] = t;
        }
    }
    true
}

// Replaying.

/// The bytes copied for a call's argument whose word is `offset`.
fn copied(list: &List, offset: u32) -> &[u8] {
    let bytes = || {
        let data = list.data.get(offset as usize..)?;
        let len = u32::from_le_bytes(*data.first_chunk()?) as usize;
        data.get(4..4 + len)
    };
    bytes().unwrap_or(&[])
}

/// A block of the process's memory of `size` bytes or more for the
/// current context's replays: the one it has, or a larger one in its
/// place.
fn scratch(cpu: &mut Cpu, size: usize) -> Option<u32> {
    let own = &mut ctx(cpu)?.lists;
    if own.scratch != 0 && own.scratch_size as usize >= size {
        return Some(own.scratch);
    }
    let old = std::mem::take(&mut own.scratch);
    own.scratch_size = 0;
    if old != 0 {
        kernel32::free_pages(cpu, old);
    }
    let size = u32::try_from(size.max(1).checked_next_multiple_of(0x1000)?).ok()?;
    let at = kernel32::alloc_pages(cpu, size)?;
    let own = &mut ctx(cpu)?.lists;
    own.scratch = at;
    own.scratch_size = size;
    Some(at)
}

/// Write the copies of `call`'s pointer arguments into the context's
/// block and put their addresses in `args`. False when there is no
/// memory for them.
fn place(cpu: &mut Cpu, list: &List, call: &Call, args: &mut [u32]) -> bool {
    let is_copy = |i: usize| call.copied >> i & 1 != 0;
    let room = |len: usize| len.next_multiple_of(8);
    let total = (0..args.len())
        .filter(|&i| is_copy(i))
        .map(|i| room(copied(list, args[i]).len()))
        .sum();
    let Some(mut at) = scratch(cpu, total) else {
        return false;
    };
    for i in (0..args.len()).filter(|&i| is_copy(i)) {
        let bytes = copied(list, args[i]);
        cpu.bus.guest_write_bytes(at, bytes);
        args[i] = at;
        at += room(bytes.len()) as u32;
    }
    true
}

/// Call the handler of the export at `at` as the program's stub would:
/// the draws that wait are drawn before a function they can't wait
/// through, and a ToadForce's queue before one that reads the frame.
/// With `tight`, the call's image is the list's copy, which has the
/// rows the unpack state picked when it was compiled: the handler
/// reads it with the unpack state at its defaults of one byte's
/// alignment and nothing skipped.
fn issue(cpu: &mut Cpu, at: usize, api: &Api, args: &[u32], tight: bool) {
    let id = stub_id(at);
    if state(cpu).waiting.is_some() {
        super::flush_before(cpu, id);
    }
    stubs::settle_before(cpu, id);
    let unpack = if tight {
        ctx(cpu).map(|c| std::mem::replace(&mut c.unpack, [1, 0, 0, 0]))
    } else {
        None
    };
    let outer = state(cpu).lists.replaying.replace(at as u16);
    let result = (api.handler)(cpu, args);
    state(cpu).lists.replaying = outer;
    if let (Some(unpack), Some(c)) = (unpack, ctx(cpu)) {
        c.unpack = unpack;
    }
    if super::super::trace3d::on(cpu) {
        super::super::trace3d::call(cpu, "OPENGL32", api.name, args, result);
    }
}

/// Run the call `call` of `list`.
fn replay(cpu: &mut Cpu, list: &List, call: &Call) {
    let at = call.api as usize;
    let (Some(api), Some(desc)) = (APIS.get(at), descs().get(at)) else {
        return;
    };
    let mut args = [0u32; 16];
    let n = (api.args as usize).min(args.len());
    let Some(words) = list.words.get(call.args as usize..call.args as usize + n) else {
        return;
    };
    let args = &mut args[..n];
    args.copy_from_slice(words);
    if call.copied != 0 && !place(cpu, list, call, args) {
        return;
    }
    issue(cpu, at, api, args, call.copied != 0 && desc.image());
}

/// Run the calls of `list` in order, its runs of vertices each in one
/// step.
fn run(cpu: &mut Cpu, list: &List) {
    let mut batches = list.batches().iter().peekable();
    let mut at = 0;
    while let Some(call) = list.calls.get(at) {
        if let Some(batch) = batches.next_if(|b| b.first as usize == at)
            && run_batch(cpu, batch)
        {
            at += batch.calls as usize;
            continue;
        }
        replay(cpu, list, call);
        if cpu.bus.guest_faulted() {
            return;
        }
        at += 1;
    }
}

/// Run the list `name` of the current context's group, if there is
/// one and the lists running aren't `MAX_NESTING` deep already. The
/// list as it is now runs to its end, whatever its calls do to its
/// name.
fn execute(cpu: &mut Cpu, name: u32) {
    let Some((index, _)) = current(cpu) else {
        return;
    };
    let s = state(cpu);
    if s.lists.depth >= MAX_NESTING {
        return;
    }
    let Some(list) = names(s, index).and_then(|n| n.get(&name)).cloned() else {
        return;
    };
    s.lists.depth += 1;
    run(cpu, &list);
    state(cpu).lists.depth -= 1;
}

// The groups.

/// The lists the context at `index` has, if its group has any yet.
fn names(s: &State, index: usize) -> Option<&Names> {
    let group = s.contexts.get(index)?.as_ref()?.lists.group;
    let found = s.lists.groups.iter().find(|g| g.0 == group)?;
    Some(&found.1)
}

/// The lists the context at `index` has, in a new group of its own if
/// it has none.
fn names_mut(s: &mut State, index: usize) -> Option<&mut Names> {
    let own = &mut s.contexts.get_mut(index)?.as_mut()?.lists;
    if own.group == 0 {
        s.lists.last_group += 1;
        own.group = s.lists.last_group;
        s.lists.groups.push((own.group, Names::new()));
    }
    let group = own.group;
    let found = s.lists.groups.iter_mut().find(|g| g.0 == group)?;
    Some(&mut found.1)
}

/// The contexts changed: the groups no context has go, and the count
/// of open lists is taken again.
pub fn sweep(s: &mut State) {
    let contexts = || s.contexts.iter().flatten();
    let used: Vec<u32> = contexts().map(|c| c.lists.group).collect();
    let compiling = contexts().filter(|c| c.lists.open.is_some()).count() as u32;
    s.lists.groups.retain(|g| used.contains(&g.0));
    s.lists.compiling = compiling;
}

/// The context `c` was deleted: its memory for replays is freed, and
/// its lists go unless another context shares them.
pub fn context_gone(cpu: &mut Cpu, c: &Context) {
    if c.lists.scratch != 0 {
        kernel32::free_pages(cpu, c.lists.scratch);
    }
    sweep(state(cpu));
}

/// wglShareLists: the context at `to` takes the lists of the one at
/// `from`. As Windows has it, it fails when `to` has lists of its
/// own; contexts of two processes share nothing.
pub fn share(cpu: &mut Cpu, from: usize, to: usize) -> bool {
    let s = state(cpu);
    let pid = |s: &State, i: usize| s.contexts.get(i).and_then(Option::as_ref).map(|c| c.pid);
    if pid(s, from).is_none() || pid(s, from) != pid(s, to) {
        return false;
    }
    if from == to {
        return true;
    }
    if names(s, to).is_some_and(|n| !n.is_empty()) {
        return false;
    }
    if names_mut(s, from).is_none() {
        return false;
    }
    let group = s.contexts[from].as_ref().map_or(0, |c| c.lists.group);
    if let Some(c) = &mut s.contexts[to] {
        c.lists.group = group;
    }
    sweep(s);
    true
}

// The functions.

/// The current context's place, for a function that isn't allowed
/// between glBegin and glEnd: None there, with GL_INVALID_OPERATION,
/// and without a context.
fn outside_begin(cpu: &mut Cpu) -> Option<usize> {
    let (index, _) = current(cpu)?;
    if ctx(cpu)?.begin.is_some() {
        error(cpu, GL_INVALID_OPERATION);
        return None;
    }
    Some(index)
}

/// glNewList(list, mode): the calls from here to glEndList are the
/// list's.
pub fn new_list(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [name, mode] = [args[0], args[1]];
    let Some(index) = outside_begin(cpu) else {
        return Some(0);
    };
    let open = ctx(cpu).is_some_and(|c| c.lists.open.is_some());
    let e = if name == 0 {
        GL_INVALID_VALUE
    } else if mode != GL_COMPILE && mode != GL_COMPILE_AND_EXECUTE {
        GL_INVALID_ENUM
    } else if open {
        GL_INVALID_OPERATION
    } else {
        let s = state(cpu);
        if let Some(Some(c)) = s.contexts.get_mut(index) {
            c.lists.open = Some(Open {
                name,
                mode,
                ..Open::default()
            });
        }
        sweep(s);
        return Some(0);
    };
    error(cpu, e);
    Some(0)
}

/// glEndList(): the open list takes its name, in place of the list
/// that had it. With GL_COMPILE a list may end between a glBegin of
/// its own and the glEnd, which another list or the program gives:
/// the glBegin hasn't run. With GL_COMPILE_AND_EXECUTE it has, and
/// glEndList there is GL_INVALID_OPERATION and leaves the list open,
/// as any function OpenGL 1.1 doesn't allow between glBegin and glEnd.
pub fn end_list(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    let Some(index) = outside_begin(cpu) else {
        return Some(0);
    };
    let Some(mut open) = take_open(cpu) else {
        error(cpu, GL_INVALID_OPERATION);
        return Some(0);
    };
    let list = &mut open.list;
    list.calls.shrink_to_fit();
    list.words.shrink_to_fit();
    list.data.shrink_to_fit();
    // Its runs of vertices are made here, once, and not at its first
    // call.
    list.batches();
    let s = state(cpu);
    if let Some(names) = names_mut(s, index) {
        names.insert(open.name, Arc::new(open.list));
    }
    sweep(s);
    Some(0)
}

/// The first of `n` names in a row that no list of `names` has: after
/// the last one, or in the first gap that has the room when the names
/// reach the end of the numbers.
fn free_names(names: &Names, n: u32) -> Option<u32> {
    let after = names
        .keys()
        .next_back()
        .map_or(Some(1), |k| k.checked_add(1));
    if let Some(first) = after.filter(|first| first.checked_add(n - 1).is_some()) {
        return Some(first);
    }
    let mut from = 1u32;
    for &name in names.keys().filter(|&&name| name != 0) {
        if name - from >= n {
            return Some(from);
        }
        from = name.checked_add(1)?;
    }
    None
}

/// glGenLists(range): the first of `range` names in a row that had no
/// list, each now an empty list's; 0 for none.
pub fn gen_lists(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let n = args[0];
    let Some(index) = outside_begin(cpu) else {
        return Some(0);
    };
    if (n as i32) < 0 {
        error(cpu, GL_INVALID_VALUE);
        return Some(0);
    }
    if n == 0 {
        return Some(0);
    }
    let first = names_mut(state(cpu), index).and_then(|names| {
        let first = free_names(names, n).filter(|_| n <= MOST_NAMES)?;
        let empty = Arc::new(List::default());
        names.extend((first..=first + (n - 1)).map(|name| (name, empty.clone())));
        Some(first)
    });
    if first.is_none() {
        error(cpu, GL_OUT_OF_MEMORY);
    }
    Some(first.unwrap_or(0))
}

/// glDeleteLists(list, range): the lists of the `range` names from
/// `list` go; a name without one is passed over.
pub fn delete_lists(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [first, n] = [args[0], args[1]];
    let Some(index) = outside_begin(cpu) else {
        return Some(0);
    };
    if (n as i32) < 0 {
        error(cpu, GL_INVALID_VALUE);
        return Some(0);
    }
    let Some(last) = n.checked_sub(1).map(|n| first.saturating_add(n)) else {
        return Some(0);
    };
    if let Some(names) = names_mut(state(cpu), index) {
        // Only the names in the range are visited: a program that
        // deletes its lists one by one doesn't go over all of them
        // each time.
        let gone: Vec<u32> = names.range(first..=last).map(|(&name, _)| name).collect();
        for name in gone {
            names.remove(&name);
        }
    }
    Some(0)
}

/// glIsList(list): whether the name has a list, an empty one of
/// glGenLists' included.
pub fn is_list(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(index) = outside_begin(cpu) else {
        return Some(0);
    };
    let found = names(state(cpu), index).is_some_and(|n| n.contains_key(&args[0]));
    Some(found as u32)
}

/// glListBase(base): what glCallLists adds to its names.
pub fn list_base(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if outside_begin(cpu).is_some()
        && let Some(c) = ctx(cpu)
    {
        c.lists.base = args[0];
    }
    Some(0)
}

/// glCallList(list): the list's calls run; nothing for a name without
/// a list. It is allowed between glBegin and glEnd.
pub fn call_list(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    execute(cpu, args[0]);
    Some(0)
}

/// The `i`th of glCallLists' names in `bytes`, of the type `kind`:
/// integers as they are, floats without their fractions, and
/// GL_2_BYTES to GL_4_BYTES as bytes from the highest to the lowest.
fn list_name(bytes: &[u8], kind: u32, size: usize, i: usize) -> u32 {
    let b = &bytes[i * size..][..size];
    match kind {
        0x1400 => b[0] as i8 as u32,
        0x1401 => b[0] as u32,
        0x1402 => i16::from_le_bytes([b[0], b[1]]) as u32,
        0x1403 => u16::from_le_bytes([b[0], b[1]]) as u32,
        0x1404 | 0x1405 => u32::from_le_bytes([b[0], b[1], b[2], b[3]]),
        0x1406 => f32::from_le_bytes([b[0], b[1], b[2], b[3]]) as i64 as u32,
        _ => b.iter().fold(0, |v, &b| v << 8 | b as u32),
    }
}

/// glCallLists(n, type, lists): the lists whose names are the list
/// base plus each of the `n` values at `lists`, in order.
pub fn call_lists(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [n, kind, at] = [args[0], args[1], args[2]];
    if (n as i32) < 0 {
        error(cpu, GL_INVALID_VALUE);
        return Some(0);
    }
    let Some(size) = name_size(kind) else {
        error(cpu, GL_INVALID_ENUM);
        return Some(0);
    };
    // All of them are read before the first runs: in a replay they are
    // in the context's block, which the lists they name write over.
    let mut bytes = Vec::new();
    if at == 0 || !read(cpu, at, n as usize * size, &mut bytes) {
        return Some(0);
    }
    for i in 0..n as usize {
        let Some(base) = ctx(cpu).map(|c| c.lists.base) else {
            break;
        };
        execute(cpu, base.wrapping_add(list_name(&bytes, kind, size, i)));
    }
    Some(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn desc(name: &str) -> (Desc, usize) {
        let at = place_of(name).expect(name);
        (descs()[at], APIS[at].args as usize)
    }

    #[test]
    fn every_export_is_described_within_its_arguments() {
        for (api, desc) in APIS.iter().zip(descs()) {
            // A replay has the arguments in an array of sixteen.
            assert!(api.args <= 16, "{}", api.name);
            for p in desc.pointers.iter().flatten() {
                assert!(p.arg < api.args, "{}", api.name);
                assert_eq!(desc.when, When::Compiled, "{}", api.name);
            }
            // A function named as a vector takes a pointer, whose data
            // a list has to copy.
            let name = api.name.strip_suffix("ARB").unwrap_or(api.name);
            if name.ends_with('v') && desc.when != When::Now {
                assert!(desc.pointers[0].is_some(), "{}", api.name);
            }
        }
    }

    #[test]
    fn pointers_have_the_sizes_of_their_calls() {
        let pointer = |name: &str| desc(name).0.pointers.map(|p| p.map(|p| (p.arg, p.size)));
        assert_eq!(pointer("glVertex3fv"), [Some((0, Size::Bytes(12))), None]);
        assert_eq!(pointer("glColor4ubv"), [Some((0, Size::Bytes(4))), None]);
        assert_eq!(pointer("glColor3usv"), [Some((0, Size::Bytes(6))), None]);
        assert_eq!(pointer("glNormal3dv"), [Some((0, Size::Bytes(24))), None]);
        assert_eq!(pointer("glIndexubv"), [Some((0, Size::Bytes(1))), None]);
        assert_eq!(
            pointer("glMultiTexCoord2svARB"),
            [Some((1, Size::Bytes(4))), None]
        );
        assert_eq!(pointer("glMaterialfv"), [Some((2, Size::Pname(4))), None]);
        assert_eq!(pointer("glFogiv"), [Some((1, Size::Pname(4))), None]);
        assert_eq!(pointer("glTexGendv"), [Some((2, Size::Pname(8))), None]);
        assert_eq!(
            pointer("glLoadMatrixd"),
            [Some((0, Size::Bytes(128))), None]
        );
        assert_eq!(
            pointer("glRectsv"),
            [Some((0, Size::Bytes(4))), Some((1, Size::Bytes(4)))]
        );
        assert_eq!(pointer("glVertex3f"), [None, None]);
        assert_eq!(pointer("glBindTexture"), [None, None]);
    }

    /// A list of the calls `calls`: each an export's name, its argument
    /// words, and what its pointer points at.
    fn list_of(calls: &[(&str, &[u32], Option<&[u8]>)]) -> List {
        let mut list = List::default();
        for &(name, args, bytes) in calls {
            let at = place_of(name).expect(name);
            let desc = descs()[at];
            let mut call = Call {
                api: at as u16,
                copied: 0,
                args: list.words.len() as u32,
            };
            list.words.extend_from_slice(args);
            if let Some(bytes) = bytes {
                let arg = desc.pointers[0].unwrap().arg as usize;
                list.words[call.args as usize + arg] = list.data.len() as u32;
                call.copied = 1 << arg;
                list.data
                    .extend_from_slice(&(bytes.len() as u32).to_le_bytes());
                list.data.extend_from_slice(bytes);
            }
            list.calls.push(call);
        }
        list
    }

    fn bits(values: &[f32]) -> Vec<u32> {
        values.iter().map(|v| v.to_bits()).collect()
    }

    /// A glBegin to glEnd run of vertices is kept as the vertices, each
    /// with the values in force when it was given: the first vertex
    /// came before the run's color and takes the current one, none has
    /// a normal of the run's, and the second unit's coordinates are
    /// given from the start.
    #[test]
    fn a_run_of_vertices_is_kept_as_a_batch() {
        const GL_TEXTURE1: u32 = GL_TEXTURE0 + 1;
        let green = [0u8, 255, 0];
        let corner: Vec<u8> = [3.0f32, 4.0, 5.0]
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect();
        let st = [GL_TEXTURE1, 0.5f32.to_bits(), 0.25f32.to_bits()];
        let list = list_of(&[
            ("glMatrixMode", &[0x1700], None),
            ("glBegin", &[4], None),
            ("glMultiTexCoord2fARB", &st, None),
            ("glVertex2f", &bits(&[1.0, 2.0]), None),
            ("glColor3ubv", &[0], Some(&green)),
            ("glVertex3fv", &[0], Some(&corner)),
            ("glVertex2i", &[7, 8], None),
            ("glEnd", &[], None),
            ("glLoadIdentity", &[], None),
        ]);
        let [batch] = list.batches() else {
            panic!("one run: {:?}", list.batches());
        };
        assert_eq!((batch.first, batch.calls, batch.kind), (1, 7, 4));
        let positions: Vec<[f32; 4]> = batch.vertices.iter().map(|v| v.position).collect();
        assert_eq!(
            positions,
            [
                [1.0, 2.0, 0.0, 1.0],
                [3.0, 4.0, 5.0, 1.0],
                [7.0, 8.0, 0.0, 1.0]
            ]
        );
        let green = [0.0, 1.0, 0.0, 1.0];
        assert_eq!(batch.vertices[1].color, green);
        assert_eq!(batch.vertices[2].color, green);
        assert!(batch.vertices.iter().all(|v| v.tex[1] == [0.5, 0.25, 0.0, 1.0]));
        let inherit = draw::Inherit {
            color: 1,
            normal: 3,
            tex: [3, 0],
        };
        assert_eq!(batch.inherit, inherit);
        let last = Last {
            color: Some(green),
            normal: None,
            tex: [None, Some([0.5, 0.25, 0.0, 1.0])],
        };
        assert_eq!(batch.last, last);
    }

    /// A run stays calls when a batch couldn't stand for it: with a call
    /// between glBegin and glEnd that gives no vertex's value, without
    /// its glEnd, for a primitive or a texture unit there isn't, and
    /// with an array the list has no copy of.
    #[test]
    fn a_run_a_batch_cannot_stand_for_stays_calls() {
        let v = bits(&[1.0, 2.0]);
        let vertex = ("glVertex2f", &v[..], None);
        let begin = ("glBegin", &[7u32][..], None);
        let end = ("glEnd", &[][..], None);
        let none = |calls: &[(&str, &[u32], Option<&[u8]>)]| {
            assert_eq!(list_of(calls).batches(), [], "{calls:?}");
        };
        none(&[begin, vertex, ("glEdgeFlag", &[1], None), end]);
        none(&[begin, vertex, ("glCallList", &[2], None), end]);
        none(&[begin, vertex]);
        none(&[("glBegin", &[10], None), vertex, end]);
        let unit = [GL_TEXTURE0 + UNITS as u32, 0, 0];
        none(&[begin, ("glMultiTexCoord2fARB", &unit, None), vertex, end]);
        none(&[begin, ("glVertex2fv", &[0x1000], None), end]);
        // The run after one that stays calls is a batch.
        let list = list_of(&[begin, vertex, begin, vertex, end]);
        assert_eq!(list.batches().len(), 1);
        assert_eq!(list.batches()[0].first, 2);
    }

    /// A replay draws a run in one step and leaves the current values
    /// its calls would: the color and normal are the run's last, and
    /// the context's block for copies, which replaying glColor3fv
    /// would have asked for, isn't there. Inside a glBegin of the
    /// program's the calls are replayed, and the run's own glBegin is
    /// the error it is there.
    #[test]
    fn a_replay_draws_a_run_in_one_step() {
        let mut cpu = Cpu::new(".".into());
        let tid = super::super::super::user32::current_tid(&cpu);
        let s = state(&mut cpu);
        s.contexts.push(Some(super::super::new_context(1, 2, 3)));
        s.current.push((tid, super::super::hglrc(0), 0));
        let blue: Vec<u8> = [0.0f32, 0.0, 1.0]
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect();
        let corner = bits(&[1.0, 2.0]);
        let up = bits(&[0.0, 1.0, 0.0]);
        let list = list_of(&[
            ("glBegin", &[4], None),
            ("glColor3fv", &[0], Some(&blue)),
            ("glVertex2f", &corner, None),
            ("glNormal3f", &up, None),
            ("glEnd", &[], None),
        ]);
        run(&mut cpu, &list);
        let c = ctx(&mut cpu).unwrap();
        assert_eq!(c.color, [0.0, 0.0, 1.0, 1.0]);
        assert_eq!(c.normal, [0.0, 1.0, 0.0]);
        assert_eq!((c.lists.scratch, c.begin, c.error), (0, None, 0));

        let green = bits(&[0.0, 1.0, 0.0]);
        let list = list_of(&[
            ("glBegin", &[4], None),
            ("glColor3f", &green, None),
            ("glVertex2f", &corner, None),
            ("glEnd", &[], None),
        ]);
        assert_eq!(list.batches().len(), 1);
        c.begin = Some(7);
        run(&mut cpu, &list);
        let c = ctx(&mut cpu).unwrap();
        assert_eq!(c.color, [0.0, 1.0, 0.0, 1.0]);
        assert_eq!((c.begin, c.error), (None, GL_INVALID_OPERATION));
    }

    /// What a replay of a run costs call by call and as a batch, in
    /// nanoseconds for each vertex: a strip of 2000 vertices, each a
    /// glNormal3f and a glVertex3f as a model in a list is given,
    /// unlit and lit by one light. The context has no window, so the
    /// vertices are made and lit and not drawn; the drawing is the
    /// same work both ways. A timing measurement, to run alone:
    /// `cargo test --release --lib a_runs_replay_cost -- --ignored
    /// --nocapture`.
    #[test]
    #[ignore = "a timing measurement, for a machine with nothing else running"]
    fn a_runs_replay_cost() {
        const VERTICES: usize = 2000;
        const REPLAYS: u32 = 500;
        let mut cpu = Cpu::new(".".into());
        let tid = super::super::super::user32::current_tid(&cpu);
        let s = state(&mut cpu);
        s.contexts.push(Some(super::super::new_context(1, 2, 3)));
        s.current.push((tid, super::super::hglrc(0), 0));
        let words: Vec<(Vec<u32>, Vec<u32>)> = (0..VERTICES)
            .map(|i| {
                let turn = i as f32 * 0.01;
                let normal = bits(&[turn.cos(), turn.sin(), 0.0]);
                let vertex = bits(&[turn.cos() * 2.0, turn.sin() * 2.0, (i % 2) as f32]);
                (normal, vertex)
            })
            .collect();
        let mut calls: Vec<(&str, &[u32], Option<&[u8]>)> = vec![("glBegin", &[8], None)];
        for (normal, vertex) in &words {
            calls.push(("glNormal3f", normal, None));
            calls.push(("glVertex3f", vertex, None));
        }
        calls.push(("glEnd", &[], None));
        let batched = list_of(&calls);
        assert_eq!(batched.batches().len(), 1);
        // The same list with no runs kept: every call is replayed.
        let by_call = list_of(&calls);
        by_call.batches.set(Vec::new()).unwrap();
        let time = |cpu: &mut Cpu, list: &List| {
            let start = std::time::Instant::now();
            for _ in 0..REPLAYS {
                run(cpu, list);
            }
            start.elapsed().as_nanos() as f64 / (REPLAYS as f64 * VERTICES as f64)
        };
        for lit in [false, true] {
            if lit {
                let c = ctx(&mut cpu).unwrap();
                c.caps |= super::super::CAP_LIGHTING;
                c.lighting.enabled = 1;
            }
            // Three rounds, the two ways in turn, and the best of each.
            let mut best = [f64::MAX; 2];
            for _ in 0..3 {
                best[0] = best[0].min(time(&mut cpu, &by_call));
                best[1] = best[1].min(time(&mut cpu, &batched));
            }
            let [calls, batch] = best;
            println!(
                "lit {lit}: {calls:.1} ns a vertex call by call, {batch:.1} ns as a batch, \
                 {:.1} times",
                calls / batch
            );
        }
    }

    /// Every export that is one of the vertex functions takes the
    /// arguments its name says.
    #[test]
    fn the_vertex_functions_take_the_arguments_of_their_names() {
        let mut found = 0;
        for api in APIS.iter() {
            if let Some(spec) = imm::spec(api.name) {
                assert_eq!(spec.words(), api.args as usize, "{}", api.name);
                found += 1;
            }
        }
        // glVertex has 24 forms, glNormal 10, and glColor, glTexCoord
        // and glMultiTexCoordARB 32 each.
        assert_eq!(found, 24 + 10 + 3 * 32);
        for name in ["glVertexPointer", "glColorMask", "glNormalPointer", "glColorMaterial"] {
            assert!(imm::spec(name).is_none(), "{name}");
        }
    }

    #[test]
    fn the_calls_opengl_never_compiles_run_at_once() {
        for name in [
            "glGetIntegerv",
            "glGetError",
            "glGetString",
            "glIsEnabled",
            "glIsList",
            "glGenLists",
            "glDeleteLists",
            "glNewList",
            "glEndList",
            "glFinish",
            "glFlush",
            "glReadPixels",
            "glPixelStorei",
            "glGenTextures",
            "glDeleteTextures",
            "glIsTexture",
            "glFeedbackBuffer",
            "glSelectBuffer",
            "glRenderMode",
            "glEnableClientState",
            "glVertexPointer",
            "glEdgeFlagPointer",
            "glInterleavedArrays",
            "wglSwapBuffers",
        ] {
            assert_eq!(desc(name).0.when, When::Now, "{name}");
        }
        for name in [
            "glCallList",
            "glCallLists",
            "glListBase",
            "glBegin",
            "glBindTexture",
            "glTexImage2D",
            "glBitmap",
            "glRasterPos2i",
            "glPrioritizeTextures",
        ] {
            assert_eq!(desc(name).0.when, When::Compiled, "{name}");
        }
        assert_eq!(desc("glDrawElements").0.when, When::Elements);
    }

    /// A bitmap of no columns copies nothing, however many rows it has:
    /// with pixels skipped there is a byte to read, and the rows, zero
    /// bytes apart, would be gone through two billion times.
    #[test]
    fn an_image_with_no_pixels_copies_nothing() {
        let mut cpu = Cpu::new(".".into());
        let mut c = super::super::new_context(1, 2, 3);
        c.unpack = [1, 0, 0, 8];
        let tid = super::super::super::user32::current_tid(&cpu);
        let s = state(&mut cpu);
        s.contexts.push(Some(c));
        s.current.push((tid, super::super::hglrc(0), 0));
        assert!(cpu.bus.guest_probe(0x1000, 1, false));
        let mut out = Vec::new();
        for size in [(0, 0x7FFF_FFFF), (8, 0)] {
            assert!(!copy_image(&mut cpu, 0x1000, size, None, &mut out));
        }
        let rgba = Some((GL_RGBA, GL_UNSIGNED_BYTE));
        assert!(!copy_image(&mut cpu, 0x1000, (0, 4), rgba, &mut out));
        assert!(out.is_empty());
    }

    #[test]
    fn names_come_after_the_last_or_in_a_gap() {
        let mut names = Names::new();
        assert_eq!(free_names(&names, 3), Some(1));
        names.insert(5, Arc::default());
        assert_eq!(free_names(&names, 3), Some(6));
        names.insert(u32::MAX, Arc::default());
        assert_eq!(free_names(&names, 4), Some(1));
        assert_eq!(free_names(&names, 5), Some(6));
    }

    #[test]
    fn glcalllists_names_are_read_by_their_type() {
        assert_eq!(list_name(&[0xFE], 0x1400, 1, 0), -2i32 as u32);
        assert_eq!(list_name(&[0xFE], 0x1401, 1, 0), 0xFE);
        assert_eq!(list_name(&[0, 0, 0x34, 0x12], 0x1403, 2, 1), 0x1234);
        assert_eq!(list_name(&7.9f32.to_le_bytes(), 0x1406, 4, 0), 7);
        assert_eq!(list_name(&[1, 2], 0x1407, 2, 0), 0x102);
        assert_eq!(list_name(&[1, 2, 3], 0x1408, 3, 0), 0x1_0203);
        assert_eq!(list_name(&[1, 2, 3, 4], 0x1409, 4, 0), 0x102_0304);
    }

    #[test]
    fn a_saved_state_has_the_lists() {
        use crate::savestate::{Reader, State as _, Writer};
        let mut state = State::default();
        let mut c = super::super::new_context(1, 2, 3);
        c.lists.base = 32;
        c.lists.open = Some(Open {
            name: 9,
            mode: GL_COMPILE,
            begun: true,
            list: List::default(),
        });
        state.contexts.push(Some(c));
        let list = List {
            calls: vec![Call {
                api: 4,
                copied: 1,
                args: 0,
            }],
            words: vec![0],
            data: vec![2, 0, 0, 0, 7, 8],
            ..List::default()
        };
        names_mut(&mut state, 0)
            .unwrap()
            .insert(3, Arc::new(list.clone()));
        sweep(&mut state);
        let mut w = Writer::default();
        state.save(&mut w);
        let mut loaded = State::default();
        loaded.load(&mut Reader::new(&w.buf)).unwrap();
        assert_eq!(loaded.lists.compiling, 1);
        assert_eq!(
            names(&loaded, 0).unwrap().get(&3).map(|l| &**l),
            Some(&list)
        );
        let own = &loaded.contexts[0].as_ref().unwrap().lists;
        assert_eq!((own.base, own.get(GL_LIST_INDEX)), (32, Some(9.0)));
        assert_eq!(copied(&list, 0), [7, 8]);
    }
}
