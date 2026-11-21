//! OLE2DISP, 16-bit OLE automation: BSTRs, VARIANTs and their changes
//! between types, safe arrays, and the dispatch helpers. The ordinals
//! and names are Windows 98 SE's, as Wine's ole2disp.dll16 spec file has
//! them; the type libraries are TYPELIB's (typelib.rs).
//!
//! The 16-bit structures differ from the 32-bit ones. A BSTR is a far
//! pointer to ANSI characters, the dword before them their length in
//! bytes, a NUL after them. A VARIANT is 16 bytes, its VARTYPE at 0 and
//! its value at 8, with 16:16 pointers. A SAFEARRAY is cDims, fFeatures,
//! cbElements and cLocks (words), a dword handle, the 16:16 pointer to
//! the data, and the bounds, the last dimension's first. The changes
//! between types are those of the built-in OLEAUT32
//! (win32/oleaut32/variant.rs), the 16-bit values read into its and its
//! results written back.
//!
//! BSTRs and safe arrays take their blocks from the task allocator, the
//! one CoGetMalloc(MEMCTX_TASK) gives, as the OLE 2 Programmer's
//! Reference has them: a program may free a BSTR or an array's data with
//! IMalloc::Free, and ask its allocator's GetSize or DidAlloc about one.
//! That is the program's own allocator when it gave CoInitialize or
//! OleInitialize one, as Visual Basic 4's runtime does, and the built-in
//! one of ole2.rs otherwise (before CoInitialize too, where the real
//! OLE2DISP has no allocator).
//!
//! The program's allocator is 16-bit code, and no Rust frame lives
//! across a call of 16-bit code (thunk.rs). A function that makes blocks
//! (`Op`) runs again from its start each time the program's Alloc or
//! Realloc returned, with the blocks those calls gave so far (`Heap`),
//! until it needs no more; the frees and the interfaces' AddRef and
//! Release calls it asked for are made after it ends, in order. So a
//! function writes nothing the program can see before its last Alloc,
//! except into the blocks it made, and reads nothing a free changes. A
//! block goes to the allocator CoGetMalloc gives when it is freed, as
//! the real OLE2DISP's do, whichever made it.
//!
//! Copyright 1998 Jean-Claude Cote
//! Copyright 2003 Jon Griffiths
//! Copyright 2005 Daniel Remenak

use super::builtin::{Def, cdecl, missing, rust, variable};
use super::ole2::{self, call_all, call_method, guid};
use super::thunk;
use crate::cpu::Cpu;
use crate::dpmi::Context;
use crate::win32::com::{E_INVALIDARG, E_NOINTERFACE, E_OUTOFMEMORY, S_FALSE, S_OK};
use crate::win32::oleaut32::variant::{Out, Val, change, format_g};

const VT_EMPTY: u16 = 0;
const VT_NULL: u16 = 1;
const VT_I2: u16 = 2;
const VT_I4: u16 = 3;
const VT_R4: u16 = 4;
const VT_R8: u16 = 5;
const VT_CY: u16 = 6;
const VT_DATE: u16 = 7;
const VT_BSTR: u16 = 8;
const VT_DISPATCH: u16 = 9;
const VT_ERROR: u16 = 10;
const VT_BOOL: u16 = 11;
const VT_VARIANT: u16 = 12;
const VT_UNKNOWN: u16 = 13;
const VT_UI1: u16 = 17;
const VT_ARRAY: u16 = 0x2000;
const VT_BYREF: u16 = 0x4000;

const DISP_E_TYPEMISMATCH: u32 = 0x8002_0005;
const DISP_E_BADVARTYPE: u32 = 0x8002_0008;
const DISP_E_BADINDEX: u32 = 0x8002_000B;
const DISP_E_ARRAYISLOCKED: u32 = 0x8002_000D;
const E_UNEXPECTED: u32 = 0x8000_FFFF;

const FADF_BSTR: u16 = 0x0100;
const FADF_UNKNOWN: u16 = 0x0200;
const FADF_DISPATCH: u16 = 0x0400;
const FADF_VARIANT: u16 = 0x0800;

/// A SAFEARRAY's bytes before its bounds, and each bound's.
const SA_HEADER: u32 = 16;
const SA_BOUND: u32 = 8;

const IID_IERRORINFO: [u8; 16] = guid(
    0x1CF2_B120,
    0x547D,
    0x101B,
    [0x8E, 0x65, 0x08, 0x00, 0x2B, 0x2B, 0xD1, 0x19],
);
const IID_ICREATEERRORINFO: [u8; 16] = guid(
    0x22F0_3340,
    0x547D,
    0x101B,
    [0x8E, 0x65, 0x08, 0x00, 0x2B, 0x2B, 0xD1, 0x19],
);

