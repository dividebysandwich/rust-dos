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
//! BSTRs and safe arrays take their blocks from the built-in task
//! allocator (ole2.rs). The real OLE2DISP takes them from the one
//! CoGetMalloc gives, the program's own when it gave CoInitialize one;
//! a program sees the difference only when it frees one of them with
//! IMalloc::Free rather than with OLE2DISP's functions (#1586).
//!
//! Copyright 1998 Jean-Claude Cote
//! Copyright 2003 Jon Griffiths
//! Copyright 2005 Daniel Remenak

use super::builtin::{Def, missing, rust, variable};
use super::ole2::{self, call_all, call_method, guid};
use crate::cpu::Cpu;
use crate::win32::com::{E_INVALIDARG, E_OUTOFMEMORY, S_FALSE, S_OK};
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

pub static OLE2DISP: Def = Def {
    name: "OLE2DISP",
    file: "OLE2DISP.DLL",
    apis: &[
        missing("DllGetClassObject", 1),
        rust("SysAllocString", 2, "p", sys_alloc_string),
        rust("SysReAllocString", 3, "pp", sys_realloc_string),
        rust("SysAllocStringLen", 4, "pw", sys_alloc_string_len),
        rust("SysReAllocStringLen", 5, "ppw", sys_realloc_string_len),
        rust("SysFreeString", 6, "l", sys_free_string),
        rust("SysStringLen", 7, "l", sys_string_len),
        rust("VariantInit", 8, "p", variant_init),
        rust("VariantClear", 9, "p", variant_clear),
        rust("VariantCopy", 10, "pp", variant_copy),
        rust("VariantCopyInd", 11, "pp", variant_copy_ind),
        rust("VariantChangeType", 12, "ppww", variant_change_type),
        missing("VariantTimeToDosDateTime", 13),
        missing("DosDateTimeToVariantTime", 14),
        rust("SafeArrayCreate", 15, "wwp", safe_array_create),
        rust("SafeArrayDestroy", 16, "l", safe_array_destroy),
        rust("SafeArrayGetDim", 17, "l", safe_array_get_dim),
        rust("SafeArrayGetElemsize", 18, "l", safe_array_get_elemsize),
        rust("SafeArrayGetUBound", 19, "lwp", safe_array_get_ubound),
        rust("SafeArrayGetLBound", 20, "lwp", safe_array_get_lbound),
        rust("SafeArrayLock", 21, "l", safe_array_lock),
        rust("SafeArrayUnlock", 22, "l", safe_array_unlock),
        rust("SafeArrayAccessData", 23, "lp", safe_array_access_data),
        rust("SafeArrayUnaccessData", 24, "l", safe_array_unlock),
        rust("SafeArrayGetElement", 25, "lpl", safe_array_get_element),
        rust("SafeArrayPutElement", 26, "lpl", safe_array_put_element),
        rust("SafeArrayCopy", 27, "lp", safe_array_copy),
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
        rust(
            "SafeArrayAllocDescriptor",
            38,
            "wp",
            safe_array_alloc_descriptor,
        ),
        rust("SafeArrayAllocData", 39, "l", safe_array_alloc_data),
        rust(
            "SafeArrayDestroyDescriptor",
            40,
            "l",
            safe_array_destroy_descriptor,
        ),
        rust("SafeArrayDestroyData", 41, "l", safe_array_destroy_data),
        rust("SafeArrayRedim", 42, "lp", safe_array_redim),
        rust("VarI2FromI4", 43, "lp", var_from),
        rust("VarI2FromR4", 44, "lp", var_from),
        rust("VarI2FromR8", 45, "llp", var_from),
        rust("VarI2FromCy", 46, "llp", var_from),
        rust("VarI2FromDate", 47, "llp", var_from),
        rust("VarI2FromStr", 48, "pllp", var_from),
        missing("VarI2FromDisp", 49),
        rust("VarI2FromBool", 50, "wp", var_from),
        rust("VarI4FromI2", 51, "wp", var_from),
        rust("VarI4FromR4", 52, "lp", var_from),
        rust("VarI4FromR8", 53, "llp", var_from),
        rust("VarI4FromCy", 54, "llp", var_from),
        rust("VarI4FromDate", 55, "llp", var_from),
        rust("VarI4FromStr", 56, "pllp", var_from),
        missing("VarI4FromDisp", 57),
        rust("VarI4FromBool", 58, "wp", var_from),
        rust("VarR4FromI2", 59, "wp", var_from),
        rust("VarR4FromI4", 60, "lp", var_from),
        rust("VarR4FromR8", 61, "llp", var_from),
        rust("VarR4FromCy", 62, "llp", var_from),
        rust("VarR4FromDate", 63, "llp", var_from),
        rust("VarR4FromStr", 64, "pllp", var_from),
        missing("VarR4FromDisp", 65),
        rust("VarR4FromBool", 66, "wp", var_from),
        rust("VarR8FromI2", 67, "wp", var_from),
        rust("VarR8FromI4", 68, "lp", var_from),
        rust("VarR8FromR4", 69, "lp", var_from),
        rust("VarR8FromCy", 70, "llp", var_from),
        rust("VarR8FromDate", 71, "llp", var_from),
        rust("VarR8FromStr", 72, "pllp", var_from),
        missing("VarR8FromDisp", 73),
        rust("VarR8FromBool", 74, "wp", var_from),
        rust("VarDateFromI2", 75, "wp", var_from),
        rust("VarDateFromI4", 76, "lp", var_from),
        rust("VarDateFromR4", 77, "lp", var_from),
        rust("VarDateFromR8", 78, "llp", var_from),
        rust("VarDateFromCy", 79, "llp", var_from),
        rust("VarDateFromStr", 80, "pllp", var_from),
        missing("VarDateFromDisp", 81),
        rust("VarDateFromBool", 82, "wp", var_from),
        rust("VarCyFromI2", 83, "wp", var_from),
        rust("VarCyFromI4", 84, "lp", var_from),
        rust("VarCyFromR4", 85, "lp", var_from),
        rust("VarCyFromR8", 86, "llp", var_from),
        rust("VarCyFromDate", 87, "llp", var_from),
        rust("VarCyFromStr", 88, "pllp", var_from),
        missing("VarCyFromDisp", 89),
        rust("VarCyFromBool", 90, "wp", var_from),
        rust("VarBstrFromI2", 91, "wllp", var_from),
        rust("VarBstrFromI4", 92, "lllp", var_from),
        rust("VarBstrFromR4", 93, "lllp", var_from),
        rust("VarBstrFromR8", 94, "llllp", var_from),
        rust("VarBstrFromCy", 95, "llllp", var_from),
        rust("VarBstrFromDate", 96, "llllp", var_from),
        missing("VarBstrFromDisp", 97),
        rust("VarBstrFromBool", 98, "wllp", var_from),
        rust("VarBoolFromI2", 99, "wp", var_from),
        rust("VarBoolFromI4", 100, "lp", var_from),
        rust("VarBoolFromR4", 101, "lp", var_from),
        rust("VarBoolFromR8", 102, "llp", var_from),
        rust("VarBoolFromDate", 103, "llp", var_from),
        rust("VarBoolFromCy", 104, "llp", var_from),
        rust("VarBoolFromStr", 105, "pllp", var_from),
        missing("VarBoolFromDisp", 106),
        missing("DoInvokeMethod", 107),
        rust("VariantChangeTypeEx", 108, "pplww", variant_change_type_ex),
        rust("SafeArrayPtrOfIndex", 109, "lpp", safe_array_ptr_of_index),
        rust("SetErrorInfo", 110, "ll", set_error_info),
        rust("GetErrorInfo", 111, "lp", get_error_info),
        missing("CreateErrorInfo", 112),
        variable(
            "_IID_IErrorInfo",
            113,
            &guid(
                0x1CF2_B120,
                0x547D,
                0x101B,
                [0x8E, 0x65, 0x08, 0x00, 0x2B, 0x2B, 0xD1, 0x19],
            ),
        ),
        variable(
            "_IID_ICreateErrorInfo",
            114,
            &guid(
                0x22F0_3340,
                0x547D,
                0x101B,
                [0x8E, 0x65, 0x08, 0x00, 0x2B, 0x2B, 0xD1, 0x19],
            ),
        ),
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
        rust("VarUI1FromI2", 116, "wp", var_from),
        rust("VarUI1FromI4", 117, "lp", var_from),
        rust("VarUI1FromR4", 118, "lp", var_from),
        rust("VarUI1FromR8", 119, "llp", var_from),
        rust("VarUI1FromCy", 120, "llp", var_from),
        rust("VarUI1FromDate", 121, "llp", var_from),
        rust("VarUI1FromStr", 122, "pllp", var_from),
        missing("VarUI1FromDisp", 123),
        rust("VarUI1FromBool", 124, "wp", var_from),
        rust("VarI2FromUI1", 125, "wp", var_from),
        rust("VarI4FromUI1", 126, "wp", var_from),
        rust("VarR4FromUI1", 127, "wp", var_from),
        rust("VarR8FromUI1", 128, "wp", var_from),
        rust("VarDateFromUI1", 129, "wp", var_from),
        rust("VarCyFromUI1", 130, "wp", var_from),
        rust("VarBstrFromUI1", 131, "wllp", var_from),
        rust("VarBoolFromUI1", 132, "wp", var_from),
        // No object of the program's can be unloaded while it runs.
        rust("DllCanUnloadNow", 133, "", s_false),
    ],
    library: true,
};

