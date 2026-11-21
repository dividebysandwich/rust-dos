//! The 16-bit OLE 2 modules Windows 95 and 98 have in their SYSTEM
//! directory: COMPOBJ (the component object library: initialization,
//! the task allocator, class objects, GUIDs as text), STORAGE (compound
//! files), OLE2 (linking and embedding, drag and drop, the clipboard)
//! and OLE2NLS (locales); OLE2DISP (automation) has ole2disp.rs. A
//! program made with Visual Basic 4's 16-bit edition imports all of
//! them through VB40016.DLL.
//!
//! The ordinals and names are those of the modules of Windows 98 SE, as
//! Wine's compobj.dll16, storage.dll16, ole2.dll16 and ole2nls.dll16
//! spec files list them. What a program calls is done as OLE 2.02 does
//! it, much of it after Wine's implementations (LGPL 2.1 or later). The
//! rest is in two kinds. A function with a documented way to fail that a
//! program checks (compound files, which need IStorage objects of 16-bit
//! code's own) fails with E_NOTIMPL, saying so in the log. One without
//! (the OLE 1 conversion, the monikers, the data cache) is `missing`: a
//! call ends the program saying which function it was.
//!
//! Interfaces: a 16-bit interface's methods are far cdecl functions, the
//! object's 16:16 pointer first, as OLE 2's STDMETHODCALLTYPE has them
//! in Win16. The built-in task allocator (IMalloc) has methods that are
//! stubs of COMPOBJ that aren't exported, and its blocks come from local
//! heaps in global blocks of COMPOBJ's (`alloc`). A program that gives
//! CoInitialize an allocator of its own has that one as its task
//! allocator instead: CoGetMalloc gives it, and the strings COMPOBJ
//! makes for the program come from it (`task_block`). The methods of an
//! interface of the program's are called through its vtable
//! (`call_method`). What a task initialized, registered or was given is
//! its own (`OleTask`), and goes when it ends.
//!
//! Copyright 1995 Martin von Loewis
//! Copyright 1998 Justin Bradford
//! Copyright 1999 Francis Beaudet
//! Copyright 1999 Sylvain St-Germain
//! Copyright 2002 Marcus Meissner

use super::builtin::{Def, cdecl, missing, rust, variable, win32};
use super::{global, local, thunk};
use crate::cpu::Cpu;
use crate::dpmi::Context;
use crate::win32::com::{
    E_INVALIDARG, E_NOINTERFACE, E_NOTIMPL, E_OUTOFMEMORY, REGDB_E_CLASSNOTREG, S_FALSE, S_OK,
};

const K: &str = "KERNEL32.DLL";

pub const CO_E_NOTINITIALIZED: u32 = 0x8004_01F0;
const CO_E_CLASSSTRING: u32 = 0x8004_01F3;
const CO_E_OBJNOTREG: u32 = 0x8004_01FB;
const DRAGDROP_E_NOTREGISTERED: u32 = 0x8004_0100;
const DRAGDROP_E_ALREADYREGISTERED: u32 = 0x8004_0101;
const MEMCTX_TASK: u32 = 1;
const MEMCTX_SHARED: u32 = 2;

/// OLE 2's build version, which CoBuildVersion and OleBuildVersion
/// return: 23.700, as Windows 98 SE's COMPOBJ.DLL has it. Programs
/// compare it with the oldest they take: Visual Basic 4's runtime wants
/// 23.700 or later.
const RMM: u32 = 23;
const RUP: u32 = 700;

/// The GUIDs of OLE's own interfaces, {xxxxxxxx-0000-0000-C000-
/// 000000000046}, by their first part.
pub const fn ole_guid(d1: u32) -> [u8; 16] {
    guid(d1, 0, 0, [0xC0, 0, 0, 0, 0, 0, 0, 0x46])
}

/// A GUID's 16 bytes, as it is in memory.
pub const fn guid(d1: u32, d2: u16, d3: u16, d4: [u8; 8]) -> [u8; 16] {
    let a = d1.to_le_bytes();
    let b = d2.to_le_bytes();
    let c = d3.to_le_bytes();
    [
        a[0], a[1], a[2], a[3], b[0], b[1], c[0], c[1], d4[0], d4[1], d4[2], d4[3], d4[4], d4[5],
        d4[6], d4[7],
    ]
}

pub const IID_IUNKNOWN: [u8; 16] = ole_guid(0);
pub const IID_IMALLOC: [u8; 16] = ole_guid(2);

/// The W32 state version that first saves `Ole16`.
pub const OLE16: u16 = 84;

/// What the OLE modules keep of the Win16 process.
#[derive(Clone, Debug, Default)]
pub struct Ole16 {
    /// The tasks that called the modules, each with what is its own.
    pub tasks: Vec<OleTask>,
    /// The built-in task allocator's object (16:16), 0 before it is
    /// made.
    pub malloc: u32,
    /// The selectors of the global blocks the allocator's local heaps
    /// are in, and of the blocks too big for one, each a block of its
    /// own.
    pub heaps: Vec<u16>,
    pub large: Vec<u16>,
    /// The class objects registered (CoRegisterClassObject).
    pub classes: Vec<Class>,
    pub next_cookie: u32,
    /// The windows drag and drop targets were registered for, the
    /// targets, and the tasks that registered them.
    pub drop_targets: Vec<(u16, u32, u16)>,
    /// CoCreateGuid's generator.
    pub guid_seed: u64,
}

crate::state_fields!(Ole16 {
    tasks,
    malloc,
    heaps,
    large,
    classes,
    next_cookie,
    drop_targets,
    guid_seed
});

/// What COMPOBJ, OLE2 and OLE2DISP keep for one task, as the real ones
/// keep it in each task's own data: a task's initialization, allocator
/// and error object are its own, and go when it ends (`task_ended`).
#[derive(Clone, Debug, Default)]
pub struct OleTask {
    /// The task's handle.
    pub task: u16,
    /// How many times CoInitialize and OleInitialize have been called
    /// without their uninitialize.
    pub co_inits: u32,
    pub ole_inits: u32,
    /// The program's own allocator that CoInitialize was given, if any:
    /// the task allocator from then on.
    pub app_malloc: u32,
    /// The message filter CoRegisterMessageFilter registered.
    pub message_filter: u32,
    /// The error object of the last SetErrorInfo (OLE2DISP).
    pub error_info: u32,
}

crate::state_fields!(OleTask {
    task,
    co_inits,
    ole_inits,
    app_malloc,
    message_filter,
    error_info
});

/// A class object CoRegisterClassObject registered.
#[derive(Clone, Debug, Default)]
pub struct Class {
    /// The class's GUID as four dwords.
    pub clsid: [u32; 4],
    /// The object (16:16), the registration's number, and the task that
    /// registered it.
    pub object: u32,
    pub cookie: u32,
    pub task: u16,
}

crate::state_fields!(Class {
    clsid,
    object,
    cookie,
    task
});

fn ole(cpu: &mut Cpu) -> &mut Ole16 {
    &mut cpu.bus.win32.win16.ole
}

/// The handle of the task running, 0 outside one.
fn current(cpu: &Cpu) -> u16 {
    super::current_task(cpu).map_or(0, |t| cpu.bus.win32.win16.tasks[t].handle)
}