pub static OLE2DISP: Def = Def {
    name: "OLE2DISP",
    file: "OLE2DISP.DLL",
    apis: &[
        missing("DllGetClassObject", 1),
        rust("SysAllocString", 2, "p", with_heap),
        rust("SysReAllocString", 3, "pp", with_heap),
        rust("SysAllocStringLen", 4, "pw", with_heap),
        rust("SysReAllocStringLen", 5, "ppw", with_heap),
        rust("SysFreeString", 6, "l", with_heap),
        rust("SysStringLen", 7, "l", sys_string_len),
        rust("VariantInit", 8, "p", variant_init),
        rust("VariantClear", 9, "p", with_heap),
        rust("VariantCopy", 10, "pp", with_heap),
        rust("VariantCopyInd", 11, "pp", with_heap),
        rust("VariantChangeType", 12, "ppww", with_heap),
        missing("VariantTimeToDosDateTime", 13),
        missing("DosDateTimeToVariantTime", 14),
        rust("SafeArrayCreate", 15, "wwp", with_heap),
        rust("SafeArrayDestroy", 16, "l", with_heap),
        rust("SafeArrayGetDim", 17, "l", safe_array_get_dim),
        rust("SafeArrayGetElemsize", 18, "l", safe_array_get_elemsize),
        rust("SafeArrayGetUBound", 19, "lwp", safe_array_get_ubound),
        rust("SafeArrayGetLBound", 20, "lwp", safe_array_get_lbound),
        rust("SafeArrayLock", 21, "l", safe_array_lock),
        rust("SafeArrayUnlock", 22, "l", safe_array_unlock),
        rust("SafeArrayAccessData", 23, "lp", safe_array_access_data),
        rust("SafeArrayUnaccessData", 24, "l", safe_array_unlock),
        rust("SafeArrayGetElement", 25, "lpl", with_heap),
        rust("SafeArrayPutElement", 26, "lpl", with_heap),
        rust("SafeArrayCopy", 27, "lp", with_heap),
        missing("DispGetParam", 28),
        rust("DispGetIDsOfNames", 29, "llwl", disp_get_ids_of_names),
        rust("DispInvoke", 30, "lllwllll", disp_invoke),
        missing("CreateDispTypeInfo", 31),
        missing("CreateStdDispatch", 32),
        variable("_IID_IDispatch", 33, &ole2::ole_guid(0x0002_0400)),
        variable("_IID_IEnumVARIANT", 34, &ole2::ole_guid(0x0002_0404)),
        missing("RegisterActiveObject", 35),
        missing("RevokeActiveObject", 36),
        missing("GetActiveObject", 37),
        rust("SafeArrayAllocDescriptor", 38, "wp", with_heap),
        rust("SafeArrayAllocData", 39, "l", with_heap),
        rust("SafeArrayDestroyDescriptor", 40, "l", with_heap),
        rust("SafeArrayDestroyData", 41, "l", with_heap),
        rust("SafeArrayRedim", 42, "lp", with_heap),
        rust("VarI2FromI4", 43, "lp", with_heap),
        rust("VarI2FromR4", 44, "lp", with_heap),
        rust("VarI2FromR8", 45, "llp", with_heap),
        rust("VarI2FromCy", 46, "llp", with_heap),
        rust("VarI2FromDate", 47, "llp", with_heap),
        rust("VarI2FromStr", 48, "pllp", with_heap),
        missing("VarI2FromDisp", 49),
        rust("VarI2FromBool", 50, "wp", with_heap),
        rust("VarI4FromI2", 51, "wp", with_heap),
        rust("VarI4FromR4", 52, "lp", with_heap),
        rust("VarI4FromR8", 53, "llp", with_heap),
        rust("VarI4FromCy", 54, "llp", with_heap),
        rust("VarI4FromDate", 55, "llp", with_heap),
        rust("VarI4FromStr", 56, "pllp", with_heap),
        missing("VarI4FromDisp", 57),
        rust("VarI4FromBool", 58, "wp", with_heap),
        rust("VarR4FromI2", 59, "wp", with_heap),
        rust("VarR4FromI4", 60, "lp", with_heap),
        rust("VarR4FromR8", 61, "llp", with_heap),
        rust("VarR4FromCy", 62, "llp", with_heap),
        rust("VarR4FromDate", 63, "llp", with_heap),
        rust("VarR4FromStr", 64, "pllp", with_heap),
        missing("VarR4FromDisp", 65),
        rust("VarR4FromBool", 66, "wp", with_heap),
        rust("VarR8FromI2", 67, "wp", with_heap),
        rust("VarR8FromI4", 68, "lp", with_heap),
        rust("VarR8FromR4", 69, "lp", with_heap),
        rust("VarR8FromCy", 70, "llp", with_heap),
        rust("VarR8FromDate", 71, "llp", with_heap),
        rust("VarR8FromStr", 72, "pllp", with_heap),
        missing("VarR8FromDisp", 73),
        rust("VarR8FromBool", 74, "wp", with_heap),
        rust("VarDateFromI2", 75, "wp", with_heap),
        rust("VarDateFromI4", 76, "lp", with_heap),
        rust("VarDateFromR4", 77, "lp", with_heap),
        rust("VarDateFromR8", 78, "llp", with_heap),
        rust("VarDateFromCy", 79, "llp", with_heap),
        rust("VarDateFromStr", 80, "pllp", with_heap),
        missing("VarDateFromDisp", 81),
        rust("VarDateFromBool", 82, "wp", with_heap),
        rust("VarCyFromI2", 83, "wp", with_heap),
        rust("VarCyFromI4", 84, "lp", with_heap),
        rust("VarCyFromR4", 85, "lp", with_heap),
        rust("VarCyFromR8", 86, "llp", with_heap),
        rust("VarCyFromDate", 87, "llp", with_heap),
        rust("VarCyFromStr", 88, "pllp", with_heap),
        missing("VarCyFromDisp", 89),
        rust("VarCyFromBool", 90, "wp", with_heap),
        rust("VarBstrFromI2", 91, "wllp", with_heap),
        rust("VarBstrFromI4", 92, "lllp", with_heap),
        rust("VarBstrFromR4", 93, "lllp", with_heap),
        rust("VarBstrFromR8", 94, "llllp", with_heap),
        rust("VarBstrFromCy", 95, "llllp", with_heap),
        rust("VarBstrFromDate", 96, "llllp", with_heap),
        missing("VarBstrFromDisp", 97),
        rust("VarBstrFromBool", 98, "wllp", with_heap),
        rust("VarBoolFromI2", 99, "wp", with_heap),
        rust("VarBoolFromI4", 100, "lp", with_heap),
        rust("VarBoolFromR4", 101, "lp", with_heap),
        rust("VarBoolFromR8", 102, "llp", with_heap),
        rust("VarBoolFromDate", 103, "llp", with_heap),
        rust("VarBoolFromCy", 104, "llp", with_heap),
        rust("VarBoolFromStr", 105, "pllp", with_heap),
        missing("VarBoolFromDisp", 106),
        missing("DoInvokeMethod", 107),
        rust("VariantChangeTypeEx", 108, "pplww", with_heap),
        rust("SafeArrayPtrOfIndex", 109, "lpp", safe_array_ptr_of_index),
        rust("SetErrorInfo", 110, "ll", set_error_info),
        rust("GetErrorInfo", 111, "lp", get_error_info),
        rust("CreateErrorInfo", 112, "p", create_error_info),
        variable("_IID_IErrorInfo", 113, &IID_IERRORINFO),
        variable("_IID_ICreateErrorInfo", 114, &IID_ICREATEERRORINFO),
        variable(
            "_IID_ISupportErrorInfo",
            115,
            &guid(
                0xDF0B_3D60,
                0x548F,
                0x101B,
                [0x8E, 0x65, 0x08, 0x00, 0x2B, 0x2B, 0xD1, 0x19],
            ),
        ),
        rust("VarUI1FromI2", 116, "wp", with_heap),
        rust("VarUI1FromI4", 117, "lp", with_heap),
        rust("VarUI1FromR4", 118, "lp", with_heap),
        rust("VarUI1FromR8", 119, "llp", with_heap),
        rust("VarUI1FromCy", 120, "llp", with_heap),
        rust("VarUI1FromDate", 121, "llp", with_heap),
        rust("VarUI1FromStr", 122, "pllp", with_heap),
        missing("VarUI1FromDisp", 123),
        rust("VarUI1FromBool", 124, "wp", with_heap),
        rust("VarI2FromUI1", 125, "wp", with_heap),
        rust("VarI4FromUI1", 126, "wp", with_heap),
        rust("VarR4FromUI1", 127, "wp", with_heap),
        rust("VarR8FromUI1", 128, "wp", with_heap),
        rust("VarDateFromUI1", 129, "wp", with_heap),
        rust("VarCyFromUI1", 130, "wp", with_heap),
        rust("VarBstrFromUI1", 131, "wllp", with_heap),
        rust("VarBoolFromUI1", 132, "wp", with_heap),
        // No object of the program's can be unloaded while it runs.
        rust("DllCanUnloadNow", 133, "", s_false),
        // The error object's methods, which aren't exported.
        cdecl(
            "ICreateErrorInfo_QueryInterface",
            0,
            "lpp",
            cei_query_interface,
        ),
        cdecl("ICreateErrorInfo_AddRef", 0, "l", cei_add_ref),
        cdecl("ICreateErrorInfo_Release", 0, "l", cei_release),
        cdecl("ICreateErrorInfo_SetGUID", 0, "lp", cei_set_guid),
        cdecl("ICreateErrorInfo_SetSource", 0, "lp", cei_set_source),
        cdecl(
            "ICreateErrorInfo_SetDescription",
            0,
            "lp",
            cei_set_description,
        ),
        cdecl("ICreateErrorInfo_SetHelpFile", 0, "lp", cei_set_help_file),
        cdecl(
            "ICreateErrorInfo_SetHelpContext",
            0,
            "ll",
            cei_set_help_context,
        ),
        cdecl("IErrorInfo_QueryInterface", 0, "lpp", ei_query_interface),
        cdecl("IErrorInfo_AddRef", 0, "l", ei_add_ref),
        cdecl("IErrorInfo_Release", 0, "l", ei_release),
        cdecl("IErrorInfo_GetGUID", 0, "lp", ei_get_guid),
        cdecl("IErrorInfo_GetSource", 0, "lp", ei_get_source),
        cdecl("IErrorInfo_GetDescription", 0, "lp", ei_get_description),
        cdecl("IErrorInfo_GetHelpFile", 0, "lp", ei_get_help_file),
        cdecl("IErrorInfo_GetHelpContext", 0, "lp", ei_get_help_context),
    ],
    library: true,
};

fn s_false(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(S_FALSE)
}

// The task allocator.

/// IMalloc's methods, by their place in the vtable.
const IMALLOC_ALLOC: u32 = 3;
const IMALLOC_REALLOC: u32 = 4;
const IMALLOC_FREE: u32 = 5;
/// IUnknown's.
const ADD_REF: u32 = 1;
const RELEASE: u32 = 2;

/// Why a function's run stopped before its end.
enum Stop {
    /// To call the program's Alloc or Realloc: it runs again when that
    /// returns (`Heap`).
    Pending,
    /// It failed with this HRESULT: the blocks it asked to free are
    /// freed, but the AddRefs and Releases it asked for aren't made.
    Fail(u32),
}

/// What a function that makes or frees blocks does: its value, or why
/// it stopped.
type Op = fn(&mut Cpu, &[u32], &mut Heap) -> Result<u32, Stop>;

/// A call to make after a function ends: `method` of `object`, with the
/// block `arg` for IMalloc::Free.
#[derive(Clone, Copy)]
struct Call {
    object: u32,
    method: u32,
    arg: u32,
}

/// The task allocator, as one run of a function sees it.
struct Heap {
    /// The name of the function that runs.
    name: &'static str,
    /// The program's own allocator, or 0 for the built-in one.
    app: u32,
    /// What the program's Alloc and Realloc returned to the runs before,
    /// in order, and how many of them this run took.
    got: Vec<u32>,
    taken: usize,
    /// The method and arguments of the call this run stopped for.
    ask: (u32, Vec<u32>),
    /// The frees, AddRefs and Releases to make when it ends.
    calls: Vec<Call>,
}

impl Heap {
    /// The built-in allocator's, for `alloc_bstr` and `free_bstr`.
    fn builtin() -> Heap {
        Heap {
            name: "",
            app: 0,
            got: Vec::new(),
            taken: 0,
            ask: (0, Vec::new()),
            calls: Vec::new(),
        }
    }

    /// What the program's `method` with `args` returned, or `Pending`
    /// for the run to stop and call it.
    fn answer(&mut self, method: u32, args: Vec<u32>) -> Result<u32, Stop> {
        if let Some(&value) = self.got.get(self.taken) {
            self.taken += 1;
            return Ok(value);
        }
        self.ask = (method, args);
        Err(Stop::Pending)
    }

    /// Whether the program's allocator has answered the next Alloc or
    /// Realloc in a run before: this run then goes past it.
    fn answered(&self) -> bool {
        self.app != 0 && self.taken < self.got.len()
    }

    /// A block of `size` bytes: its 16:16 address, or 0.
    fn alloc(&mut self, cpu: &mut Cpu, size: u32) -> Result<u32, Stop> {
        if self.app == 0 {
            return Ok(ole2::alloc(cpu, size));
        }
        self.answer(IMALLOC_ALLOC, vec![size])
    }

    /// Free the block at `p`: now with the built-in allocator, when the
    /// function ends with the program's.
    fn free(&mut self, cpu: &mut Cpu, p: u32) {
        if p == 0 {
            return;
        }
        if self.app == 0 {
            ole2::free(cpu, p);
            return;
        }
        self.calls.push(Call {
            object: self.app,
            method: IMALLOC_FREE,
            arg: p,
        });
    }

    /// The block at `p` with `size` bytes, its contents kept: where it
    /// is now, or 0 (and the block as it was).
    fn realloc(&mut self, cpu: &mut Cpu, p: u32, size: u32) -> Result<u32, Stop> {
        if self.app == 0 {
            return Ok(ole2::realloc(cpu, p, size));
        }
        self.answer(IMALLOC_REALLOC, vec![p, size])
    }

    /// Call AddRef or Release of the interface `object` when the function
    /// ends.
    fn method(&mut self, object: u32, method: u32) {
        self.calls.push(Call {
            object,
            method,
            arg: 0,
        });
    }
}