fn s_false(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(S_FALSE)
}

// BSTRs.

/// A BSTR of the bytes `text`: its 16:16 address, or 0.
pub fn alloc_bstr(cpu: &mut Cpu, text: &[u8]) -> u32 {
    let len = text.len() as u32;
    let block = ole2::alloc(cpu, len + 5);
    if block == 0 {
        return 0;
    }
    let at = super::linear(cpu, block);
    let mut bytes = len.to_le_bytes().to_vec();
    bytes.extend_from_slice(text);
    bytes.push(0);
    super::poke(cpu, at, &bytes);
    block + 4
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

pub(super) fn free_bstr(cpu: &mut Cpu, bstr: u32) {
    if bstr != 0 && bstr as u16 >= 4 {
        ole2::free(cpu, bstr - 4);
    }
}

/// SysAllocString(sz): a BSTR of the string, NULL for none.
fn sys_alloc_string(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if args[0] == 0 {
        return Some(0);
    }
    let text = super::string_bytes(cpu, args[0], 0xFFF0);
    Some(alloc_bstr(cpu, &text))
}

/// SysAllocStringLen(pch, cch): a BSTR of `cch` bytes from `pch`, of
/// zeros when it is NULL.
fn sys_alloc_string_len(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let len = args[1] as u16 as usize;
    let mut text = if args[0] == 0 {
        Vec::new()
    } else {
        super::peek_upto(cpu, args[0], len)
    };
    text.resize(len, 0);
    Some(alloc_bstr(cpu, &text))
}

/// Put a new BSTR of `text` at `pbstr`, freeing the one there: TRUE, or
/// FALSE (and the old one kept) when there is no memory.
fn replace_bstr(cpu: &mut Cpu, pbstr: u32, text: &[u8]) -> u32 {
    if pbstr == 0 {
        return 0;
    }
    let new = alloc_bstr(cpu, text);
    if new == 0 {
        return 0;
    }
    let old = super::read_32(cpu, pbstr);
    free_bstr(cpu, old);
    super::write_32(cpu, pbstr, new);
    1
}

fn sys_realloc_string(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let text = super::string_bytes(cpu, args[1], 0xFFF0);
    Some(replace_bstr(cpu, args[0], &text))
}

fn sys_realloc_string_len(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let len = args[2] as u16 as usize;
    let mut text = if args[1] == 0 {
        Vec::new()
    } else {
        super::peek_upto(cpu, args[1], len)
    };
    text.resize(len, 0);
    Some(replace_bstr(cpu, args[0], &text))
}

fn sys_free_string(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    free_bstr(cpu, args[0]);
    Some(0)
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
/// it is cleared: its BSTR freed here, its interface's Release returned
/// to be called, its array destroyed.
fn clear_value(cpu: &mut Cpu, vt: u16, value: u32, calls: &mut Vec<(u32, u32)>) {
    if vt & VT_BYREF != 0 {
        return;
    }
    if vt & VT_ARRAY != 0 {
        destroy_array(cpu, value, calls);
        return;
    }
    match vt {
        VT_BSTR => free_bstr(cpu, value),
        VT_DISPATCH | VT_UNKNOWN if value != 0 => calls.push((value, 2)),
        _ => {}
    }
}

fn clear(cpu: &mut Cpu, at: u32, calls: &mut Vec<(u32, u32)>) {
    let vt = super::read_16(cpu, at);
    let value = super::read_32(cpu, at + 8);
    clear_value(cpu, vt, value, calls);
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
fn variant_clear(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if args[0] == 0 {
        return Some(E_INVALIDARG);
    }
    let vt = super::read_16(cpu, args[0]);
    if !valid_vt(vt) {
        return Some(DISP_E_BADVARTYPE);
    }
    let mut calls = Vec::new();
    clear(cpu, args[0], &mut calls);
    call_all(cpu, S_OK, &calls)
}

/// Put the value `out` in the VARIANT at `dest` (flat), after clearing
/// it: the calls to make.
fn store(cpu: &mut Cpu, dest: u32, out: Out, calls: &mut Vec<(u32, u32)>) -> Result<(), u32> {
    let (vt, value) = match out {
        Out::Bits(vt, b) => (vt, b),
        Out::Str(u) => match alloc_bstr(cpu, &ansi(&u)) {
            0 => return Err(E_OUTOFMEMORY),
            s => (VT_BSTR, s as u64),
        },
        Out::Iface(vt, p) => {
            if p != 0 {
                calls.push((p, 1));
            }
            (vt, p as u64)
        }
    };
    clear(cpu, dest, calls);
    super::write_16(cpu, dest, vt);
    super::poke(cpu, dest + 8, &value.to_le_bytes());
    Ok(())
}

/// VariantCopy(dest, src): a BSTR copied, an interface with a reference
/// added, an array copied, a VT_BYREF pointer as it is.
fn variant_copy(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    copy(cpu, args[0], args[1], false)
}

/// VariantCopyInd(dest, src): a copy of what a VT_BYREF VARIANT points
/// at, or VariantCopy.
fn variant_copy_ind(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    copy(cpu, args[0], args[1], true)
}

fn copy(cpu: &mut Cpu, dest: u32, src: u32, indirect: bool) -> Option<u32> {
    if dest == 0 || src == 0 {
        return Some(E_INVALIDARG);
    }
    let vt = super::read_16(cpu, src);
    if !valid_vt(vt) {
        return Some(DISP_E_BADVARTYPE);
    }
    let mut value = super::read_32(cpu, src + 8);
    let mut bits = super::peek(cpu, src + 8, 8).unwrap_or_else(|| vec![0; 8]);
    let mut base = vt;
    if indirect && vt & VT_BYREF != 0 {
        base = vt & !VT_BYREF;
        let at = super::linear(cpu, value);
        if base == VT_VARIANT {
            return copy(cpu, dest, at, false);
        }
        bits = super::peek(cpu, at, 8).unwrap_or_else(|| vec![0; 8]);
        value = u32::from_le_bytes([bits[0], bits[1], bits[2], bits[3]]);
    } else if dest == src {
        return Some(S_OK);
    }
    let mut calls = Vec::new();
    let new = if base & VT_BYREF != 0 {
        value
    } else if base & VT_ARRAY != 0 {
        match copy_array(cpu, value, &mut calls) {
            Ok(a) => a,
            Err(e) => return Some(e),
        }
    } else {
        match base {
            VT_BSTR if value != 0 => match alloc_bstr(cpu, &bstr_bytes(cpu, value)) {
                0 => return Some(E_OUTOFMEMORY),
                s => s,
            },
            VT_DISPATCH | VT_UNKNOWN => {
                if value != 0 {
                    calls.push((value, 1));
                }
                value
            }
            _ => value,
        }
    };
    clear(cpu, dest, &mut calls);
    super::write_16(cpu, dest, base);
    super::poke(cpu, dest + 8, &bits);
    super::write_32(cpu, dest + 8, new);
    call_all(cpu, S_OK, &calls)
}

fn variant_change_type(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    change_type(cpu, args[0], args[1], args[2], args[3] as u16)
}

/// VariantChangeTypeEx(dest, src, lcid, flags, vt): the locale's own
/// formats aren't taken; those of English (United States) are, as the
/// built-in OLEAUT32's.
fn variant_change_type_ex(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    change_type(cpu, args[0], args[1], args[3], args[4] as u16)
}

/// VariantChangeType(dest, src, flags, vt).
fn change_type(cpu: &mut Cpu, dest: u32, src: u32, flags: u32, vt: u16) -> Option<u32> {
    if dest == 0 || src == 0 {
        return Some(E_INVALIDARG);
    }
    let result = changed(cpu, src, vt, flags);
    let mut calls = Vec::new();
    match result.and_then(|out| store(cpu, dest, out, &mut calls)) {
        Ok(()) => call_all(cpu, S_OK, &calls),
        Err(e) => Some(e),
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
fn var_from(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let name = current_function(cpu);
    let (to, from) = name
        .strip_prefix("Var")
        .and_then(|n| n.split_once("From"))
        .expect("a VarXxxFromYyy");
    let (to, from) = (vt_named(to)?, vt_named(from)?);
    let out = *args.last().expect("an out pointer");
    if out == 0 {
        return Some(E_INVALIDARG);
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
                return Some(E_INVALIDARG);
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
            Some(S_OK)
        }
        Ok(Out::Str(u)) => match alloc_bstr(cpu, &ansi(&u)) {
            0 => Some(E_OUTOFMEMORY),
            s => {
                super::write_32(cpu, out, s);
                Some(S_OK)
            }
        },
        Ok(Out::Iface(..)) => Some(DISP_E_TYPEMISMATCH),
        Err(e) => Some(e),
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
fn alloc_descriptor(cpu: &mut Cpu, dims: u16) -> u32 {
    let size = SA_HEADER + SA_BOUND * dims as u32;
    let p = ole2::alloc(cpu, size);
    if p != 0 {
        let at = super::linear(cpu, p);
        super::poke(cpu, at, &vec![0; size as usize]);
        super::write_16(cpu, at, dims);
    }
    p
}

/// The data of the array described at `a`, zeroed: false when there is
/// no memory.
fn alloc_data(cpu: &mut Cpu, a: &Array) -> bool {
    let size = a.bytes();
    let p = ole2::alloc(cpu, size.max(1));
    if p == 0 {
        return false;
    }
    let mut done = 0;
    while done < size {
        let n = (size - done).min(0x1000);
        let at = huge(cpu, p, done);
        super::poke(cpu, at, &vec![0; n as usize]);
        done += n;
    }
    super::write_32(cpu, a.at + 12, p);
    true
}

/// SafeArrayCreate(vt, cDims, rgsabound): a descriptor and its data,
/// zeroed.
fn safe_array_create(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [vt, dims, bounds] = args[..] else {
        unreachable!()
    };
    let (vt, dims) = (vt as u16, dims as u16);
    let Some((size, features)) = element(vt) else {
        return Some(0);
    };
    if dims == 0 || bounds == 0 {
        return Some(0);
    }
    let psa = alloc_descriptor(cpu, dims);
    if psa == 0 {
        return Some(0);
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
    let a = Array::read(cpu, psa)?;
    if !alloc_data(cpu, &a) {
        ole2::free(cpu, psa);
        return Some(0);
    }
    Some(psa)
}

fn safe_array_alloc_descriptor(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [dims, out] = args[..] else {
        unreachable!()
    };
    if dims as u16 == 0 || out == 0 {
        return Some(E_INVALIDARG);
    }
    match alloc_descriptor(cpu, dims as u16) {
        0 => Some(E_OUTOFMEMORY),
        psa => {
            super::write_32(cpu, out, psa);
            Some(S_OK)
        }
    }
}

fn safe_array_alloc_data(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(a) = Array::read(cpu, args[0]) else {
        return Some(E_INVALIDARG);
    };
    Some(if alloc_data(cpu, &a) {
        S_OK
    } else {
        E_OUTOFMEMORY
    })
}

/// The elements' BSTRs freed, interfaces' releases added to `calls`,
/// VARIANTs cleared.
fn clear_elements(cpu: &mut Cpu, a: &Array, calls: &mut Vec<(u32, u32)>) {
    if a.data == 0 || a.features & (FADF_BSTR | FADF_UNKNOWN | FADF_DISPATCH | FADF_VARIANT) == 0 {
        return;
    }
    for cell in 0..a.cells() {
        let at = a.data_at(cpu, cell * a.elem as u32);
        if a.features & FADF_VARIANT != 0 {
            clear(cpu, at, calls);
            continue;
        }
        let p = super::read_32(cpu, at);
        if a.features & FADF_BSTR != 0 {
            free_bstr(cpu, p);
        } else if p != 0 {
            calls.push((p, 2));
        }
        super::write_32(cpu, at, 0);
    }
}

/// Destroy the array at `psa` and its data, as SafeArrayDestroy does.
fn destroy_array(cpu: &mut Cpu, psa: u32, calls: &mut Vec<(u32, u32)>) {
    if let Some(a) = Array::read(cpu, psa) {
        clear_elements(cpu, &a, calls);
        if a.data != 0 {
            ole2::free(cpu, a.data);
        }
        ole2::free(cpu, psa);
    }
}

fn safe_array_destroy(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(a) = Array::read(cpu, args[0]) else {
        return Some(if args[0] == 0 { S_OK } else { E_INVALIDARG });
    };
    if a.locks > 0 {
        return Some(DISP_E_ARRAYISLOCKED);
    }
    let mut calls = Vec::new();
    destroy_array(cpu, args[0], &mut calls);
    call_all(cpu, S_OK, &calls)
}

fn safe_array_destroy_data(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(a) = Array::read(cpu, args[0]) else {
        return Some(E_INVALIDARG);
    };
    if a.locks > 0 {
        return Some(DISP_E_ARRAYISLOCKED);
    }
    let mut calls = Vec::new();
    clear_elements(cpu, &a, &mut calls);
    if a.data != 0 {
        ole2::free(cpu, a.data);
        super::write_32(cpu, a.at + 12, 0);
    }
    call_all(cpu, S_OK, &calls)
}

fn safe_array_destroy_descriptor(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(a) = Array::read(cpu, args[0]) else {
        return Some(E_INVALIDARG);
    };
    if a.locks > 0 {
        return Some(DISP_E_ARRAYISLOCKED);
    }
    ole2::free(cpu, args[0]);
    Some(S_OK)
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
fn safe_array_get_element(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(a) = Array::read(cpu, args[0]) else {
        return Some(E_INVALIDARG);
    };
    let out = super::linear(cpu, args[2]);
    if out == 0 {
        return Some(E_INVALIDARG);
    }
    let Some((_, at)) = element_ptr(cpu, &a, args[1]) else {
        return Some(DISP_E_BADINDEX);
    };
    if a.features & FADF_VARIANT != 0 {
        super::poke(cpu, out, &[0; 16]);
        return copy(cpu, out, at, false);
    }
    let bytes = super::peek(cpu, at, a.elem as usize).unwrap_or_default();
    let mut calls = Vec::new();
    if a.features & FADF_BSTR != 0 {
        let s = super::read_32(cpu, at);
        let new = if s == 0 {
            0
        } else {
            match alloc_bstr(cpu, &bstr_bytes(cpu, s)) {
                0 => return Some(E_OUTOFMEMORY),
                n => n,
            }
        };
        super::write_32(cpu, out, new);
        return Some(S_OK);
    }
    if a.features & (FADF_UNKNOWN | FADF_DISPATCH) != 0 {
        let p = super::read_32(cpu, at);
        if p != 0 {
            calls.push((p, 1));
        }
    }
    super::poke(cpu, out, &bytes);
    call_all(cpu, S_OK, &calls)
}

/// SafeArrayPutElement(psa, rgIndices, pv): the value at `pv` (16:16)
/// put in the element, whose old value is freed. For an array of BSTRs
/// or interfaces, `pv` is the BSTR or interface itself.
fn safe_array_put_element(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(a) = Array::read(cpu, args[0]) else {
        return Some(E_INVALIDARG);
    };
    let Some((_, at)) = element_ptr(cpu, &a, args[1]) else {
        return Some(DISP_E_BADINDEX);
    };
    let pv = args[2];
    let mut calls = Vec::new();
    if a.features & FADF_VARIANT != 0 {
        let src = super::linear(cpu, pv);
        return copy(cpu, at, src, false);
    }
    if a.features & FADF_BSTR != 0 {
        let new = if pv == 0 {
            0
        } else {
            match alloc_bstr(cpu, &bstr_bytes(cpu, pv)) {
                0 => return Some(E_OUTOFMEMORY),
                n => n,
            }
        };
        let old = super::read_32(cpu, at);
        free_bstr(cpu, old);
        super::write_32(cpu, at, new);
        return Some(S_OK);
    }
    if a.features & (FADF_UNKNOWN | FADF_DISPATCH) != 0 {
        let old = super::read_32(cpu, at);
        if pv != 0 {
            calls.push((pv, 1));
        }
        if old != 0 {
            calls.push((old, 2));
        }
        super::write_32(cpu, at, pv);
        return call_all(cpu, S_OK, &calls);
    }
    let from = super::linear(cpu, pv);
    if from == 0 {
        return Some(E_INVALIDARG);
    }
    let bytes = super::peek(cpu, from, a.elem as usize).unwrap_or_default();
    super::poke(cpu, at, &bytes);
    Some(S_OK)
}

/// A copy of the array at `psa` (16:16) and its elements: the copy's
/// 16:16 address, the AddRefs to make added to `calls`.
fn copy_array(cpu: &mut Cpu, psa: u32, calls: &mut Vec<(u32, u32)>) -> Result<u32, u32> {
    if psa == 0 {
        return Ok(0);
    }
    let a = Array::read(cpu, psa).ok_or(E_INVALIDARG)?;
    let size = SA_HEADER + SA_BOUND * a.dims as u32;
    let new = ole2::alloc(cpu, size);
    if new == 0 {
        return Err(E_OUTOFMEMORY);
    }
    let to = super::linear(cpu, new);
    let head = super::peek(cpu, a.at, size as usize).unwrap_or_default();
    super::poke(cpu, to, &head);
    super::write_16(cpu, to + 6, 0);
    super::write_32(cpu, to + 12, 0);
    if a.data == 0 {
        return Ok(new);
    }
    let b = Array::read(cpu, new).ok_or(E_INVALIDARG)?;
    if !alloc_data(cpu, &b) {
        ole2::free(cpu, new);
        return Err(E_OUTOFMEMORY);
    }
    let b = Array::read(cpu, new).ok_or(E_INVALIDARG)?;
    for cell in 0..a.cells() {
        let offset = cell * a.elem as u32;
        let from = a.data_at(cpu, offset);
        let to = b.data_at(cpu, offset);
        if a.features & FADF_VARIANT != 0 {
            let vt = super::read_16(cpu, from);
            let value = super::read_32(cpu, from + 8);
            let mut bytes = super::peek(cpu, from, 16).unwrap_or_else(|| vec![0; 16]);
            let new_value = match vt {
                VT_BSTR if value != 0 => alloc_bstr(cpu, &bstr_bytes(cpu, value)),
                VT_DISPATCH | VT_UNKNOWN if value != 0 => {
                    calls.push((value, 1));
                    value
                }
                _ if vt & VT_ARRAY != 0 && vt & VT_BYREF == 0 => copy_array(cpu, value, calls)?,
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
            let s = alloc_bstr(cpu, &bstr_bytes(cpu, p));
            super::write_32(cpu, to, s);
        } else if a.features & (FADF_UNKNOWN | FADF_DISPATCH) != 0 && p != 0 {
            calls.push((p, 1));
        }
    }
    Ok(new)
}

fn safe_array_copy(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if args[1] == 0 {
        return Some(E_INVALIDARG);
    }
    let mut calls = Vec::new();
    match copy_array(cpu, args[0], &mut calls) {
        Ok(p) => {
            super::write_32(cpu, args[1], p);
            call_all(cpu, S_OK, &calls)
        }
        Err(e) => {
            super::write_32(cpu, args[1], 0);
            Some(e)
        }
    }
}

/// SafeArrayRedim(psa, psaboundNew): the last dimension's bound changed,
/// the data grown (zeroed) or cut (the elements cut freed).
fn safe_array_redim(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(a) = Array::read(cpu, args[0]) else {
        return Some(E_INVALIDARG);
    };
    if args[1] == 0 || a.dims == 0 {
        return Some(E_INVALIDARG);
    }
    if a.locks > 0 {
        return Some(DISP_E_ARRAYISLOCKED);
    }
    let count = super::read_32(cpu, args[1]);
    let lower = super::read_32(cpu, args[1] + 4);
    let old_bytes = a.bytes();
    let last = a.bounds[a.dims as usize - 1].0;
    let per = old_bytes.checked_div(last).unwrap_or(0);
    let new_bytes = per.saturating_mul(count);
    let mut calls = Vec::new();
    let owned = FADF_BSTR | FADF_UNKNOWN | FADF_DISPATCH | FADF_VARIANT;
    if new_bytes < old_bytes && a.features & owned != 0 {
        // The elements cut go.
        let elem = a.elem.max(1) as u32;
        for cell in 0..(old_bytes - new_bytes) / elem {
            let at = a.data_at(cpu, new_bytes + cell * elem);
            if a.features & FADF_VARIANT != 0 {
                clear(cpu, at, &mut calls);
                continue;
            }
            let p = super::read_32(cpu, at);
            if a.features & FADF_BSTR != 0 {
                free_bstr(cpu, p);
            } else if p != 0 {
                calls.push((p, 2));
            }
        }
    }
    let data = ole2::realloc(cpu, a.data, new_bytes.max(1));
    if data == 0 {
        return Some(E_OUTOFMEMORY);
    }
    let mut done = old_bytes;
    while done < new_bytes {
        let n = (new_bytes - done).min(0x1000);
        let at = huge(cpu, data, done);
        super::poke(cpu, at, &vec![0; n as usize]);
        done += n;
    }
    super::write_32(cpu, a.at + 12, data);
    super::write_32(cpu, a.at + SA_HEADER, count);
    super::write_32(cpu, a.at + SA_HEADER + 4, lower);
    call_all(cpu, S_OK, &calls)
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