/// What the modules keep for the task running, if it has called them.
fn this_task(cpu: &mut Cpu) -> Option<&mut OleTask> {
    let task = current(cpu);
    ole(cpu).tasks.iter_mut().find(|t| t.task == task)
}

/// What the modules keep for the task running, made the first time.
pub fn this_task_or_new(cpu: &mut Cpu) -> &mut OleTask {
    let task = current(cpu);
    let tasks = &mut ole(cpu).tasks;
    let i = match tasks.iter().position(|t| t.task == task) {
        Some(i) => i,
        None => {
            tasks.push(OleTask {
                task,
                ..OleTask::default()
            });
            tasks.len() - 1
        }
    };
    &mut tasks[i]
}

/// The task `task` ended: what the modules kept for it goes, with the
/// class objects and drop targets it registered, so that no other task
/// reaches them. Their code and data went with the task, so none of
/// their methods is called.
pub fn task_ended(cpu: &mut Cpu, task: u16) {
    let o = ole(cpu);
    o.tasks.retain(|t| t.task != task);
    o.classes.retain(|c| c.task != task);
    o.drop_targets.retain(|d| d.2 != task);
}

pub static COMPOBJ: Def = Def {
    name: "COMPOBJ",
    file: "COMPOBJ.DLL",
    apis: &[
        rust("CoBuildVersion", 1, "", build_version),
        rust("CoInitialize", 2, "l", co_initialize),
        rust("CoUninitialize", 3, "", co_uninitialize),
        rust("CoGetMalloc", 4, "lp", co_get_malloc),
        rust(
            "CoRegisterClassObject",
            5,
            "plllp",
            co_register_class_object,
        ),
        rust("CoRevokeClassObject", 6, "l", co_revoke_class_object),
        rust("CoGetClassObject", 7, "pllll", co_get_class_object),
        missing("CoMarshalInterface", 8),
        missing("CoUnmarshalInterface", 9),
        missing("CoLoadLibrary", 10),
        missing("CoFreeLibrary", 11),
        missing("CoFreeAllLibraries", 12),
        rust("CoCreateInstance", 13, "pllll", co_create_instance),
        rust("StringFromIID", 14, "pl", string_from_clsid),
        rust("CoDisconnectObject", 15, "ll", s_ok),
        missing("CoReleaseMarshalData", 16),
        rust("CoFreeUnusedLibraries", 17, "", s_ok),
        rust("IsEqualGUID", 18, "pp", is_equal_guid),
        rust("StringFromCLSID", 19, "pl", string_from_clsid),
        rust("CLSIDFromString", 20, "pp", clsid_from_string),
        rust("IsValidPtrIn", 21, "Sw", is_valid_ptr),
        rust("IsValidPtrOut", 22, "Sw", is_valid_ptr),
        rust("IsValidInterface", 23, "l", is_valid_interface),
        missing("IsValidIid", 24),
        rust("ResultFromScode", 25, "l", scode),
        rust("GetScode", 26, "l", scode),
        rust(
            "CoRegisterMessageFilter",
            27,
            "lp",
            co_register_message_filter,
        ),
        missing("CoIsHandlerConnected", 28),
        win32(
            "CoFileTimeToDosDateTime",
            30,
            "ppp",
            K,
            "FileTimeToDosDateTime",
            b'b',
        ),
        win32(
            "CoDosDateTimeToFileTime",
            31,
            "wwp",
            K,
            "DosDateTimeToFileTime",
            b'b',
        ),
        missing("CoMarshalHresult", 32),
        missing("CoUnmarshalHresult", 33),
        rust("CoGetCurrentProcess", 34, "", co_get_current_process),
        rust("CoIsOle1Class", 36, "p", zero),
        variable("_GUID_NULL", 37, &[0; 16]),
        variable("_IID_IUnknown", 38, &IID_IUNKNOWN),
        variable("_IID_IClassFactory", 39, &ole_guid(1)),
        variable("_IID_IMalloc", 40, &IID_IMALLOC),
        variable("_IID_IMarshal", 41, &ole_guid(3)),
        variable("_IID_IRpcChannel", 42, &ole_guid(4)),
        variable("_IID_IRpcStub", 43, &ole_guid(5)),
        variable("_IID_IStubManager", 44, &ole_guid(6)),
        variable("_IID_IRpcProxy", 45, &ole_guid(7)),
        variable("_IID_IProxyManager", 46, &ole_guid(8)),
        variable("_IID_IPSFactory", 47, &ole_guid(9)),
        variable("_IID_ILockBytes", 48, &ole_guid(0x0A)),
        variable("_IID_IStorage", 49, &ole_guid(0x0B)),
        variable("_IID_IStream", 50, &ole_guid(0x0C)),
        variable("_IID_IEnumSTATSTG", 51, &ole_guid(0x0D)),
        variable("_IID_IBindCtx", 52, &ole_guid(0x0E)),
        variable("_IID_IMoniker", 53, &ole_guid(0x0F)),
        variable("_IID_IRunningObjectTable", 54, &ole_guid(0x10)),
        variable("_IID_IInternalMoniker", 55, &ole_guid(0x11)),
        variable("_IID_IRootStorage", 56, &ole_guid(0x12)),
        variable("_IID_IDfReserved1", 57, &ole_guid(0x13)),
        variable("_IID_IDfReserved2", 58, &ole_guid(0x14)),
        variable("_IID_IDfReserved3", 59, &ole_guid(0x15)),
        variable("_IID_IMessageFilter", 60, &ole_guid(0x16)),
        rust("CLSIDFromProgID", 61, "pp", clsid_from_prog_id),
        rust("ProgIDFromCLSID", 62, "pl", prog_id_from_clsid),
        rust("CoLockObjectExternal", 63, "lww", s_ok),
        variable("_CLSID_StdMarshal", 64, &ole_guid(0x17)),
        missing("CoGetTreatAsClass", 65),
        missing("CoTreatAsClass", 66),
        missing("CoGetStandardMarshal", 67),
        missing("PropagateResult", 68),
        missing("IIDFromString", 69),
        variable("_IID_IStdMarshalInfo", 70, &ole_guid(0x18)),
        rust(
            "CoCreateStandardMalloc",
            71,
            "lp",
            co_create_standard_malloc,
        ),
        variable("_IID_IExternalConnection", 72, &ole_guid(0x19)),
        rust("CoCreateGuid", 73, "p", co_create_guid),
        rust("StringFromGUID2", 76, "ppw", string_from_guid2),
        rust("CoFileTimeNow", 82, "p", co_file_time_now),
        // The task allocator's methods, which aren't exported.
        cdecl("IMalloc_QueryInterface", 0, "lpp", malloc_query_interface),
        cdecl("IMalloc_AddRef", 0, "l", one),
        cdecl("IMalloc_Release", 0, "l", one),
        cdecl("IMalloc_Alloc", 0, "ll", malloc_alloc),
        cdecl("IMalloc_Realloc", 0, "lll", malloc_realloc),
        cdecl("IMalloc_Free", 0, "ll", malloc_free),
        cdecl("IMalloc_GetSize", 0, "ll", malloc_get_size),
        cdecl("IMalloc_DidAlloc", 0, "ll", malloc_did_alloc),
        cdecl("IMalloc_HeapMinimize", 0, "l", zero),
    ],
    library: true,
};