/// The function of OLE2DISP's called `name`, of those that make or free
/// blocks.
fn op_named(name: &str) -> Option<Op> {
    Some(match name {
        "SysAllocString" => sys_alloc_string,
        "SysReAllocString" => sys_realloc_string,
        "SysAllocStringLen" => sys_alloc_string_len,
        "SysReAllocStringLen" => sys_realloc_string_len,
        "SysFreeString" => sys_free_string,
        "VariantClear" => variant_clear,
        "VariantCopy" => variant_copy,
        "VariantCopyInd" => variant_copy_ind,
        "VariantChangeType" => variant_change_type,
        "VariantChangeTypeEx" => variant_change_type_ex,
        "SafeArrayCreate" => safe_array_create,
        "SafeArrayDestroy" => safe_array_destroy,
        "SafeArrayAllocDescriptor" => safe_array_alloc_descriptor,
        "SafeArrayAllocData" => safe_array_alloc_data,
        "SafeArrayDestroyDescriptor" => safe_array_destroy_descriptor,
        "SafeArrayDestroyData" => safe_array_destroy_data,
        "SafeArrayRedim" => safe_array_redim,
        "SafeArrayGetElement" => safe_array_get_element,
        "SafeArrayPutElement" => safe_array_put_element,
        "SafeArrayCopy" => safe_array_copy,
        n if n.starts_with("Var") && n.contains("From") => var_from,
        _ => return None,
    })
}

/// What `data` of a THEN_OLE2DISP frame starts with: a run stopped for
/// the program's Alloc or Realloc, or the calls after a function ended.
const THEN_RUN: u32 = 0;
const THEN_CALLS: u32 = 1;

/// The stub of a function that makes or frees blocks: run it with the
/// task allocator.
fn with_heap(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let name = current_function(cpu);
    let ordinal = OLE2DISP.apis.iter().find(|a| a.name == name)?.ordinal;
    let mut ctx = Context::of(cpu);
    run(cpu, ordinal, args, Vec::new(), &mut ctx, true)
}

/// Run the function with the ordinal `ordinal` and the arguments `args`,
/// `got` what the program's allocator returned to its runs before, for
/// the caller that goes on as `ctx`. `first` when it is the stub's run,
/// whose value the stub returns; else it goes to `ctx`'s DX:AX.
fn run(
    cpu: &mut Cpu,
    ordinal: u16,
    args: &[u32],
    got: Vec<u32>,
    ctx: &mut Context,
    first: bool,
) -> Option<u32> {
    let name = OLE2DISP.apis.iter().find(|a| a.ordinal == ordinal)?.name;
    let op = op_named(name)?;
    let app = ole2::this_task_or_new(cpu).app_malloc;
    let mut heap = Heap {
        name,
        app,
        got,
        ..Heap::builtin()
    };
    match op(cpu, args, &mut heap) {
        Ok(value) => finish(cpu, value, &heap.calls, ctx, first),
        Err(Stop::Fail(e)) => {
            heap.calls.retain(|c| c.method == IMALLOC_FREE);
            finish(cpu, e, &heap.calls, ctx, first)
        }
        Err(Stop::Pending) => {
            let mut data = vec![THEN_RUN, ordinal as u32, args.len() as u32];
            data.extend_from_slice(args);
            data.extend_from_slice(&heap.got);
            let (method, margs) = std::mem::take(&mut heap.ask);
            call_program(cpu, *ctx, app, method, &margs, data);
            None
        }
    }
}

/// The function ended with `value`: make `calls` in turn, then return
/// `value`.
fn finish(
    cpu: &mut Cpu,
    value: u32,
    calls: &[Call],
    ctx: &mut Context,
    first: bool,
) -> Option<u32> {
    for (i, c) in calls.iter().enumerate() {
        if ole2::method_address(cpu, c.object, c.method) == 0 {
            continue;
        }
        let mut data = vec![THEN_CALLS, value];
        for rest in &calls[i + 1..] {
            data.extend([rest.object, rest.method, rest.arg]);
        }
        let args: &[u32] = if c.method == IMALLOC_FREE {
            &[c.arg]
        } else {
            &[]
        };
        call_program(cpu, *ctx, c.object, c.method, args, data);
        return None;
    }
    if first {
        Some(value)
    } else {
        ole2::dx_ax(ctx, value)
    }
}

/// Call the method `method` of the program's 16-bit interface `object`
/// with the dwords `args`, as C calls it; `then` gets what it returned,
/// and `data`, and the caller goes on as `resume`.
fn call_program(
    cpu: &mut Cpu,
    resume: Context,
    object: u32,
    method: u32,
    args: &[u32],
    data: Vec<u32>,
) {
    let target = ole2::method_address(cpu, object, method);
    // C's order: the last argument pushed first, a dword's high word
    // before its low one.
    let mut words = Vec::new();
    for &a in args.iter().rev() {
        words.push((a >> 16) as u16);
        words.push(a as u16);
    }
    words.push((object >> 16) as u16);
    words.push(object as u16);
    thunk::call16(cpu, target, &words, 0, resume, thunk::THEN_OLE2DISP, data);
}

/// A method of the program's that `run` or `finish` called returned
/// `value`: the function's next run, or the next of its calls.
pub(super) fn then(cpu: &mut Cpu, value: u32, data: &[u32], ctx: &mut Context) -> Option<u32> {
    if data[0] == THEN_CALLS {
        let (chunks, _) = data[2..].as_chunks::<3>();
        let calls: Vec<Call> = chunks
            .iter()
            .map(|&[object, method, arg]| Call {
                object,
                method,
                arg,
            })
            .collect();
        return finish(cpu, data[1], &calls, ctx, false);
    }
    let count = data[2] as usize;
    let args = &data[3..3 + count];
    let mut got = data[3 + count..].to_vec();
    got.push(value);
    run(cpu, data[1] as u16, args, got, ctx, false)
}

// BSTRs.

/// A BSTR of the bytes `text`: its 16:16 address, or 0.
fn new_bstr(cpu: &mut Cpu, heap: &mut Heap, text: &[u8]) -> Result<u32, Stop> {
    let len = text.len() as u32;
    let block = heap.alloc(cpu, len + 5)?;
    if block == 0 {
        return Ok(0);
    }
    let at = super::linear(cpu, block);
    let mut bytes = len.to_le_bytes().to_vec();
    bytes.extend_from_slice(text);
    bytes.push(0);
    super::poke(cpu, at, &bytes);
    Ok(block + 4)
}

/// A BSTR of the bytes `text` from the built-in allocator: its 16:16
/// address, or 0. For TYPELIB (#1598) and the error object (#1601),
/// whose functions don't run with a `Heap` yet (#1625).
#[allow(dead_code)]
pub fn alloc_bstr(cpu: &mut Cpu, text: &[u8]) -> u32 {
    new_bstr(cpu, &mut Heap::builtin(), text).unwrap_or(0)
}

/// The bytes of the BSTR `bstr` (16:16), its length as the dword before
/// it says.
pub fn bstr_bytes(cpu: &Cpu, bstr: u32) -> Vec<u8> {
    let at = super::linear(cpu, bstr);
    if at == 0 || (bstr as u16) < 4 {
        return Vec::new();
    }
    let len = super::read_32(cpu, at - 4).min(0xFFFF);
    super::peek_upto(cpu, at, len as usize)
}

/// Free the BSTR `bstr` of the built-in allocator's. For the error
/// object (#1601) and TYPELIB, as `alloc_bstr`.
#[allow(dead_code)]
pub(super) fn free_bstr(cpu: &mut Cpu, bstr: u32) {
    drop_bstr(cpu, &mut Heap::builtin(), bstr);
}

/// Free the BSTR `bstr` when the function ends.
fn drop_bstr(cpu: &mut Cpu, heap: &mut Heap, bstr: u32) {
    if bstr != 0 && bstr as u16 >= 4 {
        heap.free(cpu, bstr - 4);
    }
}

/// SysAllocString(sz): a BSTR of the string, NULL for none.
fn sys_alloc_string(cpu: &mut Cpu, args: &[u32], heap: &mut Heap) -> Result<u32, Stop> {
    if args[0] == 0 {
        return Ok(0);
    }
    let text = super::string_bytes(cpu, args[0], 0xFFF0);
    new_bstr(cpu, heap, &text)
}

/// SysAllocStringLen(pch, cch): a BSTR of `cch` bytes from `pch`, of
/// zeros when it is NULL.
fn sys_alloc_string_len(cpu: &mut Cpu, args: &[u32], heap: &mut Heap) -> Result<u32, Stop> {
    let len = args[1] as u16 as usize;
    let mut text = if args[0] == 0 {
        Vec::new()
    } else {
        super::peek_upto(cpu, args[0], len)
    };
    text.resize(len, 0);
    new_bstr(cpu, heap, &text)
}

/// Put a new BSTR of `text` at `pbstr`, freeing the one there: TRUE, or
/// FALSE (and the old one kept) when there is no memory.
fn replace_bstr(cpu: &mut Cpu, heap: &mut Heap, pbstr: u32, text: &[u8]) -> Result<u32, Stop> {
    if pbstr == 0 {
        return Ok(0);
    }
    let new = new_bstr(cpu, heap, text)?;
    if new == 0 {
        return Ok(0);
    }
    let old = super::read_32(cpu, pbstr);
    drop_bstr(cpu, heap, old);
    super::write_32(cpu, pbstr, new);
    Ok(1)
}

fn sys_realloc_string(cpu: &mut Cpu, args: &[u32], heap: &mut Heap) -> Result<u32, Stop> {
    let text = super::string_bytes(cpu, args[1], 0xFFF0);
    replace_bstr(cpu, heap, args[0], &text)
}

fn sys_realloc_string_len(cpu: &mut Cpu, args: &[u32], heap: &mut Heap) -> Result<u32, Stop> {
    let len = args[2] as u16 as usize;
    let mut text = if args[1] == 0 {
        Vec::new()
    } else {
        super::peek_upto(cpu, args[1], len)
    };
    text.resize(len, 0);
    replace_bstr(cpu, heap, args[0], &text)
}

fn sys_free_string(cpu: &mut Cpu, args: &[u32], heap: &mut Heap) -> Result<u32, Stop> {
    drop_bstr(cpu, heap, args[0]);
    Ok(0)
}

