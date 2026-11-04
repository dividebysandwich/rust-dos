//! OLEAUT32.DLL: the BSTR functions and VARIANTs' simplest ones, which
//! setup programs and OLE32's PROPVARIANTs use; type libraries
//! (typelib.rs), which controls register themselves with; error objects
//! (errorinfo.rs); and changes of VARIANTs' types (variant.rs). A BSTR is
//! a string of Unicode characters whose length in bytes is in the 4
//! bytes before it, and a zero after it, in a block of the task
//! allocator, as Windows has it.

mod desc;
mod errorinfo;
mod model;
mod msft;
mod sltg;
mod stdole;
#[cfg(test)]
mod tests;
mod typelib;
mod variant;

use super::stubs::{Api, Conv, Dll, Vtable};
use crate::cpu::Cpu;
use crate::win32::task::{self, Next, Task};

pub static DLL: Dll = Dll {
    name: "OLEAUT32.DLL",
    base: 0xBFE6_0000,
    apis: APIS,
    vars: &[],
    init: None,
    term: None,
    vtables: VTABLES,
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

/// The functions, by OLEAUT32's ordinals. Append only (see `stubs`).
static APIS: &[Api] = &[
    stdcall("SysAllocString", 2, 1, sys_alloc_string),
    stdcall("SysReAllocString", 3, 2, sys_realloc_string),
    stdcall("SysAllocStringLen", 4, 2, sys_alloc_string_len),
    stdcall("SysReAllocStringLen", 5, 3, sys_realloc_string_len),
    stdcall("SysFreeString", 6, 1, sys_free_string),
    stdcall("SysStringLen", 7, 1, sys_string_len),
    stdcall("VariantInit", 8, 1, variant_init),
    stdcall("VariantClear", 9, 1, variant_clear),
    stdcall("SysStringByteLen", 149, 1, sys_string_byte_len),
    stdcall("SysAllocStringByteLen", 150, 2, sys_alloc_string_byte_len),
    stdcall("VariantCopy", 10, 2, variant::variant_copy),
    stdcall("VariantChangeType", 12, 4, variant::variant_change_type),
    stdcall("DispGetIDsOfNames", 29, 4, disp_get_ids_of_names),
    stdcall("VariantChangeTypeEx", 147, 5, variant::variant_change_type_ex),
    stdcall("LoadTypeLib", 161, 2, typelib::load_type_lib),
    stdcall("LoadRegTypeLib", 162, 5, typelib::load_reg_type_lib),
    stdcall("RegisterTypeLib", 163, 3, typelib::register_type_lib),
    stdcall(
        "QueryPathOfRegTypeLib",
        164,
        5,
        typelib::query_path_of_reg_type_lib,
    ),
    stdcall("LoadTypeLibEx", 183, 3, typelib::load_type_lib_ex),
    stdcall("UnRegisterTypeLib", 186, 5, typelib::unregister_type_lib),
    stdcall("GetErrorInfo", 200, 2, errorinfo::get_error_info),
    stdcall("SetErrorInfo", 201, 2, errorinfo::set_error_info),
    stdcall("CreateErrorInfo", 202, 1, errorinfo::create_error_info),
    stdcall("ITypeLib_QueryInterface", 0, 3, typelib::lib_query_interface),
    stdcall("ITypeLib_AddRef", 0, 1, typelib::lib_add_ref),
    stdcall("ITypeLib_Release", 0, 1, typelib::lib_release),
    stdcall("ITypeLib_GetTypeInfoCount", 0, 1, typelib::get_type_info_count),
    stdcall("ITypeLib_GetTypeInfo", 0, 3, typelib::get_type_info),
    stdcall("ITypeLib_GetTypeInfoType", 0, 3, typelib::get_type_info_type),
    stdcall("ITypeLib_GetTypeInfoOfGuid", 0, 3, typelib::get_type_info_of_guid),
    stdcall("ITypeLib_GetLibAttr", 0, 2, typelib::get_lib_attr),
    stdcall("ITypeLib_GetTypeComp", 0, 2, typelib::get_type_comp),
    stdcall("ITypeLib_GetDocumentation", 0, 6, typelib::lib_get_documentation),
    stdcall("ITypeLib_IsName", 0, 4, typelib::is_name),
    stdcall("ITypeLib_FindName", 0, 6, typelib::find_name),
    stdcall("ITypeLib_ReleaseTLibAttr", 0, 2, typelib::release_attr),
    stdcall("ITypeLib2_GetCustData", 0, 3, typelib::get_cust_data),
    stdcall("ITypeLib2_GetLibStatistics", 0, 3, typelib::get_lib_statistics),
    stdcall(
        "ITypeLib2_GetDocumentation2",
        0,
        6,
        typelib::lib_get_documentation2,
    ),
    stdcall("ITypeLib2_GetAllCustData", 0, 2, typelib::get_all_cust_data),
    stdcall("ITypeInfo_QueryInterface", 0, 3, typelib::info_query_interface),
    stdcall("ITypeInfo_AddRef", 0, 1, typelib::info_add_ref),
    stdcall("ITypeInfo_Release", 0, 1, typelib::info_release),
    stdcall("ITypeInfo_GetTypeAttr", 0, 2, typelib::get_type_attr),
    stdcall("ITypeInfo_GetTypeComp", 0, 2, typelib::get_type_comp),
    stdcall("ITypeInfo_GetFuncDesc", 0, 3, typelib::get_func_desc),
    stdcall("ITypeInfo_GetVarDesc", 0, 3, typelib::get_var_desc),
    stdcall("ITypeInfo_GetNames", 0, 5, typelib::get_names),
    stdcall(
        "ITypeInfo_GetRefTypeOfImplType",
        0,
        3,
        typelib::get_ref_type_of_impl_type,
    ),
    stdcall("ITypeInfo_GetImplTypeFlags", 0, 3, typelib::get_impl_type_flags),
    stdcall("ITypeInfo_GetIDsOfNames", 0, 4, typelib::get_ids_of_names),
    stdcall("ITypeInfo_Invoke", 0, 8, typelib::invoke),
    stdcall(
        "ITypeInfo_GetDocumentation",
        0,
        6,
        typelib::info_get_documentation,
    ),
    stdcall("ITypeInfo_GetDllEntry", 0, 6, typelib::get_dll_entry),
    stdcall("ITypeInfo_GetRefTypeInfo", 0, 3, typelib::get_ref_type_info),
    stdcall("ITypeInfo_AddressOfMember", 0, 4, typelib::not_yet),
    stdcall("ITypeInfo_CreateInstance", 0, 4, typelib::not_yet),
    stdcall("ITypeInfo_GetMops", 0, 3, typelib::get_mops),
    stdcall(
        "ITypeInfo_GetContainingTypeLib",
        0,
        3,
        typelib::get_containing_type_lib,
    ),
    stdcall("ITypeInfo_ReleaseTypeAttr", 0, 2, typelib::release_attr),
    stdcall("ITypeInfo_ReleaseFuncDesc", 0, 2, typelib::release_func_desc),
    stdcall("ITypeInfo_ReleaseVarDesc", 0, 2, typelib::release_var_desc),
    stdcall("ITypeInfo2_GetTypeKind", 0, 2, typelib::get_type_kind),
    stdcall("ITypeInfo2_GetTypeFlags", 0, 2, typelib::get_type_flags),
    stdcall(
        "ITypeInfo2_GetFuncIndexOfMemId",
        0,
        4,
        typelib::get_func_index_of_mem_id,
    ),
    stdcall(
        "ITypeInfo2_GetVarIndexOfMemId",
        0,
        3,
        typelib::get_var_index_of_mem_id,
    ),
    stdcall("ITypeInfo2_GetCustData", 0, 3, typelib::get_cust_data),
    stdcall("ITypeInfo2_GetFuncCustData", 0, 4, typelib::get_cust_data),
    stdcall("ITypeInfo2_GetParamCustData", 0, 5, typelib::get_cust_data),
    stdcall("ITypeInfo2_GetVarCustData", 0, 4, typelib::get_cust_data),
    stdcall("ITypeInfo2_GetImplTypeCustData", 0, 4, typelib::get_cust_data),
    stdcall(
        "ITypeInfo2_GetDocumentation2",
        0,
        6,
        typelib::info_get_documentation2,
    ),
    stdcall("ITypeInfo2_GetAllCustData", 0, 2, typelib::get_all_cust_data),
    stdcall("ITypeInfo2_GetAllFuncCustData", 0, 3, typelib::get_all_cust_data),
    stdcall("ITypeInfo2_GetAllParamCustData", 0, 4, typelib::get_all_cust_data),
    stdcall("ITypeInfo2_GetAllVarCustData", 0, 3, typelib::get_all_cust_data),
    stdcall(
        "ITypeInfo2_GetAllImplTypeCustData",
        0,
        3,
        typelib::get_all_cust_data,
    ),
    stdcall("IErrorInfo_QueryInterface", 0, 3, errorinfo::query_interface),
    stdcall("IErrorInfo_AddRef", 0, 1, errorinfo::method_add_ref),
    stdcall("IErrorInfo_Release", 0, 1, errorinfo::method_release),
    stdcall("ICreateErrorInfo_SetGUID", 0, 2, errorinfo::set_guid),
    stdcall("ICreateErrorInfo_SetSource", 0, 2, errorinfo::set_source),
    stdcall(
        "ICreateErrorInfo_SetDescription",
        0,
        2,
        errorinfo::set_description,
    ),
    stdcall("ICreateErrorInfo_SetHelpFile", 0, 2, errorinfo::set_help_file),
    stdcall(
        "ICreateErrorInfo_SetHelpContext",
        0,
        2,
        errorinfo::set_help_context,
    ),
    stdcall("IErrorInfo_GetGUID", 0, 2, errorinfo::get_guid),
    stdcall("IErrorInfo_GetSource", 0, 2, errorinfo::get_source),
    stdcall("IErrorInfo_GetDescription", 0, 2, errorinfo::get_description),
    stdcall("IErrorInfo_GetHelpFile", 0, 2, errorinfo::get_help_file),
    stdcall("IErrorInfo_GetHelpContext", 0, 2, errorinfo::get_help_context),
];

/// The vtables, by their places in this list. ITypeLib's and ITypeInfo's
/// are ITypeLib2's and ITypeInfo2's, as Windows 98's objects have them.
static VTABLES: &[Vtable] = &[
    Vtable {
        name: "ITypeLib",
        methods: &[
            "ITypeLib_QueryInterface",
            "ITypeLib_AddRef",
            "ITypeLib_Release",
            "ITypeLib_GetTypeInfoCount",
            "ITypeLib_GetTypeInfo",
            "ITypeLib_GetTypeInfoType",
            "ITypeLib_GetTypeInfoOfGuid",
            "ITypeLib_GetLibAttr",
            "ITypeLib_GetTypeComp",
            "ITypeLib_GetDocumentation",
            "ITypeLib_IsName",
            "ITypeLib_FindName",
            "ITypeLib_ReleaseTLibAttr",
            "ITypeLib2_GetCustData",
            "ITypeLib2_GetLibStatistics",
            "ITypeLib2_GetDocumentation2",
            "ITypeLib2_GetAllCustData",
        ],
    },
    Vtable {
        name: "ITypeInfo",
        methods: &[
            "ITypeInfo_QueryInterface",
            "ITypeInfo_AddRef",
            "ITypeInfo_Release",
            "ITypeInfo_GetTypeAttr",
            "ITypeInfo_GetTypeComp",
            "ITypeInfo_GetFuncDesc",
            "ITypeInfo_GetVarDesc",
            "ITypeInfo_GetNames",
            "ITypeInfo_GetRefTypeOfImplType",
            "ITypeInfo_GetImplTypeFlags",
            "ITypeInfo_GetIDsOfNames",
            "ITypeInfo_Invoke",
            "ITypeInfo_GetDocumentation",
            "ITypeInfo_GetDllEntry",
            "ITypeInfo_GetRefTypeInfo",
            "ITypeInfo_AddressOfMember",
            "ITypeInfo_CreateInstance",
            "ITypeInfo_GetMops",
            "ITypeInfo_GetContainingTypeLib",
            "ITypeInfo_ReleaseTypeAttr",
            "ITypeInfo_ReleaseFuncDesc",
            "ITypeInfo_ReleaseVarDesc",
            "ITypeInfo2_GetTypeKind",
            "ITypeInfo2_GetTypeFlags",
            "ITypeInfo2_GetFuncIndexOfMemId",
            "ITypeInfo2_GetVarIndexOfMemId",
            "ITypeInfo2_GetCustData",
            "ITypeInfo2_GetFuncCustData",
            "ITypeInfo2_GetParamCustData",
            "ITypeInfo2_GetVarCustData",
            "ITypeInfo2_GetImplTypeCustData",
            "ITypeInfo2_GetDocumentation2",
            "ITypeInfo2_GetAllCustData",
            "ITypeInfo2_GetAllFuncCustData",
            "ITypeInfo2_GetAllParamCustData",
            "ITypeInfo2_GetAllVarCustData",
            "ITypeInfo2_GetAllImplTypeCustData",
        ],
    },
    Vtable {
        name: "ICreateErrorInfo",
        methods: &[
            "IErrorInfo_QueryInterface",
            "IErrorInfo_AddRef",
            "IErrorInfo_Release",
            "ICreateErrorInfo_SetGUID",
            "ICreateErrorInfo_SetSource",
            "ICreateErrorInfo_SetDescription",
            "ICreateErrorInfo_SetHelpFile",
            "ICreateErrorInfo_SetHelpContext",
        ],
    },
    Vtable {
        name: "IErrorInfo",
        methods: &[
            "IErrorInfo_QueryInterface",
            "IErrorInfo_AddRef",
            "IErrorInfo_Release",
            "IErrorInfo_GetGUID",
            "IErrorInfo_GetSource",
            "IErrorInfo_GetDescription",
            "IErrorInfo_GetHelpFile",
            "IErrorInfo_GetHelpContext",
        ],
    },
];

/// OLEAUT32's state: its objects, each kept with the process it belongs
/// to.
#[derive(Clone, Debug, Default)]
pub struct State {
    /// The type libraries and the ITypeInfos of their types, by their
    /// ids, and the libraries as the model has them.
    libs: Vec<Option<typelib::Lib>>,
    infos: Vec<Option<typelib::Info>>,
    parsed: typelib::Parsed,
    /// The error objects, by their ids, and each thread's: (process,
    /// thread, interface).
    errors: Vec<Option<errorinfo::ErrorObject>>,
    thread_errors: Vec<(u32, u32, u32)>,
}

crate::state_fields!(State {
    libs,
    infos,
    parsed,
    errors,
    thread_errors
});

fn dll_index() -> usize {
    super::stubs::DLLS
        .iter()
        .position(|d| d.name == DLL.name)
        .expect("OLEAUT32 is a built-in DLL")
}

/// The place of the vtable `name` in `VTABLES`.
fn vtable(name: &str) -> usize {
    VTABLES
        .iter()
        .position(|v| v.name == name)
        .expect("a vtable of OLEAUT32's")
}

/// The id of a free slot of `table`, filled with `item`.
fn insert<T>(table: &mut Vec<Option<T>>, item: T) -> usize {
    match table.iter().position(Option::is_none) {
        Some(i) => {
            table[i] = Some(item);
            i
        }
        None => {
            table.push(Some(item));
            table.len() - 1
        }
    }
}

/// A process ended: its objects went with its heap.
pub fn process_ended(cpu: &mut Cpu, pid: u32) {
    typelib::process_ended(cpu, pid);
    errorinfo::process_ended(cpu, pid);
}

/// A new BSTR of the string `s`, or 0 when memory is out.
fn bstr(cpu: &mut Cpu, s: &str) -> u32 {
    let units: Vec<u16> = s.encode_utf16().collect();
    let at = alloc(cpu, 0, 2 * units.len() as u32);
    if at != 0 {
        for (i, u) in units.iter().enumerate() {
            cpu.bus.guest_write_16(at + 2 * i as u32, *u);
        }
    }
    at
}

/// The Unicode string at `at`, None for NULL.
fn wide_string(cpu: &mut Cpu, at: u32) -> Option<String> {
    if at == 0 {
        return None;
    }
    let n = wide_len(cpu, at);
    let units: Vec<u16> = (0..n).map(|i| cpu.bus.guest_read_16(at + 2 * i)).collect();
    Some(String::from_utf16_lossy(&units))
}

/// `s` in the ANSI code page.
fn ansi(s: &str) -> Vec<u8> {
    let units: Vec<u16> = s.encode_utf16().collect();
    super::kernel32::wide_to_ansi(&units)
}

/// The Unicode path `s` as KERNEL32's functions take a path.
fn path_of(s: &str) -> String {
    crate::dosstr::from_bytes(&ansi(s))
}

/// DispGetIDsOfNames(type, names, count, ids): the type's GetIDsOfNames.
fn disp_get_ids_of_names(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[info, names, count, ids] = args else {
        unreachable!()
    };
    // One of these ITypeInfos: its method takes the same arguments.
    if typelib::info_of(cpu, info).is_some() {
        return typelib::get_ids_of_names(cpu, args);
    }
    // An ITypeInfo of the program's own.
    let Some(method) = super::ole32::method(cpu, info, 10) else {
        return Some(super::com::E_INVALIDARG);
    };
    task::start(cpu, task::OLEAUT32, vec![method, info, names, count, ids], 0)
}

/// The task of the functions that call into the program:
/// DispGetIDsOfNames calls the type's GetIDsOfNames and returns what it
/// does.
pub fn step(_: &mut Cpu, t: &mut Task, value: u32) -> Next {
    match t.step {
        0 => Next::call(t.var(0), &[t.var(1), t.var(2), t.var(3), t.var(4)], 1),
        _ => Next::Done(value),
    }
}

const S_OK: u32 = 0;
const DISP_E_BADVARTYPE: u32 = 0x8002_0008;

/// A new BSTR of `len` bytes, copied from `from` if it isn't 0: its
/// address, or 0 when memory is out.
fn alloc(cpu: &mut Cpu, from: u32, len: u32) -> u32 {
    let Some(size) = len.checked_add(6) else {
        return 0;
    };
    let block = super::ole32::task_alloc(cpu, size);
    if block == 0 {
        return 0;
    }
    let mut bytes = vec![0u8; size as usize];
    bytes[..4].copy_from_slice(&len.to_le_bytes());
    if from != 0 && cpu.bus.guest_probe(from, len as usize, false) {
        cpu.bus
            .guest_read_bytes(from, &mut bytes[4..4 + len as usize]);
    }
    cpu.bus.guest_write_bytes(block, &bytes);
    block + 4
}

/// A new BSTR of `text`, for a built-in object's method that gives one
/// (IVideoWindow::get_Caption): its address, or 0 when memory is out.
pub fn new_bstr(cpu: &mut Cpu, text: &str) -> u32 {
    let units: Vec<u8> = text.encode_utf16().flat_map(u16::to_le_bytes).collect();
    let at = alloc(cpu, 0, units.len() as u32);
    if at != 0 {
        cpu.bus.guest_write_bytes(at, &units);
    }
    at
}

/// The length in characters of the Unicode string at `at`.
fn wide_len(cpu: &mut Cpu, at: u32) -> u32 {
    let mut n = 0;
    while cpu.bus.guest_probe(at + 2 * n, 2, false) && cpu.bus.guest_read_16(at + 2 * n) != 0 {
        n += 1;
    }
    n
}

/// The length in bytes of the BSTR `s`.
fn byte_len(cpu: &mut Cpu, s: u32) -> u32 {
    if s < 4 || !cpu.bus.guest_probe(s - 4, 4, false) {
        return 0;
    }
    cpu.bus.guest_read_32(s - 4)
}

/// SysAllocString(text): NULL for NULL.
fn sys_alloc_string(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let text = args[0];
    if text == 0 {
        return Some(0);
    }
    let n = wide_len(cpu, text);
    Some(alloc(cpu, text, 2 * n))
}

/// SysAllocStringLen(text, characters): uninitialized when text is NULL.
fn sys_alloc_string_len(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[text, n] = args else { unreachable!() };
    if n >= 0x8000_0000 / 2 {
        return Some(0);
    }
    Some(alloc(cpu, text, 2 * n))
}

/// SysAllocStringByteLen(bytes, length).
fn sys_alloc_string_byte_len(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[text, n] = args else { unreachable!() };
    if n >= 0x8000_0000 {
        return Some(0);
    }
    Some(alloc(cpu, text, n))
}

/// SysReAllocString(place, text): a new BSTR at `place`, the old freed.
fn sys_realloc_string(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[place, text] = args else { unreachable!() };
    let n = if text == 0 { 0 } else { wide_len(cpu, text) };
    realloc(cpu, place, text, n)
}

/// SysReAllocStringLen(place, text, characters).
fn sys_realloc_string_len(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[place, text, n] = args else {
        unreachable!()
    };
    if n >= 0x8000_0000 / 2 {
        return Some(0);
    }
    realloc(cpu, place, text, n)
}

fn realloc(cpu: &mut Cpu, place: u32, text: u32, n: u32) -> Option<u32> {
    if place == 0 || !cpu.bus.guest_probe(place, 4, true) {
        return Some(0);
    }
    let new = alloc(cpu, text, 2 * n);
    if new == 0 {
        return Some(0);
    }
    let old = cpu.bus.guest_read_32(place);
    free(cpu, old);
    cpu.bus.guest_write_32(place, new);
    Some(1)
}

fn free(cpu: &mut Cpu, s: u32) {
    if s >= 4 {
        super::ole32::task_free(cpu, s - 4);
    }
}

fn sys_free_string(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    free(cpu, args[0]);
    Some(0)
}

fn sys_string_len(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(byte_len(cpu, args[0]) / 2)
}

fn sys_string_byte_len(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(byte_len(cpu, args[0]))
}

/// VariantInit(variant): VT_EMPTY.
fn variant_init(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if cpu.bus.guest_probe(args[0], 2, true) {
        cpu.bus.guest_write_16(args[0], 0);
    }
    Some(0)
}

/// VariantClear(variant): a BSTR is freed, an interface released.
/// Arrays (VT_ARRAY) aren't taken yet.
fn variant_clear(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let at = args[0];
    if !cpu.bus.guest_probe(at, 16, true) {
        return Some(S_OK);
    }
    let vt = cpu.bus.guest_read_16(at);
    let value = cpu.bus.guest_read_32(at + 8);
    let mut release = None;
    if vt & 0x4000 == 0 {
        match vt {
            // VT_BSTR.
            8 => free(cpu, value),
            // VT_DISPATCH, VT_UNKNOWN.
            9 | 13 => release = super::ole32::method(cpu, value, 2).map(|m| (m, value)),
            _ if vt & 0x2000 != 0 => return Some(DISP_E_BADVARTYPE),
            _ => {}
        }
    }
    cpu.bus.guest_write_16(at, 0);
    match release {
        Some(call) => super::ole32::call_all(cpu, S_OK, &[call]),
        None => Some(S_OK),
    }
}