/// The names of the task allocator's methods, in vtable order.
const MALLOC_METHODS: [&str; 9] = [
    "IMalloc_QueryInterface",
    "IMalloc_AddRef",
    "IMalloc_Release",
    "IMalloc_Alloc",
    "IMalloc_Realloc",
    "IMalloc_Free",
    "IMalloc_GetSize",
    "IMalloc_DidAlloc",
    "IMalloc_HeapMinimize",
];

pub static STORAGE: Def = Def {
    name: "STORAGE",
    file: "STORAGE.DLL",
    apis: &[
        rust("StgCreateDocfile", 1, "pllp", no_storage),
        rust("StgCreateDocfileOnILockBytes", 2, "lllp", no_storage),
        rust("StgOpenStorage", 3, "pllllp", no_storage),
        rust("StgOpenStorageOnILockBytes", 4, "lllllp", no_storage),
        rust("StgIsStorageFile", 5, "p", not_implemented),
        rust("StgIsStorageILockBytes", 6, "l", not_implemented),
        rust("StgSetTimes", 7, "pppp", not_implemented),
    ],
    library: true,
};

pub static OLE2: Def = Def {
    name: "OLE2",
    file: "OLE2.DLL",
    apis: &[
        rust("OleBuildVersion", 1, "", build_version),
        rust("OleInitialize", 2, "l", ole_initialize),
        rust("OleUninitialize", 3, "", ole_uninitialize),
        missing("DllGetClassObject", 4),
        missing("OleQueryLinkFromData", 6),
        missing("OleQueryCreateFromData", 7),
        missing("OleCreateFromData", 8),
        missing("OleCreateLinkFromData", 9),
        missing("OleCreate", 10),
        missing("OleCreateLink", 11),
        missing("OleLoad", 12),
        missing("OleSave", 13),
        missing("OleRun", 14),
        missing("OleIsRunning", 16),
        missing("OleLockRunning", 17),
        missing("ReadClassStg", 18),
        missing("WriteClassStg", 19),
        missing("ReadClassStm", 20),
        missing("WriteClassStm", 21),
        missing("BindMoniker", 22),
        missing("MkParseDisplayName", 23),
        missing("OleSaveToStream", 24),
        missing("OleLoadFromStream", 25),
        missing("CreateBindCtx", 26),
        missing("CreateItemMoniker", 27),
        missing("CreateFileMoniker", 28),
        missing("CreateGenericComposite", 29),
        missing("GetRunningObjectTable", 30),
        missing("OleGetMalloc", 31),
        missing("ReleaseStgMedium", 32),
        missing("ReadStringStream", 33),
        missing("WriteStringStream", 34),
        rust("RegisterDragDrop", 35, "wl", register_drag_drop),
        rust("RevokeDragDrop", 36, "w", revoke_drag_drop),
        missing("DoDragDrop", 37),
        missing("CreateOleAdviseHolder", 38),
        missing("CreateDataAdviseHolder", 39),
        missing("OleCreateMenuDescriptor", 40),
        missing("OleSetMenuDescriptor", 41),
        missing("OleDestroyMenuDescriptor", 42),
        missing("OpenOrCreateStream", 43),
        missing("CreateAntiMoniker", 44),
        missing("CreatePointerMoniker", 45),
        missing("MonikerRelativePathTo", 46),
        missing("MonikerCommonPrefixWith", 47),
        missing("IsAccelerator", 48),
        missing("OleSetClipboard", 49),
        missing("OleGetClipboard", 50),
        missing("OleDuplicateData", 51),
        missing("OleGetIconOfFile", 52),
        missing("OleGetIconOfClass", 53),
        missing("CreateILockBytesOnHGlobal", 54),
        missing("GetHGlobalFromILockBytes", 55),
        missing("OleMetafilePictFromIconAndLabel", 56),
        missing("GetClassFile", 57),
        missing("OleDraw", 58),
        missing("OleCreateDefaultHandler", 59),
        missing("OleCreateEmbeddingHelper", 60),
        missing("OleConvertIStorageToOLESTREAMEx", 61),
        missing("OleConvertOLESTREAMToIStorageEx", 62),
        missing("SetDocumentBitStg", 63),
        missing("GetDocumentBitStg", 64),
        missing("WriteOleStg", 65),
        missing("ReadOleStg", 66),
        missing("OleCreateFromFile", 67),
        missing("OleCreateLinkToFile", 68),
        missing("CreateDataCache", 69),
        missing("OleConvertIStorageToOLESTREAM", 70),
        missing("OleConvertOLESTREAMToIStorage", 71),
        missing("ReadFmtUserTypeStg", 74),
        missing("WriteFmtUserTypeStg", 75),
        missing("OleFlushClipboard", 76),
        missing("OleIsCurrentClipboard", 77),
        missing("OleTranslateAccelerator", 78),
        missing("OleDoAutoConvert", 79),
        missing("OleGetAutoConvert", 80),
        missing("OleSetAutoConvert", 81),
        missing("GetConvertStg", 82),
        missing("SetConvertStg", 83),
        missing("CreateStreamOnHGlobal", 84),
        missing("GetHGlobalFromStream", 85),
        missing("OleSetContainedObject", 86),
        missing("OleNoteObjectVisible", 87),
        missing("OleCreateStaticFromData", 88),
        missing("OleRegGetUserType", 89),
        missing("OleRegGetMiscStatus", 90),
        missing("OleRegEnumFormatEtc", 91),
        missing("OleRegEnumVerbs", 92),
        missing("OleGetEnumFormatEtc", 93),
        variable("_IID_IEnumUnknown", 120, &ole_guid(0x100)),
        variable("_IID_IEnumString", 121, &ole_guid(0x101)),
        variable("_IID_IEnumMoniker", 122, &ole_guid(0x102)),
        variable("_IID_IEnumFORMATETC", 123, &ole_guid(0x103)),
        variable("_IID_IEnumOLEVERB", 124, &ole_guid(0x104)),
        variable("_IID_IEnumSTATDATA", 125, &ole_guid(0x105)),
        variable("_IID_IEnumGeneric", 126, &ole_guid(0x106)),
        variable("_IID_IEnumHolder", 127, &ole_guid(0x107)),
        variable("_IID_IEnumCallback", 128, &ole_guid(0x108)),
        variable("_IID_IPersistStream", 129, &ole_guid(0x109)),
        variable("_IID_IPersistStorage", 130, &ole_guid(0x10A)),
        variable("_IID_IPersistFile", 131, &ole_guid(0x10B)),
        variable("_IID_IPersist", 132, &ole_guid(0x10C)),
        variable("_IID_IViewObject", 133, &ole_guid(0x10D)),
        variable("_IID_IDataObject", 134, &ole_guid(0x10E)),
        variable("_IID_IAdviseSink", 135, &ole_guid(0x10F)),
        variable("_IID_IDataAdviseHolder", 136, &ole_guid(0x110)),
        variable("_IID_IOleAdviseHolder", 137, &ole_guid(0x111)),
        variable("_IID_IOleObject", 138, &ole_guid(0x112)),
        variable("_IID_IOleInPlaceObject", 139, &ole_guid(0x113)),
        variable("_IID_IOleWindow", 140, &ole_guid(0x114)),
        variable("_IID_IOleInPlaceUIWindow", 141, &ole_guid(0x115)),
        variable("_IID_IOleInPlaceFrame", 142, &ole_guid(0x116)),
        variable("_IID_IOleInPlaceActiveObject", 143, &ole_guid(0x117)),
        variable("_IID_IOleClientSite", 144, &ole_guid(0x118)),
        variable("_IID_IOleInPlaceSite", 145, &ole_guid(0x119)),
        variable("_IID_IParseDisplayName", 146, &ole_guid(0x11A)),
        variable("_IID_IOleContainer", 147, &ole_guid(0x11B)),
        variable("_IID_IOleItemContainer", 148, &ole_guid(0x11C)),
        variable("_IID_IOleLink", 149, &ole_guid(0x11D)),
        variable("_IID_IOleCache", 150, &ole_guid(0x11E)),
        variable("_IID_IOleManager", 151, &ole_guid(0x11F)),
        variable("_IID_IOlePresObj", 152, &ole_guid(0x120)),
        variable("_IID_IDropSource", 153, &ole_guid(0x121)),
        variable("_IID_IDropTarget", 154, &ole_guid(0x122)),
        variable("_IID_IDebug", 155, &ole_guid(0x123)),
        variable("_IID_IDebugStream", 156, &ole_guid(0x124)),
        variable("_IID_IAdviseSink2", 157, &ole_guid(0x125)),
        variable("_IID_IViewObject2", 158, &ole_guid(0x127)),
        variable("_IID_IOleCache2", 159, &ole_guid(0x128)),
        variable("_IID_IOleCacheControl", 160, &ole_guid(0x129)),
        variable("_IID_IRunnableObject", 161, &ole_guid(0x126)),
    ],
    library: true,
};