fn sys_string_len(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if args[0] == 0 {
        return Some(0);
    }
    let at = super::linear(cpu, args[0]);
    Some(super::read_32(cpu, at.wrapping_sub(4)) & 0xFFFF)
}

/// ANSI text as the characters of OLEAUT32's strings, a byte each.
fn units(bytes: &[u8]) -> Vec<u16> {
    bytes.iter().map(|&b| b as u16).collect()
}

/// OLEAUT32's characters as ANSI text: "?" for one that isn't a byte.
pub(super) fn ansi(units: &[u16]) -> Vec<u8> {
    units
        .iter()
        .map(|&u| u8::try_from(u).unwrap_or(b'?'))
        .collect()
}

// VARIANTs.

/// The value of the VARIANT at `at` (flat), as OLEAUT32's changes take
/// it.
fn read_variant(cpu: &Cpu, at: u32, depth: u32) -> Result<Val, u32> {
    let vt = super::read_16(cpu, at);
    let mut place = at + 8;
    if vt & VT_BYREF != 0 {
        place = super::linear(cpu, super::read_32(cpu, at + 8));
        if place == 0 {
            return Err(DISP_E_TYPEMISMATCH);
        }
        if vt & !VT_BYREF == VT_VARIANT && depth == 0 {
            return read_variant(cpu, place, 1);
        }
    }
    if vt & VT_ARRAY != 0 {
        return Err(DISP_E_TYPEMISMATCH);
    }
    let bytes = super::peek(cpu, place, 8).unwrap_or_else(|| vec![0; 8]);
    let b = u64::from_le_bytes(bytes.try_into().expect("8 bytes"));
    Ok(match vt & !VT_BYREF {
        VT_EMPTY if vt & VT_BYREF != 0 => return Err(DISP_E_BADVARTYPE),
        VT_EMPTY => Val::Empty,
        VT_NULL => Val::Null,
        VT_I2 => Val::Int(b as u16 as i16 as i64),
        VT_I4 => Val::Int(b as u32 as i32 as i64),
        VT_UI1 => Val::Int(b as u8 as i64),
        VT_R4 => Val::Real(f32::from_bits(b as u32) as f64),
        VT_R8 => Val::Real(f64::from_bits(b)),
        VT_DATE => Val::Date(f64::from_bits(b)),
        VT_CY => Val::Cy(b as i64),
        VT_BOOL => Val::Bool(b as u16 != 0),
        VT_ERROR => Val::Error(b as u32),
        VT_BSTR => Val::Str(units(&bstr_bytes(cpu, b as u32))),
        VT_DISPATCH | VT_UNKNOWN => Val::Iface(vt & !VT_BYREF, b as u32),
        _ => return Err(DISP_E_BADVARTYPE),
    })
}

/// What a VARIANT of the type `vt` holding `value` needs released when
/// it is cleared: its BSTR freed, its interface released, its array
/// destroyed, when the function ends.
fn clear_value(cpu: &mut Cpu, heap: &mut Heap, vt: u16, value: u32) {
    if vt & VT_BYREF != 0 {
        return;
    }
    if vt & VT_ARRAY != 0 {
        destroy_array(cpu, heap, value);
        return;
    }
    match vt {
        VT_BSTR => drop_bstr(cpu, heap, value),
        VT_DISPATCH | VT_UNKNOWN if value != 0 => heap.method(value, RELEASE),
        _ => {}
    }
}

fn clear(cpu: &mut Cpu, heap: &mut Heap, at: u32) {
    let vt = super::read_16(cpu, at);
    let value = super::read_32(cpu, at + 8);
    clear_value(cpu, heap, vt, value);
    super::poke(cpu, at, &[0; 16]);
}

fn valid_vt(vt: u16) -> bool {
    let base = vt & !(VT_BYREF | VT_ARRAY);
    matches!(base, 0..=13 | VT_UI1) && !(vt & VT_ARRAY != 0 && base <= VT_NULL)
}

fn variant_init(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if args[0] != 0 {
        super::write_16(cpu, args[0], VT_EMPTY);
    }
    Some(0)
}

/// VariantClear(pvarg): its BSTR freed, its interface released, its
/// array destroyed, and the VARIANT VT_EMPTY.
fn variant_clear(cpu: &mut Cpu, args: &[u32], heap: &mut Heap) -> Result<u32, Stop> {
    if args[0] == 0 {
        return Ok(E_INVALIDARG);
    }
    let vt = super::read_16(cpu, args[0]);
    if !valid_vt(vt) {
        return Ok(DISP_E_BADVARTYPE);
    }
    clear(cpu, heap, args[0]);
    Ok(S_OK)
}

/// Put the value `out` in the VARIANT at `dest` (flat), after clearing
/// it.
fn store(cpu: &mut Cpu, heap: &mut Heap, dest: u32, out: Out) -> Result<(), Stop> {
    let (vt, value) = match out {
        Out::Bits(vt, b) => (vt, b),
        Out::Str(u) => match new_bstr(cpu, heap, &ansi(&u))? {
            0 => return Err(Stop::Fail(E_OUTOFMEMORY)),
            s => (VT_BSTR, s as u64),
        },
        Out::Iface(vt, p) => {
            if p != 0 {
                heap.method(p, ADD_REF);
            }
            (vt, p as u64)
        }
    };
    clear(cpu, heap, dest);
    super::write_16(cpu, dest, vt);
    super::poke(cpu, dest + 8, &value.to_le_bytes());
    Ok(())
}

/// VariantCopy(dest, src): a BSTR copied, an interface with a reference
/// added, an array copied, a VT_BYREF pointer as it is.
fn variant_copy(cpu: &mut Cpu, args: &[u32], heap: &mut Heap) -> Result<u32, Stop> {
    copy(cpu, heap, args[0], args[1], false)
}

/// VariantCopyInd(dest, src): a copy of what a VT_BYREF VARIANT points
/// at, or VariantCopy.
fn variant_copy_ind(cpu: &mut Cpu, args: &[u32], heap: &mut Heap) -> Result<u32, Stop> {
    copy(cpu, heap, args[0], args[1], true)
}

fn copy(cpu: &mut Cpu, heap: &mut Heap, dest: u32, src: u32, indirect: bool) -> Result<u32, Stop> {
    if dest == 0 || src == 0 {
        return Ok(E_INVALIDARG);
    }
    let vt = super::read_16(cpu, src);
    if !valid_vt(vt) {
        return Ok(DISP_E_BADVARTYPE);
    }
    let mut value = super::read_32(cpu, src + 8);
    let mut bits = super::peek(cpu, src + 8, 8).unwrap_or_else(|| vec![0; 8]);
    let mut base = vt;
    if indirect && vt & VT_BYREF != 0 {
        base = vt & !VT_BYREF;
        let at = super::linear(cpu, value);
        if base == VT_VARIANT {
            return copy(cpu, heap, dest, at, false);
        }
        bits = super::peek(cpu, at, 8).unwrap_or_else(|| vec![0; 8]);
        value = u32::from_le_bytes([bits[0], bits[1], bits[2], bits[3]]);
    } else if dest == src {
        return Ok(S_OK);
    }
    let new = if base & VT_BYREF != 0 {
        value
    } else if base & VT_ARRAY != 0 {
        copy_array(cpu, heap, value)?
    } else {
        match base {
            VT_BSTR if value != 0 => match new_bstr(cpu, heap, &bstr_bytes(cpu, value))? {
                0 => return Ok(E_OUTOFMEMORY),
                s => s,
            },
            VT_DISPATCH | VT_UNKNOWN => {
                if value != 0 {
                    heap.method(value, ADD_REF);
                }
                value
            }
            _ => value,
        }
    };
    clear(cpu, heap, dest);
    super::write_16(cpu, dest, base);
    super::poke(cpu, dest + 8, &bits);
    super::write_32(cpu, dest + 8, new);
    Ok(S_OK)
}

fn variant_change_type(cpu: &mut Cpu, args: &[u32], heap: &mut Heap) -> Result<u32, Stop> {
    change_type(cpu, heap, args[0], args[1], args[2], args[3] as u16)
}

/// VariantChangeTypeEx(dest, src, lcid, flags, vt): the locale's own
/// formats aren't taken; those of English (United States) are, as the
/// built-in OLEAUT32's.
fn variant_change_type_ex(cpu: &mut Cpu, args: &[u32], heap: &mut Heap) -> Result<u32, Stop> {
    change_type(cpu, heap, args[0], args[1], args[3], args[4] as u16)
}

/// VariantChangeType(dest, src, flags, vt).
fn change_type(
    cpu: &mut Cpu,
    heap: &mut Heap,
    dest: u32,
    src: u32,
    flags: u32,
    vt: u16,
) -> Result<u32, Stop> {
    if dest == 0 || src == 0 {
        return Ok(E_INVALIDARG);
    }
    match changed(cpu, src, vt, flags) {
        Ok(out) => store(cpu, heap, dest, out).map(|()| S_OK),
        Err(e) => Ok(e),
    }
}

/// The value of the VARIANT at `src` (flat) changed to the type `vt`,
/// as VariantChangeType changes it.
pub(super) fn changed(cpu: &Cpu, src: u32, vt: u16, flags: u32) -> Result<Out, u32> {
    let from = super::read_16(cpu, src);
    read_variant(cpu, src, 0).and_then(|v| {
        // A VT_R4's text is "%.7G" of the single value.
        if vt == VT_BSTR
            && from & !VT_BYREF == VT_R4
            && let Val::Real(f) = v
        {
            return Ok(Out::Str(units(format_g(f, 7).as_bytes())));
        }
        change(&v, vt, flags & 0xFFFF)
    })
}

// VarXxxFromYyy.