/// OLE2NLS: the locale functions of Win32's NLS before Win32, which
/// KERNEL32's do as they are.
pub static OLE2NLS: Def = Def {
    name: "OLE2NLS",
    file: "OLE2NLS.DLL",
    apis: &[
        win32("GetUserDefaultLCID", 1, "", K, "GetUserDefaultLCID", b'w'),
        win32(
            "GetSystemDefaultLCID",
            2,
            "",
            K,
            "GetSystemDefaultLCID",
            b'w',
        ),
        win32(
            "GetUserDefaultLangID",
            3,
            "",
            K,
            "GetUserDefaultLangID",
            b'w',
        ),
        win32(
            "GetSystemDefaultLangID",
            4,
            "",
            K,
            "GetSystemDefaultLangID",
            b'w',
        ),
        win32("GetLocaleInfoA", 5, "llps", K, "GetLocaleInfoA", b'w'),
        win32("LCMapStringA", 6, "llpsps", K, "LCMapStringA", b'w'),
        win32("GetStringTypeA", 7, "llpsp", K, "GetStringTypeA", b'b'),
        win32("CompareStringA", 8, "llpsps", K, "CompareStringA", b'w'),
        // No locale's information changes, so nothing is ever notified.
        rust("RegisterNLSInfoChanged", 9, "l", one),
    ],
    library: true,
};

fn zero(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(0)
}

fn one(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(1)
}

fn s_ok(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(S_OK)
}

/// A function the built-in Windows doesn't do that has a way to fail:
/// E_NOTIMPL, said in the log.
fn not_implemented(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    log_not_implemented(cpu);
    Some(E_NOTIMPL)
}

/// A compound file's functions, whose last argument is where the
/// IStorage goes: NULL there, and E_NOTIMPL.
fn no_storage(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let out = *args.last().expect("an IStorage's place");
    if out != 0 {
        super::write_32(cpu, out, 0);
    }
    log_not_implemented(cpu);
    Some(E_NOTIMPL)
}

fn log_not_implemented(cpu: &mut Cpu) {
    // The stub's trap is 4 bytes before the return the processor is at.
    let at = cpu.seg_cache(crate::cpu::Seg::CS).base + cpu.eip().wrapping_sub(4);
    let id = cpu.bus.guest_read_16(at + 2);
    let name = super::builtin::function_name(id).unwrap_or_default();
    cpu.bus
        .log_string(&format!("[W16] {name} isn't implemented: E_NOTIMPL"));
}

fn build_version(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(RMM << 16 | RUP)
}

/// GetScode(hr) and ResultFromScode(sc): the value with the bits
/// between the severity and the facility cleared, as OLE 2.02's do it.
fn scode(_: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(args[0] & 0x800F_FFFF)
}

/// IsValidPtrIn(pv, cb) and IsValidPtrOut: TRUE when the `cb` bytes at
/// `pv` are all there, as KERNEL's IsBadReadPtr and IsBadWritePtr tell
/// it.
fn is_valid_ptr(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(!super::kernel::bad_range(cpu, args[0], args[1]) as u32)
}

// Initialization.

/// CoInitialize(lpMalloc): S_OK the first time, S_FALSE after. A
/// program's own allocator, if it gives one, is the task allocator from
/// then on, and COMPOBJ holds a reference to it until the last
/// CoUninitialize: Visual Basic 4 releases its own reference right after
/// OleInitialize, and its allocator frees itself if nothing else holds
/// one.
fn co_initialize(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    initialize(cpu, args[0], false)
}

/// CoInitialize, and for OleInitialize OLE's own initialization too,
/// which has nothing to set up here.
fn initialize(cpu: &mut Cpu, malloc: u32, ole_too: bool) -> Option<u32> {
    let t = this_task_or_new(cpu);
    t.co_inits += 1;
    let first = t.co_inits == 1;
    let mut result = if first { S_OK } else { S_FALSE };
    if ole_too {
        t.ole_inits += 1;
        if t.ole_inits > 1 {
            result = S_FALSE;
        }
    }
    if first && malloc != 0 {
        t.app_malloc = malloc;
        return call_method(cpu, malloc, 1, &[], "", Some(result));
    }
    Some(result)
}

fn co_uninitialize(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    let Some(t) = this_task(cpu) else {
        return Some(0);
    };
    t.co_inits = t.co_inits.saturating_sub(1);
    if t.co_inits == 0 && t.app_malloc != 0 {
        // The reference CoInitialize took.
        let malloc = std::mem::take(&mut t.app_malloc);
        return call_method(cpu, malloc, 2, &[], "", Some(0));
    }
    Some(0)
}

/// OleInitialize(lpMalloc): CoInitialize, and OLE's own initialization.
fn ole_initialize(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    initialize(cpu, args[0], true)
}

fn ole_uninitialize(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    if let Some(t) = this_task(cpu) {
        t.ole_inits = t.ole_inits.saturating_sub(1);
    }
    co_uninitialize(cpu, &[])
}

// The task allocator.

/// The handle of COMPOBJ's module, which owns the allocator's blocks:
/// they go when it does (`unloaded`).
fn compobj_handle(cpu: &Cpu) -> u16 {
    let index = super::builtin::find("COMPOBJ");
    cpu.bus
        .win32
        .win16
        .modules
        .iter()
        .find(|m| m.builtin.map(usize::from) == index)
        .map_or(0, |m| m.handle)
}

/// COMPOBJ was unloaded, and its blocks with it: the allocator and what
/// it gave out are gone.
pub fn unloaded(cpu: &mut Cpu) {
    *ole(cpu) = Ole16::default();
}

/// The task allocator's object: a 16:16 pointer to its vtable's, which
/// follows it, made the first time. 0 if there is no memory for it.
pub fn task_malloc(cpu: &mut Cpu) -> u32 {
    if ole(cpu).malloc != 0 {
        return ole(cpu).malloc;
    }
    let Some(module) = super::builtin::find("COMPOBJ") else {
        return 0;
    };
    let owner = compobj_handle(cpu);
    let handle = global::alloc(
        cpu,
        global::GMEM_FIXED | global::GMEM_ZEROINIT,
        4 + 4 * MALLOC_METHODS.len() as u32,
        owner,
        false,
    );
    if handle == 0 {
        return 0;
    }
    let sel = global::handle_to_sel(handle);
    let base = super::lin(cpu, sel, 0);
    super::write_32(cpu, base, (sel as u32) << 16 | 4);
    for (i, name) in MALLOC_METHODS.iter().enumerate() {
        let stub = super::builtin::stub_address(cpu, module, name).unwrap_or(0);
        super::write_32(cpu, base + 4 + 4 * i as u32, stub);
    }
    let object = (sel as u32) << 16;
    ole(cpu).malloc = object;
    object
}

/// The local heaps' blocks are at most this big; a bigger block is a
/// global block of its own.
const LARGEST_LOCAL: u32 = 0x8000;
/// The size a local heap's global block starts at; it grows to 64 KB.
const HEAP_START: u32 = 0x1000;

/// A block of `size` bytes from the task allocator: its 16:16 address,
/// or 0.
pub fn alloc(cpu: &mut Cpu, size: u32) -> u32 {
    let owner = compobj_handle(cpu);
    if size > LARGEST_LOCAL {
        // A segment holds at most 64 KB; a bigger block takes several
        // selectors, its address the first one's.
        let handle = global::alloc(cpu, global::GMEM_FIXED, size, owner, false);
        if handle == 0 {
            return 0;
        }
        let sel = global::handle_to_sel(handle);
        ole(cpu).large.push(sel);
        return (sel as u32) << 16;
    }
    let heaps = ole(cpu).heaps.clone();
    for sel in heaps {
        let offset = local::alloc(cpu, sel, 0, size.max(1) as u16);
        if offset != 0 {
            return (sel as u32) << 16 | offset as u32;
        }
    }
    let handle = global::alloc(cpu, global::GMEM_MOVEABLE, HEAP_START, owner, false);
    if handle == 0 {
        return 0;
    }
    let sel = global::handle_to_sel(handle);
    if !local::init(cpu, sel, 0x10, HEAP_START as u16) {
        global::free(cpu, handle);
        return 0;
    }
    ole(cpu).heaps.push(sel);
    match local::alloc(cpu, sel, 0, size.max(1) as u16) {
        0 => 0,
        offset => (sel as u32) << 16 | offset as u32,
    }
}

/// Where the block at `p` is: a local heap's or a global block of its
/// own, or none of the allocator's.
enum Place {
    Local(u16, u16),
    Large(u16),
}

fn place(cpu: &mut Cpu, p: u32) -> Option<Place> {
    let sel = (p >> 16) as u16;
    let offset = p as u16;
    let o = ole(cpu);
    if offset == 0 && o.large.contains(&sel) {
        return Some(Place::Large(sel));
    }
    if !o.heaps.contains(&sel) {
        return None;
    }
    (local::handle(cpu, sel, offset) == offset).then_some(Place::Local(sel, offset))
}

/// Free the block at `p`. False if it isn't the allocator's.
pub fn free(cpu: &mut Cpu, p: u32) -> bool {
    match place(cpu, p) {
        Some(Place::Local(sel, offset)) => local::free(cpu, sel, offset) == 0,
        Some(Place::Large(sel)) => {
            ole(cpu).large.retain(|&s| s != sel);
            global::free(cpu, global::handle_of(cpu, sel))
        }
        None => false,
    }
}

/// The size of the block at `p`, or None if it isn't the allocator's.
pub fn size(cpu: &mut Cpu, p: u32) -> Option<u32> {
    match place(cpu, p)? {
        Place::Local(sel, offset) => Some(local::size(cpu, sel, offset) as u32),
        Place::Large(sel) => Some(global::size(cpu, global::handle_of(cpu, sel))),
    }
}

/// The block at `p` with `size` bytes, its contents kept: where it is
/// now, or 0 (and the block as it was).
pub fn realloc(cpu: &mut Cpu, p: u32, size: u32) -> u32 {
    if p == 0 {
        return alloc(cpu, size);
    }
    if size == 0 {
        free(cpu, p);
        return 0;
    }
    let Some(old) = self::size(cpu, p) else {
        return 0;
    };
    if let Some(Place::Local(sel, offset)) = place(cpu, p)
        && size <= LARGEST_LOCAL
        && local::realloc(cpu, sel, offset, size as u16, 0) == offset
    {
        return p;
    }
    let new = alloc(cpu, size);
    if new == 0 {
        return 0;
    }
    let from = super::linear(cpu, p);
    let to = super::linear(cpu, new);
    let bytes = super::peek_upto(cpu, from, old.min(size) as usize);
    super::poke(cpu, to, &bytes);
    free(cpu, p);
    new
}

/// CoGetMalloc(dwMemContext, lplpMalloc): the task allocator (which in
/// one Win16 process serves as the shared one too), once CoInitialize
/// has been called.
fn co_get_malloc(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [context, out] = args[..] else {
        unreachable!()
    };
    if out == 0 {
        return Some(E_INVALIDARG);
    }
    super::write_32(cpu, out, 0);
    if context != MEMCTX_TASK && context != MEMCTX_SHARED {
        return Some(E_INVALIDARG);
    }
    let (inits, app) = this_task(cpu).map_or((0, 0), |t| (t.co_inits, t.app_malloc));
    if context == MEMCTX_TASK && inits == 0 {
        return Some(CO_E_NOTINITIALIZED);
    }
    if context == MEMCTX_TASK && app != 0 {
        // The program's allocator, with a reference added.
        super::write_32(cpu, out, app);
        return call_method(cpu, app, 1, &[], "", Some(S_OK));
    }
    match task_malloc(cpu) {
        0 => Some(E_OUTOFMEMORY),
        malloc => {
            super::write_32(cpu, out, malloc);
            Some(S_OK)
        }
    }
}

/// CoCreateStandardMalloc(memctx, lplpMalloc): the allocator.
fn co_create_standard_malloc(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let out = args[1];
    match task_malloc(cpu) {
        0 => Some(E_OUTOFMEMORY),
        malloc => {
            if out != 0 {
                super::write_32(cpu, out, malloc);
            }
            Some(S_OK)
        }
    }
}