/// The type a function VarXxxFromYyy's "Xxx" or "Yyy" names.
fn vt_named(name: &str) -> Option<u16> {
    Some(match name {
        "I2" => VT_I2,
        "I4" => VT_I4,
        "R4" => VT_R4,
        "R8" => VT_R8,
        "Cy" => VT_CY,
        "Date" => VT_DATE,
        "Str" | "Bstr" => VT_BSTR,
        "Bool" => VT_BOOL,
        "UI1" => VT_UI1,
        _ => return None,
    })
}

/// The name of the system function whose stub the processor is in.
fn current_function(cpu: &mut Cpu) -> &'static str {
    let at = cpu.seg_cache(crate::cpu::Seg::CS).base + cpu.eip().wrapping_sub(4);
    let id = cpu.bus.guest_read_16(at + 2);
    super::builtin::api(id).map_or("", |(_, api)| api.name)
}

/// VarXxxFromYyy(in, [lcid, flags,] pout): the value of the type Yyy
/// changed to the type Xxx, as VariantChangeType changes it. Which
/// function it is comes from its name; its arguments are as its
/// signature reads them, a double or currency value as two dwords (the
/// high one first).
fn var_from(cpu: &mut Cpu, args: &[u32], heap: &mut Heap) -> Result<u32, Stop> {
    let (to, from) = heap
        .name
        .strip_prefix("Var")
        .and_then(|n| n.split_once("From"))
        .expect("a VarXxxFromYyy");
    let (Some(to), Some(from)) = (vt_named(to), vt_named(from)) else {
        return Ok(E_INVALIDARG);
    };
    let out = *args.last().expect("an out pointer");
    if out == 0 {
        return Ok(E_INVALIDARG);
    }
    let qword = || (args[0] as u64) << 32 | args[1] as u64;
    let v = match from {
        VT_I2 => Val::Int(args[0] as u16 as i16 as i64),
        VT_I4 => Val::Int(args[0] as i32 as i64),
        VT_UI1 => Val::Int(args[0] as u8 as i64),
        VT_R4 => Val::Real(f32::from_bits(args[0]) as f64),
        VT_R8 => Val::Real(f64::from_bits(qword())),
        VT_DATE => Val::Date(f64::from_bits(qword())),
        VT_CY => Val::Cy(qword() as i64),
        VT_BOOL => Val::Bool(args[0] as u16 != 0),
        _ => {
            if args[0] == 0 {
                return Ok(E_INVALIDARG);
            }
            Val::Str(units(&super::string_bytes(cpu, args[0], 0xFFF0)))
        }
    };
    // The flags come after the locale, before the out pointer, in the
    // functions that have them.
    let flags = if from == VT_BSTR || to == VT_BSTR {
        args[args.len() - 2]
    } else {
        0
    };
    let result = if to == VT_BSTR && from == VT_R4 {
        let Val::Real(f) = v else { unreachable!() };
        Ok(Out::Str(units(format_g(f, 7).as_bytes())))
    } else {
        change(&v, to, flags)
    };
    match result {
        Ok(Out::Bits(_, b)) => {
            let size = match to {
                VT_UI1 => 1,
                VT_I2 | VT_BOOL => 2,
                VT_I4 | VT_R4 => 4,
                _ => 8,
            };
            super::poke(cpu, out, &b.to_le_bytes()[..size]);
            Ok(S_OK)
        }
        Ok(Out::Str(u)) => match new_bstr(cpu, heap, &ansi(&u))? {
            0 => Ok(E_OUTOFMEMORY),
            s => {
                super::write_32(cpu, out, s);
                Ok(S_OK)
            }
        },
        Ok(Out::Iface(..)) => Ok(DISP_E_TYPEMISMATCH),
        Err(e) => Ok(e),
    }
}

// Safe arrays.

/// A safe array's descriptor, read from the program's.
struct Array {
    /// Its flat address.
    at: u32,
    dims: u16,
    features: u16,
    elem: u16,
    locks: u16,
    data: u32,
    /// The bounds (count, lower), the first dimension's first.
    bounds: Vec<(u32, i32)>,
}

impl Array {
    fn read(cpu: &Cpu, psa: u32) -> Option<Array> {
        let at = super::linear(cpu, psa);
        if at == 0 {
            return None;
        }
        let dims = super::read_16(cpu, at);
        let mut bounds = Vec::new();
        // The last dimension's bound is first in memory.
        for d in (0..dims as u32).rev() {
            let b = at + SA_HEADER + SA_BOUND * d;
            bounds.push((super::read_32(cpu, b), super::read_32(cpu, b + 4) as i32));
        }
        Some(Array {
            at,
            dims,
            features: super::read_16(cpu, at + 2),
            elem: super::read_16(cpu, at + 4),
            locks: super::read_16(cpu, at + 6),
            data: super::read_32(cpu, at + 12),
            bounds,
        })
    }

    fn cells(&self) -> u32 {
        self.bounds.iter().fold(1u32, |n, b| n.saturating_mul(b.0))
    }

    fn bytes(&self) -> u32 {
        self.cells().saturating_mul(self.elem as u32)
    }

    /// The offset in the data of the element at the indexes `indexes`,
    /// the first dimension's first and varying fastest.
    fn offset(&self, indexes: &[i32]) -> Option<u32> {
        let mut offset = 0u32;
        let mut stride = 1u32;
        for (&i, &(count, lower)) in indexes.iter().zip(&self.bounds) {
            let k = i.checked_sub(lower)?;
            if k < 0 || k as u32 >= count {
                return None;
            }
            offset += k as u32 * stride;
            stride = stride.saturating_mul(count);
        }
        Some(offset * self.elem as u32)
    }

    /// The flat address of the data's byte `offset`, across the
    /// selectors of a block bigger than 64 KB.
    fn data_at(&self, cpu: &Cpu, offset: u32) -> u32 {
        huge(cpu, self.data, offset)
    }
}

/// The flat address of the byte `offset` of the huge block at `p`: its
/// selectors are __AHINCR apart, each 64 KB.
fn huge(cpu: &Cpu, p: u32, offset: u32) -> u32 {
    let at = (p & 0xFFFF) + offset;
    let sel = (p >> 16) as u16 + (at >> 16) as u16 * super::ldt::AHINCR;
    super::lin(cpu, sel, at as u16)
}

/// The bytes an element of the type `vt` takes, and its features.
fn element(vt: u16) -> Option<(u16, u16)> {
    Some(match vt {
        VT_UI1 => (1, 0),
        VT_I2 | VT_BOOL => (2, 0),
        VT_I4 | VT_R4 | VT_ERROR => (4, 0),
        VT_R8 | VT_CY | VT_DATE => (8, 0),
        VT_BSTR => (4, FADF_BSTR),
        VT_UNKNOWN => (4, FADF_UNKNOWN),
        VT_DISPATCH => (4, FADF_DISPATCH),
        VT_VARIANT => (16, FADF_VARIANT),
        _ => return None,
    })
}

/// A descriptor of `dims` dimensions: its 16:16 address, or 0.
fn alloc_descriptor(cpu: &mut Cpu, heap: &mut Heap, dims: u16) -> Result<u32, Stop> {
    let size = SA_HEADER + SA_BOUND * dims as u32;
    let p = heap.alloc(cpu, size)?;
    if p != 0 {
        let at = super::linear(cpu, p);
        super::poke(cpu, at, &vec![0; size as usize]);
        super::write_16(cpu, at, dims);
    }
    Ok(p)
}

/// Zero `size` bytes of the huge block at `p` from its byte `from`.
fn zero(cpu: &mut Cpu, p: u32, from: u32, size: u32) {
    let mut done = from;
    while done < size {
        let n = (size - done).min(0x1000);
        let at = huge(cpu, p, done);
        super::poke(cpu, at, &vec![0; n as usize]);
        done += n;
    }
}

/// Copy `size` bytes from the byte `from` of the huge block at `src` to
/// the start of the huge block at `dest`.
fn copy_huge(cpu: &mut Cpu, src: u32, from: u32, dest: u32, size: u32) {
    let mut done = 0;
    while done < size {
        // No piece crosses either block's 64 KB boundaries.
        let s = (src & 0xFFFF) + from + done;
        let d = (dest & 0xFFFF) + done;
        let n = (size - done)
            .min(0x1000)
            .min(0x1_0000 - (s & 0xFFFF))
            .min(0x1_0000 - (d & 0xFFFF));
        let at = huge(cpu, src, from + done);
        let bytes = super::peek(cpu, at, n as usize).unwrap_or_else(|| vec![0; n as usize]);
        let to = huge(cpu, dest, done);
        super::poke(cpu, to, &bytes);
        done += n;
    }
}

/// The data of the array described at `a`, zeroed: false when there is
/// no memory.
fn alloc_data(cpu: &mut Cpu, heap: &mut Heap, a: &Array) -> Result<bool, Stop> {
    let size = a.bytes();
    let p = heap.alloc(cpu, size.max(1))?;
    if p == 0 {
        return Ok(false);
    }
    zero(cpu, p, 0, size);
    super::write_32(cpu, a.at + 12, p);
    Ok(true)
}

/// SafeArrayCreate(vt, cDims, rgsabound): a descriptor and its data,
/// zeroed.
fn safe_array_create(cpu: &mut Cpu, args: &[u32], heap: &mut Heap) -> Result<u32, Stop> {
    let [vt, dims, bounds] = args[..] else {
        unreachable!()
    };
    let (vt, dims) = (vt as u16, dims as u16);
    let Some((size, features)) = element(vt) else {
        return Ok(0);
    };
    if dims == 0 || bounds == 0 {
        return Ok(0);
    }
    let psa = alloc_descriptor(cpu, heap, dims)?;
    if psa == 0 {
        return Ok(0);
    }
    let at = super::linear(cpu, psa);
    super::write_16(cpu, at + 2, features);
    super::write_16(cpu, at + 4, size);
    // The program's bounds are the first dimension's first; the
    // descriptor's the last's.
    for d in 0..dims as u32 {
        let from = bounds + SA_BOUND * d;
        let to = at + SA_HEADER + SA_BOUND * (dims as u32 - 1 - d);
        let b = super::peek(cpu, from, 8).unwrap_or_else(|| vec![0; 8]);
        super::poke(cpu, to, &b);
    }
    let Some(a) = Array::read(cpu, psa) else {
        return Ok(0);
    };
    if !alloc_data(cpu, heap, &a)? {
        heap.free(cpu, psa);
        return Ok(0);
    }
    Ok(psa)
}