/// IMalloc::QueryInterface(this, riid, ppv): IUnknown and IMalloc.
fn malloc_query_interface(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, riid, out] = args[..] else {
        unreachable!()
    };
    if out == 0 {
        return Some(E_INVALIDARG);
    }
    let iid = read_guid(cpu, riid);
    if iid == IID_IUNKNOWN || iid == IID_IMALLOC {
        super::write_32(cpu, out, this);
        Some(S_OK)
    } else {
        super::write_32(cpu, out, 0);
        Some(E_NOINTERFACE)
    }
}

fn malloc_alloc(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(alloc(cpu, args[1]))
}

fn malloc_realloc(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(realloc(cpu, args[1], args[2]))
}

fn malloc_free(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if args[1] != 0 {
        free(cpu, args[1]);
    }
    Some(0)
}

/// IMalloc::GetSize: the block's size, or -1 for a block not the
/// allocator's.
fn malloc_get_size(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(size(cpu, args[1]).unwrap_or(u32::MAX))
}

/// IMalloc::DidAlloc: 1 for a block of the allocator's, 0 for another;
/// -1 for NULL, which it can't tell.
fn malloc_did_alloc(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if args[1] == 0 {
        return Some(0xFFFF);
    }
    Some(place(cpu, args[1]).is_some() as u32)
}

// Calling the program's objects.

/// Call the method `method` (its place in the vtable) of the 16-bit
/// interface `object`, with the arguments `args` after the object's
/// pointer, of the sizes `sizes` says (`l` a dword, `w` a word), then
/// return to the function's caller `value`, or what the method returned
/// when it is None.
pub fn call_method(
    cpu: &mut Cpu,
    object: u32,
    method: u32,
    args: &[u32],
    sizes: &str,
    value: Option<u32>,
) -> Option<u32> {
    let target = method_address(cpu, object, method);
    if target == 0 {
        return Some(value.unwrap_or(E_INVALIDARG));
    }
    let data = match value {
        Some(v) => vec![1, v],
        None => vec![0, 0],
    };
    call_at(
        cpu,
        target,
        object,
        args,
        sizes,
        thunk::THEN_OLE_METHOD,
        data,
    );
    None
}

/// Call the method at `target` of the 16-bit interface `object` with
/// the arguments `args` (`sizes` as `call_method` has them); `then`
/// gets what it returned, and `data`.
fn call_at(
    cpu: &mut Cpu,
    target: u32,
    object: u32,
    args: &[u32],
    sizes: &str,
    then: u16,
    data: Vec<u32>,
) {
    // C's order: the last argument pushed first, a dword's high word
    // before its low one.
    let mut words = Vec::new();
    for (&a, size) in args.iter().zip(sizes.bytes()).rev() {
        if size == b'l' {
            words.push((a >> 16) as u16);
        }
        words.push(a as u16);
    }
    words.push((object >> 16) as u16);
    words.push(object as u16);
    let resume = Context::of(cpu);
    thunk::call16(cpu, target, &words, 0, resume, then, data);
}

/// Return `result` to the function's caller in DX:AX from a `then`.
fn dx_ax(ctx: &mut Context, result: u32) -> Option<u32> {
    ctx.gpr[thunk::EDX] = ctx.gpr[thunk::EDX] & 0xFFFF_0000 | result >> 16;
    Some(ctx.gpr[thunk::EAX] & 0xFFFF_0000 | result & 0xFFFF)
}

/// Give the program `bytes` in a block of its task allocator, the
/// block's 16:16 address at the 16:16 `out`, and return S_OK, or
/// E_OUTOFMEMORY (and NULL at `out`) when there is no block. The program
/// frees the block with the allocator CoGetMalloc gives it, so when
/// CoInitialize was given the program's own allocator, its Alloc makes
/// the block (`then_block` fills it); otherwise the built-in one does.
fn task_block(cpu: &mut Cpu, out: u32, bytes: &[u8]) -> Option<u32> {
    let app = this_task(cpu).map_or(0, |t| t.app_malloc);
    let target = if app == 0 {
        0
    } else {
        method_address(cpu, app, 3)
    };
    if target == 0 {
        let p = alloc(cpu, bytes.len() as u32);
        return Some(fill_block(cpu, out, p, bytes));
    }
    let mut data = vec![out, bytes.len() as u32];
    for chunk in bytes.chunks(4) {
        let mut word = [0; 4];
        word[..chunk.len()].copy_from_slice(chunk);
        data.push(u32::from_le_bytes(word));
    }
    let size = bytes.len() as u32;
    call_at(cpu, target, app, &[size], "l", thunk::THEN_OLE_BLOCK, data);
    None
}

/// Put `bytes` in the block at `p` (16:16) and `p` at `out`: S_OK, or
/// E_OUTOFMEMORY for no block.
fn fill_block(cpu: &mut Cpu, out: u32, p: u32, bytes: &[u8]) -> u32 {
    let at = super::linear(cpu, out);
    if at != 0 {
        super::write_32(cpu, at, p);
    }
    let block = super::linear(cpu, p);
    if block == 0 {
        if at != 0 {
            super::write_32(cpu, at, 0);
        }
        return E_OUTOFMEMORY;
    }
    super::poke(cpu, block, bytes);
    S_OK
}

/// The program's IMalloc::Alloc that `task_block` called returned the
/// block `value`: `data` is `out`, the bytes' count, and the bytes, four
/// in each dword.
pub(super) fn then_block(
    cpu: &mut Cpu,
    value: u32,
    data: &[u32],
    ctx: &mut Context,
) -> Option<u32> {
    let bytes: Vec<u8> = data[2..].iter().flat_map(|w| w.to_le_bytes()).collect();
    let result = fill_block(cpu, data[0], value, &bytes[..data[1] as usize]);
    dx_ax(ctx, result)
}

/// Call IUnknown's AddRef (method 1) or Release (2) of each of `calls`
/// (object, method) in turn, then return `value` to the function's
/// caller.
pub fn call_all(cpu: &mut Cpu, value: u32, calls: &[(u32, u32)]) -> Option<u32> {
    let calls: Vec<(u32, u32)> = calls
        .iter()
        .copied()
        .filter(|&(o, m)| method_address(cpu, o, m) != 0)
        .collect();
    let Some((&(object, method), rest)) = calls.split_first() else {
        return Some(value);
    };
    let target = method_address(cpu, object, method);
    let resume = Context::of(cpu);
    let mut data = vec![1, value];
    for &(o, m) in rest {
        data.extend([o, m]);
    }
    let words = [(object >> 16) as u16, object as u16];
    thunk::call16(cpu, target, &words, 0, resume, thunk::THEN_OLE_METHOD, data);
    None
}

/// The 16:16 address of the method `method` of the 16-bit interface
/// `object`, or 0 if its vtable can't be read.
pub fn method_address(cpu: &Cpu, object: u32, method: u32) -> u32 {
    let at = super::linear(cpu, object);
    if at == 0 {
        return 0;
    }
    let vtable = super::linear(cpu, super::read_32(cpu, at));
    if vtable == 0 {
        return 0;
    }
    super::read_32(cpu, vtable + 4 * method)
}

/// A method `call_method` or `call_all` called returned `value`: the
/// next of `call_all`'s calls, or DX:AX for the function's caller, as
/// `data` says: [0, 0] the method's value, [1, value] that value, then
/// the calls still to make.
pub(super) fn then_method(
    cpu: &mut Cpu,
    value: u32,
    data: &[u32],
    ctx: &mut Context,
) -> Option<u32> {
    let result = if data[0] == 1 { data[1] } else { value };
    if let [object, method, rest @ ..] = &data[2..] {
        let target = method_address(cpu, *object, *method);
        let mut more = vec![1, result];
        more.extend_from_slice(rest);
        let words = [(*object >> 16) as u16, *object as u16];
        thunk::call16(cpu, target, &words, 0, *ctx, thunk::THEN_OLE_METHOD, more);
        return None;
    }
    dx_ax(ctx, result)
}

// Class objects.

fn guid_dwords(g: [u8; 16]) -> [u32; 4] {
    let (chunks, _) = g.as_chunks::<4>();
    [0, 1, 2, 3].map(|i| u32::from_le_bytes(chunks[i]))
}

/// CoRegisterClassObject(rclsid, pUnk, dwClsContext, flags,
/// lpdwRegister): the object kept, with a reference, for CoGetClassObject
/// and CoCreateInstance of the class in this process.
fn co_register_class_object(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [clsid, unk, _context, _flags, out] = args[..] else {
        unreachable!()
    };
    if clsid == 0 || unk == 0 || out == 0 {
        return Some(E_INVALIDARG);
    }
    let clsid = guid_dwords(read_guid(cpu, clsid));
    let task = current(cpu);
    let o = ole(cpu);
    o.next_cookie += 1;
    let cookie = o.next_cookie;
    o.classes.push(Class {
        clsid,
        object: unk,
        cookie,
        task,
    });
    super::write_32(cpu, out, cookie);
    call_method(cpu, unk, 1, &[], "", Some(S_OK))
}

/// CoRevokeClassObject(dwRegister): the registration ended, the
/// object's reference released.
fn co_revoke_class_object(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let classes = &mut ole(cpu).classes;
    let Some(i) = classes.iter().position(|c| c.cookie == args[0]) else {
        return Some(CO_E_OBJNOTREG);
    };
    let unk = classes.remove(i).object;
    call_method(cpu, unk, 2, &[], "", Some(S_OK))
}

/// The object registered for the class whose GUID is at `clsid`.
fn registered(cpu: &mut Cpu, clsid: u32) -> Option<u32> {
    let clsid = guid_dwords(read_guid(cpu, clsid));
    ole(cpu)
        .classes
        .iter()
        .find(|c| c.clsid == clsid)
        .map(|c| c.object)
}

/// CoGetClassObject(rclsid, dwClsContext, pvReserved, riid, ppv): the
/// class object registered in this process, asked for the interface.
/// The built-in Windows starts no server for a class, so one not
/// registered is REGDB_E_CLASSNOTREG. `riid` and `ppv` come as 16:16
/// pointers, which the method gets.
fn co_get_class_object(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [clsid, _context, _reserved, riid, out] = args[..] else {
        unreachable!()
    };
    let at = super::linear(cpu, out);
    if at == 0 {
        return Some(E_INVALIDARG);
    }
    super::write_32(cpu, at, 0);
    match registered(cpu, clsid) {
        Some(unk) => call_method(cpu, unk, 0, &[riid, out], "ll", None),
        None => Some(REGDB_E_CLASSNOTREG),
    }
}

/// CoCreateInstance(rclsid, pUnkOuter, dwClsContext, riid, ppv): an
/// object of a class registered in this process, made by its class
/// object's IClassFactory::CreateInstance (the object registered is the
/// class factory, as CoRegisterClassObject's callers register it).
fn co_create_instance(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [clsid, outer, _context, riid, out] = args[..] else {
        unreachable!()
    };
    let at = super::linear(cpu, out);
    if at == 0 {
        return Some(E_INVALIDARG);
    }
    super::write_32(cpu, at, 0);
    match registered(cpu, clsid) {
        Some(factory) => call_method(cpu, factory, 3, &[outer, riid, out], "lll", None),
        None => Some(REGDB_E_CLASSNOTREG),
    }
}

/// CoRegisterMessageFilter(lpMessageFilter, lplpMessageFilter): the
/// task's filter kept with a reference, the one before given back with
/// its reference, or released when `lplpMessageFilter` is NULL. No call
/// between processes is made, so it is never called.
fn co_register_message_filter(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [filter, out] = args[..] else {
        unreachable!()
    };
    let old = std::mem::replace(&mut this_task_or_new(cpu).message_filter, filter);
    let mut calls = Vec::new();
    if filter != 0 {
        calls.push((filter, 1));
    }
    if out != 0 {
        super::write_32(cpu, out, old);
    } else if old != 0 {
        calls.push((old, 2));
    }
    call_all(cpu, S_OK, &calls)
}

/// CoGetCurrentProcess: a number for the task, unique while it runs.
fn co_get_current_process(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(current(cpu) as u32)
}

// GUIDs.

pub fn read_guid(cpu: &Cpu, at: u32) -> [u8; 16] {
    let mut g = [0u8; 16];
    if at != 0
        && let Some(bytes) = super::peek(cpu, at, 16)
    {
        g.copy_from_slice(&bytes);
    }
    g
}

/// "{XXXXXXXX-XXXX-XXXX-XXXX-XXXXXXXXXXXX}".
pub fn guid_text(g: &[u8; 16]) -> String {
    let [d1, d23, _, _] = guid_dwords(*g);
    let (d2, d3) = (d23 as u16, (d23 >> 16) as u16);
    let mut s = format!("{{{d1:08X}-{d2:04X}-{d3:04X}-{:02X}{:02X}-", g[8], g[9]);
    for byte in &g[10..] {
        s.push_str(&format!("{byte:02X}"));
    }
    s.push('}');
    s
}

/// The GUID "{...}" names, if it is one.
pub fn parse_guid(text: &str) -> Option<[u8; 16]> {
    let t = text.strip_prefix('{')?.strip_suffix('}')?;
    let parts: Vec<&str> = t.split('-').collect();
    let lens = [8, 4, 4, 4, 12];
    if parts.len() != 5
        || parts
            .iter()
            .zip(lens)
            .any(|(p, n)| p.len() != n || !p.bytes().all(|c| c.is_ascii_hexdigit()))
    {
        return None;
    }
    let d1 = u32::from_str_radix(parts[0], 16).ok()?;
    let d2 = u16::from_str_radix(parts[1], 16).ok()?;
    let d3 = u16::from_str_radix(parts[2], 16).ok()?;
    let rest = format!("{}{}", parts[3], parts[4]);
    let mut d4 = [0u8; 8];
    for (i, b) in d4.iter_mut().enumerate() {
        *b = u8::from_str_radix(&rest[2 * i..2 * i + 2], 16).ok()?;
    }
    Some(guid(d1, d2, d3, d4))
}

/// StringFromCLSID(rclsid, lplpsz), StringFromIID: the GUID's text in a
/// block of the task allocator's (`task_block`).
fn string_from_clsid(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [clsid, out] = args[..] else {
        unreachable!()
    };
    if out == 0 {
        return Some(E_INVALIDARG);
    }
    let mut text = guid_text(&read_guid(cpu, clsid)).into_bytes();
    text.push(0);
    task_block(cpu, out, &text)
}