fn safe_array_alloc_descriptor(cpu: &mut Cpu, args: &[u32], heap: &mut Heap) -> Result<u32, Stop> {
    let [dims, out] = args[..] else {
        unreachable!()
    };
    if dims as u16 == 0 || out == 0 {
        return Ok(E_INVALIDARG);
    }
    match alloc_descriptor(cpu, heap, dims as u16)? {
        0 => Ok(E_OUTOFMEMORY),
        psa => {
            super::write_32(cpu, out, psa);
            Ok(S_OK)
        }
    }
}

fn safe_array_alloc_data(cpu: &mut Cpu, args: &[u32], heap: &mut Heap) -> Result<u32, Stop> {
    let Some(a) = Array::read(cpu, args[0]) else {
        return Ok(E_INVALIDARG);
    };
    Ok(if alloc_data(cpu, heap, &a)? {
        S_OK
    } else {
        E_OUTOFMEMORY
    })
}

/// The element at `at` of an array with the features `features`
/// emptied: its BSTR freed, its interface released, its VARIANT cleared.
fn clear_element(cpu: &mut Cpu, heap: &mut Heap, features: u16, at: u32) {
    if features & FADF_VARIANT != 0 {
        clear(cpu, heap, at);
        return;
    }
    let p = super::read_32(cpu, at);
    if features & FADF_BSTR != 0 {
        drop_bstr(cpu, heap, p);
    } else if p != 0 {
        heap.method(p, RELEASE);
    }
    super::write_32(cpu, at, 0);
}

/// Whether an array's elements hold what must be freed or released.
fn owns(features: u16) -> bool {
    features & (FADF_BSTR | FADF_UNKNOWN | FADF_DISPATCH | FADF_VARIANT) != 0
}

/// The elements' BSTRs freed, interfaces released, VARIANTs cleared.
fn clear_elements(cpu: &mut Cpu, heap: &mut Heap, a: &Array) {
    if a.data == 0 || !owns(a.features) {
        return;
    }
    for cell in 0..a.cells() {
        let at = a.data_at(cpu, cell * a.elem as u32);
        clear_element(cpu, heap, a.features, at);
    }
}

/// Destroy the array at `psa` and its data, as SafeArrayDestroy does.
fn destroy_array(cpu: &mut Cpu, heap: &mut Heap, psa: u32) {
    if let Some(a) = Array::read(cpu, psa) {
        clear_elements(cpu, heap, &a);
        heap.free(cpu, a.data);
        heap.free(cpu, psa);
    }
}

fn safe_array_destroy(cpu: &mut Cpu, args: &[u32], heap: &mut Heap) -> Result<u32, Stop> {
    let Some(a) = Array::read(cpu, args[0]) else {
        return Ok(if args[0] == 0 { S_OK } else { E_INVALIDARG });
    };
    if a.locks > 0 {
        return Ok(DISP_E_ARRAYISLOCKED);
    }
    destroy_array(cpu, heap, args[0]);
    Ok(S_OK)
}

fn safe_array_destroy_data(cpu: &mut Cpu, args: &[u32], heap: &mut Heap) -> Result<u32, Stop> {
    let Some(a) = Array::read(cpu, args[0]) else {
        return Ok(E_INVALIDARG);
    };
    if a.locks > 0 {
        return Ok(DISP_E_ARRAYISLOCKED);
    }
    clear_elements(cpu, heap, &a);
    if a.data != 0 {
        heap.free(cpu, a.data);
        super::write_32(cpu, a.at + 12, 0);
    }
    Ok(S_OK)
}

fn safe_array_destroy_descriptor(
    cpu: &mut Cpu,
    args: &[u32],
    heap: &mut Heap,
) -> Result<u32, Stop> {
    let Some(a) = Array::read(cpu, args[0]) else {
        return Ok(E_INVALIDARG);
    };
    if a.locks > 0 {
        return Ok(DISP_E_ARRAYISLOCKED);
    }
    heap.free(cpu, args[0]);
    Ok(S_OK)
}

fn safe_array_get_dim(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(Array::read(cpu, args[0]).map_or(0, |a| a.dims as u32))
}

fn safe_array_get_elemsize(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(Array::read(cpu, args[0]).map_or(0, |a| a.elem as u32))
}

/// The bound (count, lower) of the dimension `dim` (from 1).
fn bound(cpu: &Cpu, psa: u32, dim: u32) -> Result<(u32, i32), u32> {
    let a = Array::read(cpu, psa).ok_or(E_INVALIDARG)?;
    let dim = dim as u16;
    if dim == 0 || dim > a.dims {
        return Err(DISP_E_BADINDEX);
    }
    Ok(a.bounds[dim as usize - 1])
}

fn safe_array_get_ubound(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    match bound(cpu, args[0], args[1]) {
        Ok((count, lower)) => {
            let upper = lower.wrapping_add(count as i32).wrapping_sub(1);
            super::write_32(cpu, args[2], upper as u32);
            Some(S_OK)
        }
        Err(e) => Some(e),
    }
}

fn safe_array_get_lbound(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    match bound(cpu, args[0], args[1]) {
        Ok((_, lower)) => {
            super::write_32(cpu, args[2], lower as u32);
            Some(S_OK)
        }
        Err(e) => Some(e),
    }
}

fn safe_array_lock(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(a) = Array::read(cpu, args[0]) else {
        return Some(E_INVALIDARG);
    };
    super::write_16(cpu, a.at + 6, a.locks.wrapping_add(1));
    Some(S_OK)
}

/// SafeArrayUnlock, and SafeArrayUnaccessData: E_UNEXPECTED for an
/// array that isn't locked.
fn safe_array_unlock(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(a) = Array::read(cpu, args[0]) else {
        return Some(E_INVALIDARG);
    };
    if a.locks == 0 {
        return Some(E_UNEXPECTED);
    }
    super::write_16(cpu, a.at + 6, a.locks - 1);
    Some(S_OK)
}

fn safe_array_access_data(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(a) = Array::read(cpu, args[0]) else {
        return Some(E_INVALIDARG);
    };
    if args[1] == 0 {
        return Some(E_INVALIDARG);
    }
    super::write_16(cpu, a.at + 6, a.locks.wrapping_add(1));
    super::write_32(cpu, args[1], a.data);
    Some(S_OK)
}

/// The 16:16 address of the element at the indexes at `indexes`.
fn element_ptr(cpu: &Cpu, a: &Array, indexes: u32) -> Option<(u32, u32)> {
    if indexes == 0 {
        return None;
    }
    let idx: Vec<i32> = (0..a.dims as u32)
        .map(|d| super::read_32(cpu, indexes + 4 * d) as i32)
        .collect();
    let offset = a.offset(&idx)?;
    let at = (a.data & 0xFFFF) + offset;
    let sel = (a.data >> 16) as u16 + (at >> 16) as u16 * super::ldt::AHINCR;
    Some(((sel as u32) << 16 | at & 0xFFFF, a.data_at(cpu, offset)))
}

fn safe_array_ptr_of_index(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(a) = Array::read(cpu, args[0]) else {
        return Some(E_INVALIDARG);
    };
    match element_ptr(cpu, &a, args[1]) {
        Some((p, _)) if args[2] != 0 => {
            super::write_32(cpu, args[2], p);
            Some(S_OK)
        }
        Some(_) => Some(E_INVALIDARG),
        None => Some(DISP_E_BADINDEX),
    }
}

/// SafeArrayGetElement(psa, rgIndices, pv): a copy of the element at
/// `pv` (16:16): a BSTR copied, an interface with a reference added, a
/// VARIANT as VariantCopy copies it.
fn safe_array_get_element(cpu: &mut Cpu, args: &[u32], heap: &mut Heap) -> Result<u32, Stop> {
    let Some(a) = Array::read(cpu, args[0]) else {
        return Ok(E_INVALIDARG);
    };
    let out = super::linear(cpu, args[2]);
    if out == 0 {
        return Ok(E_INVALIDARG);
    }
    let Some((_, at)) = element_ptr(cpu, &a, args[1]) else {
        return Ok(DISP_E_BADINDEX);
    };
    if a.features & FADF_VARIANT != 0 {
        super::poke(cpu, out, &[0; 16]);
        return copy(cpu, heap, out, at, false);
    }
    let bytes = super::peek(cpu, at, a.elem as usize).unwrap_or_default();
    if a.features & FADF_BSTR != 0 {
        let s = super::read_32(cpu, at);
        let new = if s == 0 {
            0
        } else {
            match new_bstr(cpu, heap, &bstr_bytes(cpu, s))? {
                0 => return Ok(E_OUTOFMEMORY),
                n => n,
            }
        };
        super::write_32(cpu, out, new);
        return Ok(S_OK);
    }
    if a.features & (FADF_UNKNOWN | FADF_DISPATCH) != 0 {
        let p = super::read_32(cpu, at);
        if p != 0 {
            heap.method(p, ADD_REF);
        }
    }
    super::poke(cpu, out, &bytes);
    Ok(S_OK)
}