/// StringFromGUID2(rguid, lpsz, cbMax): the text in the program's
/// buffer, its length with the NUL, or 0 if it doesn't fit.
fn string_from_guid2(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [g, out, max] = args[..] else {
        unreachable!()
    };
    let mut text = guid_text(&read_guid(cpu, g)).into_bytes();
    text.push(0);
    if (max as u16 as usize) < text.len() || out == 0 {
        return Some(0);
    }
    super::poke(cpu, out, &text);
    Some(text.len() as u32)
}

/// CLSIDFromString(lpsz, pclsid): the GUID of the text, CLSID_NULL for
/// none, CO_E_CLASSSTRING for text that isn't one.
fn clsid_from_string(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [text, out] = args[..] else {
        unreachable!()
    };
    if out == 0 {
        return Some(E_INVALIDARG);
    }
    let text = super::string(cpu, text);
    if text.is_empty() {
        super::poke(cpu, out, &[0; 16]);
        return Some(S_OK);
    }
    match parse_guid(&text) {
        Some(g) => {
            super::poke(cpu, out, &g);
            Some(S_OK)
        }
        None => Some(CO_E_CLASSSTRING),
    }
}

fn is_equal_guid(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some((read_guid(cpu, args[0]) == read_guid(cpu, args[1])) as u32)
}

/// IsValidInterface(punk): whether it points at a vtable whose first
/// three methods (IUnknown's) are there.
fn is_valid_interface(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let valid = (0..3).all(|m| {
        let f = method_address(cpu, args[0], m);
        f != 0 && super::linear(cpu, f) != 0
    });
    Some(valid as u32)
}

const HKEY_CLASSES_ROOT: u32 = 0x8000_0000;

/// CLSIDFromProgID(lpszProgID, pclsid): the class the registry gives
/// the name at HKEY_CLASSES_ROOT\name\CLSID.
fn clsid_from_prog_id(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [name, out] = args[..] else {
        unreachable!()
    };
    let name = super::string(cpu, name);
    if out == 0 || name.is_empty() {
        return Some(E_INVALIDARG);
    }
    let path = format!("{name}\\CLSID");
    let found = crate::win32::registry::string_value(cpu, HKEY_CLASSES_ROOT, &path, "")
        .and_then(|t| parse_guid(t.trim()));
    match found {
        Some(g) => {
            super::poke(cpu, out, &g);
            Some(S_OK)
        }
        None => {
            super::poke(cpu, out, &[0; 16]);
            Some(CO_E_CLASSSTRING)
        }
    }
}

/// ProgIDFromCLSID(clsid, lplpszProgID): the name at
/// HKEY_CLASSES_ROOT\CLSID\{...}\ProgID, in a block of the task
/// allocator's (`task_block`).
fn prog_id_from_clsid(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [clsid, out] = args[..] else {
        unreachable!()
    };
    let at = super::linear(cpu, out);
    if at == 0 {
        return Some(E_INVALIDARG);
    }
    super::write_32(cpu, at, 0);
    let path = format!("CLSID\\{}\\ProgID", guid_text(&read_guid(cpu, clsid)));
    let Some(name) = crate::win32::registry::string_value(cpu, HKEY_CLASSES_ROOT, &path, "") else {
        return Some(REGDB_E_CLASSNOTREG);
    };
    let mut text = crate::dosstr::to_bytes(&name);
    text.push(0);
    task_block(cpu, out, &text)
}

/// CoCreateGuid(pguid): a version 4 (random) GUID, from a generator
/// seeded with the emulated time, so that a deterministic run makes the
/// same ones.
fn co_create_guid(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if args[0] == 0 {
        return Some(E_INVALIDARG);
    }
    let now = cpu.bus.clock.now_ns();
    let o = ole(cpu);
    if o.guid_seed == 0 {
        o.guid_seed = now | 1;
    }
    let mut g = [0u8; 16];
    for chunk in g.chunks_mut(8) {
        // xorshift64*.
        let mut x = o.guid_seed;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        o.guid_seed = x;
        chunk.copy_from_slice(&x.wrapping_mul(0x2545_F491_4F6C_DD1D).to_le_bytes());
    }
    g[7] = g[7] & 0x0F | 0x40;
    g[8] = g[8] & 0x3F | 0x80;
    super::poke(cpu, args[0], &g);
    Some(S_OK)
}

/// CoFileTimeNow(lpft): the time now, in UTC, as a FILETIME.
fn co_file_time_now(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if args[0] == 0 {
        return Some(E_INVALIDARG);
    }
    let ft = crate::win32::kernel32::time::file_time_now(cpu);
    super::poke(cpu, args[0], &ft.to_le_bytes());
    Some(S_OK)
}

// Drag and drop.

/// RegisterDragDrop(hwnd, pDropTarget): the target kept, with a
/// reference, for the window. Nothing is dragged between windows of the
/// built-in Windows, so it is never called.
fn register_drag_drop(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [hwnd, target] = args[..] else {
        unreachable!()
    };
    if target == 0 {
        return Some(E_INVALIDARG);
    }
    let hwnd = hwnd as u16;
    if ole(cpu).drop_targets.iter().any(|t| t.0 == hwnd) {
        return Some(DRAGDROP_E_ALREADYREGISTERED);
    }
    let task = current(cpu);
    ole(cpu).drop_targets.push((hwnd, target, task));
    call_method(cpu, target, 1, &[], "", Some(S_OK))
}

fn revoke_drag_drop(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let hwnd = args[0] as u16;
    let targets = &mut ole(cpu).drop_targets;
    let Some(i) = targets.iter().position(|t| t.0 == hwnd) else {
        return Some(DRAGDROP_E_NOTREGISTERED);
    };
    let (_, target, _) = targets.remove(i);
    call_method(cpu, target, 2, &[], "", Some(S_OK))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guids_read_and_print_as_the_registry_has_them() {
        let unknown = "{00000000-0000-0000-C000-000000000046}";
        assert_eq!(guid_text(&IID_IUNKNOWN), unknown);
        assert_eq!(parse_guid(unknown), Some(IID_IUNKNOWN));
        let dispatch = guid(0x0002_0400, 0, 0, [0xC0, 0, 0, 0, 0, 0, 0, 0x46]);
        assert_eq!(
            guid_text(&dispatch),
            "{00020400-0000-0000-C000-000000000046}"
        );
        let error_info = guid(
            0x1CF2_B120,
            0x547D,
            0x101B,
            [0x8E, 0x65, 0x08, 0x00, 0x2B, 0x2B, 0xD1, 0x19],
        );
        let text = "{1CF2B120-547D-101B-8E65-08002B2BD119}";
        assert_eq!(guid_text(&error_info), text);
        assert_eq!(parse_guid(text), Some(error_info));
        assert_eq!(parse_guid("{1CF2B120-547D-101B-8E65-08002B2BD11}"), None);
        assert_eq!(parse_guid("1CF2B120-547D-101B-8E65-08002B2BD119"), None);
    }
}