/// SafeArrayPutElement(psa, rgIndices, pv): the value at `pv` (16:16)
/// put in the element, whose old value is freed. For an array of BSTRs
/// or interfaces, `pv` is the BSTR or interface itself.
fn safe_array_put_element(cpu: &mut Cpu, args: &[u32], heap: &mut Heap) -> Result<u32, Stop> {
    let Some(a) = Array::read(cpu, args[0]) else {
        return Ok(E_INVALIDARG);
    };
    let Some((_, at)) = element_ptr(cpu, &a, args[1]) else {
        return Ok(DISP_E_BADINDEX);
    };
    let pv = args[2];
    if a.features & FADF_VARIANT != 0 {
        let src = super::linear(cpu, pv);
        return copy(cpu, heap, at, src, false);
    }
    if a.features & FADF_BSTR != 0 {
        let new = if pv == 0 {
            0
        } else {
            match new_bstr(cpu, heap, &bstr_bytes(cpu, pv))? {
                0 => return Ok(E_OUTOFMEMORY),
                n => n,
            }
        };
        let old = super::read_32(cpu, at);
        drop_bstr(cpu, heap, old);
        super::write_32(cpu, at, new);
        return Ok(S_OK);
    }
    if a.features & (FADF_UNKNOWN | FADF_DISPATCH) != 0 {
        let old = super::read_32(cpu, at);
        if pv != 0 {
            heap.method(pv, ADD_REF);
        }
        if old != 0 {
            heap.method(old, RELEASE);
        }
        super::write_32(cpu, at, pv);
        return Ok(S_OK);
    }
    let from = super::linear(cpu, pv);
    if from == 0 {
        return Ok(E_INVALIDARG);
    }
    let bytes = super::peek(cpu, from, a.elem as usize).unwrap_or_default();
    super::poke(cpu, at, &bytes);
    Ok(S_OK)
}

/// A copy of the array at `psa` (16:16) and its elements: the copy's
/// 16:16 address, the AddRefs to make asked of `heap`.
fn copy_array(cpu: &mut Cpu, heap: &mut Heap, psa: u32) -> Result<u32, Stop> {
    if psa == 0 {
        return Ok(0);
    }
    let a = Array::read(cpu, psa).ok_or(Stop::Fail(E_INVALIDARG))?;
    let new = alloc_descriptor(cpu, heap, a.dims)?;
    if new == 0 {
        return Err(Stop::Fail(E_OUTOFMEMORY));
    }
    let size = SA_HEADER + SA_BOUND * a.dims as u32;
    let to = super::linear(cpu, new);
    let head = super::peek(cpu, a.at, size as usize).unwrap_or_default();
    super::poke(cpu, to, &head);
    super::write_16(cpu, to + 6, 0);
    super::write_32(cpu, to + 12, 0);
    if a.data == 0 {
        return Ok(new);
    }
    let b = Array::read(cpu, new).ok_or(Stop::Fail(E_INVALIDARG))?;
    if !alloc_data(cpu, heap, &b)? {
        heap.free(cpu, new);
        return Err(Stop::Fail(E_OUTOFMEMORY));
    }
    let b = Array::read(cpu, new).ok_or(Stop::Fail(E_INVALIDARG))?;
    for cell in 0..a.cells() {
        let offset = cell * a.elem as u32;
        let from = a.data_at(cpu, offset);
        let to = b.data_at(cpu, offset);
        if a.features & FADF_VARIANT != 0 {
            let vt = super::read_16(cpu, from);
            let value = super::read_32(cpu, from + 8);
            let mut bytes = super::peek(cpu, from, 16).unwrap_or_else(|| vec![0; 16]);
            let new_value = match vt {
                VT_BSTR if value != 0 => new_bstr(cpu, heap, &bstr_bytes(cpu, value))?,
                VT_DISPATCH | VT_UNKNOWN if value != 0 => {
                    heap.method(value, ADD_REF);
                    value
                }
                _ if vt & VT_ARRAY != 0 && vt & VT_BYREF == 0 => copy_array(cpu, heap, value)?,
                _ => value,
            };
            bytes[8..12].copy_from_slice(&new_value.to_le_bytes());
            super::poke(cpu, to, &bytes);
            continue;
        }
        let p = super::read_32(cpu, from);
        let bytes = super::peek(cpu, from, a.elem as usize).unwrap_or_default();
        super::poke(cpu, to, &bytes);
        if a.features & FADF_BSTR != 0 && p != 0 {
            let s = new_bstr(cpu, heap, &bstr_bytes(cpu, p))?;
            super::write_32(cpu, to, s);
        } else if a.features & (FADF_UNKNOWN | FADF_DISPATCH) != 0 && p != 0 {
            heap.method(p, ADD_REF);
        }
    }
    Ok(new)
}

fn safe_array_copy(cpu: &mut Cpu, args: &[u32], heap: &mut Heap) -> Result<u32, Stop> {
    if args[1] == 0 {
        return Ok(E_INVALIDARG);
    }
    match copy_array(cpu, heap, args[0]) {
        Ok(p) => {
            super::write_32(cpu, args[1], p);
            Ok(S_OK)
        }
        Err(Stop::Fail(e)) => {
            super::write_32(cpu, args[1], 0);
            Err(Stop::Fail(e))
        }
        Err(Stop::Pending) => Err(Stop::Pending),
    }
}

/// SafeArrayRedim(psa, psaboundNew): the last dimension's bound changed,
/// the data reallocated, grown (zeroed) or cut (the elements cut freed).
/// As Windows' OLE2DISP does, the elements cut are copied to a block of
/// their own before the data's Realloc, which may move or free them, and
/// freed from there after it.
fn safe_array_redim(cpu: &mut Cpu, args: &[u32], heap: &mut Heap) -> Result<u32, Stop> {
    let Some(a) = Array::read(cpu, args[0]) else {
        return Ok(E_INVALIDARG);
    };
    if args[1] == 0 || a.dims == 0 {
        return Ok(E_INVALIDARG);
    }
    if a.locks > 0 {
        return Ok(DISP_E_ARRAYISLOCKED);
    }
    let count = super::read_32(cpu, args[1]);
    let lower = super::read_32(cpu, args[1] + 4);
    let old_bytes = a.bytes();
    let last = a.bounds[a.dims as usize - 1].0;
    let per = old_bytes.checked_div(last).unwrap_or(0);
    let new_bytes = per.saturating_mul(count);
    let mut data = a.data;
    if new_bytes != old_bytes || data == 0 {
        let cut = if new_bytes < old_bytes && data != 0 && owns(a.features) {
            old_bytes - new_bytes
        } else {
            0
        };
        let kept = if cut > 0 { heap.alloc(cpu, cut)? } else { 0 };
        if cut > 0 && kept == 0 {
            return Ok(E_OUTOFMEMORY);
        }
        // A run after the program's Realloc returned finds the elements
        // in their block already, and the data may be gone.
        if kept != 0 && !heap.answered() {
            copy_huge(cpu, a.data, new_bytes, kept, cut);
        }
        data = heap.realloc(cpu, a.data, new_bytes.max(1))?;
        if data == 0 {
            heap.free(cpu, kept);
            return Ok(E_OUTOFMEMORY);
        }
        zero(cpu, data, old_bytes, new_bytes);
        let elem = a.elem.max(1) as u32;
        for cell in 0..cut / elem {
            let at = huge(cpu, kept, cell * elem);
            clear_element(cpu, heap, a.features, at);
        }
        heap.free(cpu, kept);
    }
    super::write_32(cpu, a.at + 12, data);
    super::write_32(cpu, a.at + SA_HEADER, count);
    super::write_32(cpu, a.at + SA_HEADER + 4, lower);
    Ok(S_OK)
}

// Dispatch.

/// DispGetIDsOfNames(ptinfo, rgszNames, cNames, rgdispid): the type
/// information's ITypeInfo::GetIDsOfNames.
fn disp_get_ids_of_names(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [tinfo, names, count, ids] = args[..] else {
        unreachable!()
    };
    call_method(cpu, tinfo, 10, &[names, count, ids], "lwl", None)
}

/// DispInvoke(_this, ptinfo, dispidMember, wFlags, pparams, pvarResult,
/// pexcepinfo, puArgErr): the type information's ITypeInfo::Invoke on
/// the object.
fn disp_invoke(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, tinfo, id, flags, params, result, excep, arg_err] = args[..] else {
        unreachable!()
    };
    call_method(
        cpu,
        tinfo,
        11,
        &[this, id, flags, params, result, excep, arg_err],
        "llwllll",
        None,
    )
}

// Error information.

/// SetErrorInfo(dwReserved, perrinfo): the error object kept with a
/// reference, the one before released.
fn set_error_info(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let new = args[1];
    let old = std::mem::replace(&mut ole2::this_task_or_new(cpu).error_info, new);
    let mut calls = Vec::new();
    if new != 0 {
        calls.push((new, 1));
    }
    if old != 0 {
        calls.push((old, 2));
    }
    call_all(cpu, S_OK, &calls)
}

/// GetErrorInfo(dwReserved, pperrinfo): the error object, its reference
/// the caller's now, or S_FALSE and NULL when there is none.
fn get_error_info(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if args[1] == 0 {
        return Some(E_INVALIDARG);
    }
    let info = std::mem::take(&mut ole2::this_task_or_new(cpu).error_info);
    super::write_32(cpu, args[1], info);
    Some(if info == 0 { S_FALSE } else { S_OK })
}

// The error object CreateErrorInfo makes. One block holds both its
// interfaces' vtable pointers, ICreateErrorInfo's first and IErrorInfo's
// after it, its count of references, the GUID, the source, description
// and help file as BSTRs, the help context, then the two vtables.

const EI_ERROR_INFO: u32 = 4;
const EI_REFS: u32 = 8;
const EI_GUID: u32 = 10;
const EI_SOURCE: u32 = 26;
const EI_DESCRIPTION: u32 = 30;
const EI_HELP_FILE: u32 = 34;
const EI_HELP_CONTEXT: u32 = 38;
const EI_FIELDS: u32 = 42;

const CREATE_ERROR_INFO_METHODS: [&str; 8] = [
    "ICreateErrorInfo_QueryInterface",
    "ICreateErrorInfo_AddRef",
    "ICreateErrorInfo_Release",
    "ICreateErrorInfo_SetGUID",
    "ICreateErrorInfo_SetSource",
    "ICreateErrorInfo_SetDescription",
    "ICreateErrorInfo_SetHelpFile",
    "ICreateErrorInfo_SetHelpContext",
];

const ERROR_INFO_METHODS: [&str; 8] = [
    "IErrorInfo_QueryInterface",
    "IErrorInfo_AddRef",
    "IErrorInfo_Release",
    "IErrorInfo_GetGUID",
    "IErrorInfo_GetSource",
    "IErrorInfo_GetDescription",
    "IErrorInfo_GetHelpFile",
    "IErrorInfo_GetHelpContext",
];

/// CreateErrorInfo(ppcerrinfo): a new error object, by its
/// ICreateErrorInfo, with one reference.
fn create_error_info(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let out = args[0];
    if out == 0 {
        return Some(E_INVALIDARG);
    }
    super::write_32(cpu, out, 0);
    let Some(module) = super::builtin::find("OLE2DISP") else {
        return Some(E_OUTOFMEMORY);
    };
    let methods = CREATE_ERROR_INFO_METHODS.len() as u32;
    let size = EI_FIELDS + 8 * methods;
    let p = ole2::alloc(cpu, size);
    let at = super::linear(cpu, p);
    if at == 0 {
        return Some(E_OUTOFMEMORY);
    }
    super::poke(cpu, at, &vec![0; size as usize]);
    let vtables = [
        (0, EI_FIELDS, &CREATE_ERROR_INFO_METHODS),
        (EI_ERROR_INFO, EI_FIELDS + 4 * methods, &ERROR_INFO_METHODS),
    ];
    for (place, vtable, names) in vtables {
        super::write_32(cpu, at + place, p + vtable);
        for (i, name) in names.iter().enumerate() {
            let stub = super::builtin::stub_address(cpu, module, name).unwrap_or(0);
            super::write_32(cpu, at + vtable + 4 * i as u32, stub);
        }
    }
    super::write_16(cpu, at + EI_REFS, 1);
    super::write_32(cpu, out, p);
    Some(S_OK)
}

/// The error object's block, its 16:16 address and its linear one, from
/// the interface pointer `this`: ICreateErrorInfo's if `error_info` is
/// false, else IErrorInfo's.
fn error_object(cpu: &Cpu, this: u32, error_info: bool) -> (u32, u32) {
    let p = if error_info {
        this.wrapping_sub(EI_ERROR_INFO)
    } else {
        this
    };
    (p, super::linear(cpu, p))
}

fn error_query_interface(cpu: &mut Cpu, args: &[u32], error_info: bool) -> Option<u32> {
    let [this, iid, out] = args[..] else {
        unreachable!()
    };
    if out == 0 {
        return Some(E_INVALIDARG);
    }
    let (p, at) = error_object(cpu, this, error_info);
    let iid = ole2::read_guid(cpu, iid);
    let found = if iid == ole2::ole_guid(0) || iid == IID_ICREATEERRORINFO {
        Some(p)
    } else if iid == IID_IERRORINFO {
        Some(p + EI_ERROR_INFO)
    } else {
        None
    };
    let Some(found) = found else {
        super::write_32(cpu, out, 0);
        return Some(E_NOINTERFACE);
    };
    let refs = super::read_16(cpu, at + EI_REFS).saturating_add(1);
    super::write_16(cpu, at + EI_REFS, refs);
    super::write_32(cpu, out, found);
    Some(S_OK)
}

fn error_add_ref(cpu: &mut Cpu, this: u32, error_info: bool) -> Option<u32> {
    let (_, at) = error_object(cpu, this, error_info);
    let refs = super::read_16(cpu, at + EI_REFS).saturating_add(1);
    super::write_16(cpu, at + EI_REFS, refs);
    Some(refs as u32)
}

/// Release: the last reference frees the strings and the object.
fn error_release(cpu: &mut Cpu, this: u32, error_info: bool) -> Option<u32> {
    let (p, at) = error_object(cpu, this, error_info);
    let refs = super::read_16(cpu, at + EI_REFS).saturating_sub(1);
    super::write_16(cpu, at + EI_REFS, refs);
    if refs == 0 {
        for field in [EI_SOURCE, EI_DESCRIPTION, EI_HELP_FILE] {
            free_bstr(cpu, super::read_32(cpu, at + field));
        }
        ole2::free(cpu, p);
    }
    Some(refs as u32)
}

fn cei_query_interface(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    error_query_interface(cpu, args, false)
}

fn cei_add_ref(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    error_add_ref(cpu, args[0], false)
}

fn cei_release(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    error_release(cpu, args[0], false)
}

fn ei_query_interface(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    error_query_interface(cpu, args, true)
}

fn ei_add_ref(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    error_add_ref(cpu, args[0], true)
}

fn ei_release(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    error_release(cpu, args[0], true)
}

/// SetGUID(rguid).
fn cei_set_guid(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (_, at) = error_object(cpu, args[0], false);
    let guid = ole2::read_guid(cpu, args[1]);
    super::poke(cpu, at + EI_GUID, &guid);
    Some(S_OK)
}

/// SetSource, SetDescription and SetHelpFile(sz): a copy of the string
/// in place of the one before, NULL for none.
fn cei_set_string(cpu: &mut Cpu, args: &[u32], field: u32) -> Option<u32> {
    let (_, at) = error_object(cpu, args[0], false);
    let new = if args[1] == 0 {
        0
    } else {
        let text = super::string_bytes(cpu, args[1], 0xFFF0);
        match alloc_bstr(cpu, &text) {
            0 => return Some(E_OUTOFMEMORY),
            s => s,
        }
    };
    free_bstr(cpu, super::read_32(cpu, at + field));
    super::write_32(cpu, at + field, new);
    Some(S_OK)
}

fn cei_set_source(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    cei_set_string(cpu, args, EI_SOURCE)
}

fn cei_set_description(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    cei_set_string(cpu, args, EI_DESCRIPTION)
}

fn cei_set_help_file(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    cei_set_string(cpu, args, EI_HELP_FILE)
}

/// SetHelpContext(dwHelpContext).
fn cei_set_help_context(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (_, at) = error_object(cpu, args[0], false);
    super::write_32(cpu, at + EI_HELP_CONTEXT, args[1]);
    Some(S_OK)
}

/// GetGUID(pguid).
fn ei_get_guid(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if args[1] == 0 {
        return Some(E_INVALIDARG);
    }
    let (_, at) = error_object(cpu, args[0], true);
    let guid = ole2::read_guid(cpu, at + EI_GUID);
    super::poke(cpu, args[1], &guid);
    Some(S_OK)
}

/// GetSource, GetDescription and GetHelpFile(pbstr): a copy of the
/// string, which the caller frees, NULL for none.
fn ei_get_string(cpu: &mut Cpu, args: &[u32], field: u32) -> Option<u32> {
    if args[1] == 0 {
        return Some(E_INVALIDARG);
    }
    let (_, at) = error_object(cpu, args[0], true);
    let s = super::read_32(cpu, at + field);
    let copy = if s == 0 {
        0
    } else {
        match alloc_bstr(cpu, &bstr_bytes(cpu, s)) {
            0 => {
                super::write_32(cpu, args[1], 0);
                return Some(E_OUTOFMEMORY);
            }
            copy => copy,
        }
    };
    super::write_32(cpu, args[1], copy);
    Some(S_OK)
}

fn ei_get_source(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    ei_get_string(cpu, args, EI_SOURCE)
}

fn ei_get_description(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    ei_get_string(cpu, args, EI_DESCRIPTION)
}

fn ei_get_help_file(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    ei_get_string(cpu, args, EI_HELP_FILE)
}

/// GetHelpContext(pdwHelpContext).
fn ei_get_help_context(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if args[1] == 0 {
        return Some(E_INVALIDARG);
    }
    let (_, at) = error_object(cpu, args[0], true);
    let context = super::read_32(cpu, at + EI_HELP_CONTEXT);
    super::write_32(cpu, args[1], context);
    Some(S_OK)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn var_functions_name_their_types() {
        for api in OLE2DISP.apis {
            let Some(rest) = api.name.strip_prefix("Var") else {
                continue;
            };
            let Some((to, from)) = rest.split_once("From") else {
                continue;
            };
            if from == "Disp" {
                continue;
            }
            assert!(vt_named(to).is_some(), "{}", api.name);
            assert!(vt_named(from).is_some(), "{}", api.name);
            // The value's dwords, the locale and flags with text, the
            // out pointer.
            let value = match vt_named(from).unwrap() {
                VT_R8 | VT_CY | VT_DATE => "ll",
                VT_I2 | VT_BOOL | VT_UI1 => "w",
                VT_BSTR => "pll",
                _ => "l",
            };
            let ex = if to == "Bstr" { "ll" } else { "" };
            assert_eq!(api.args, format!("{value}{ex}p"), "{}", api.name);
        }
    }

    #[test]
    fn array_offsets_vary_fastest_in_the_first_dimension() {
        let a = Array {
            at: 0,
            dims: 2,
            features: 0,
            elem: 2,
            locks: 0,
            data: 0,
            bounds: vec![(3, 1), (2, 0)],
        };
        assert_eq!(a.offset(&[1, 0]), Some(0));
        assert_eq!(a.offset(&[2, 0]), Some(2));
        assert_eq!(a.offset(&[1, 1]), Some(6));
        assert_eq!(a.offset(&[3, 1]), Some(10));
        assert_eq!(a.offset(&[4, 0]), None);
        assert_eq!(a.offset(&[0, 0]), None);
    }
}
