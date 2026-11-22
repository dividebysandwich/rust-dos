//! RPCRT4.DLL: what COM's proxy/stub DLLs call, the DLLs MIDL makes for
//! the interfaces of a program's local server (InstallShield 6's
//! OBJECTPS.DLL, for its engine IKernel.exe). Their proxies and stubs
//! are code of their own that calls these functions to set up a call's
//! messages, to marshal the types it doesn't copy itself into an NDR
//! buffer and back, and to send the buffer to the object's process.
//! The functions are OLE32's (ole32/rpc/ndr.rs), where the calls
//! between processes are. Windows 98's RPCRT4 also has RPC itself,
//! between machines and endpoints, which no program of a game's needs:
//! only the functions COM's proxies and stubs call are here.

use super::ole32::ndr;
use super::stubs::{Api, Conv, Dll, Vtable};

pub static DLL: Dll = Dll {
    name: "RPCRT4.DLL",
    base: 0xBF50_0000,
    apis: APIS,
    vars: &[],
    init: None,
    term: None,
    vtables: VTABLES,
    resources: None,
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
/// `stubs`). The methods of the proxy/stub DLLs' class objects have
/// ordinal 0: they aren't exported.
static APIS: &[Api] = &[
    stdcall("NdrDllGetClassObject", 150, 6, ndr::dll_get_class_object),
    stdcall("NdrDllCanUnloadNow", 149, 1, ndr::dll_can_unload_now),
    stdcall("NdrDllRegisterProxy", 151, 3, ndr::dll_register_proxy),
    stdcall("NdrDllUnregisterProxy", 152, 3, ndr::dll_unregister_proxy),
    stdcall("NdrProxyInitialize", 211, 5, ndr::proxy_initialize),
    stdcall("NdrProxyGetBuffer", 210, 2, ndr::proxy_get_buffer),
    stdcall("NdrProxySendReceive", 212, 2, ndr::proxy_send_receive),
    stdcall("NdrProxyFreeBuffer", 209, 2, ndr::proxy_free_buffer),
    stdcall("NdrProxyErrorHandler", 208, 1, ndr::proxy_error_handler),
    stdcall("NdrClearOutParameters", 105, 3, ndr::clear_out_parameters),
    stdcall(
        "IUnknown_QueryInterface_Proxy",
        21,
        3,
        ndr::unknown_query_interface_proxy,
    ),
    stdcall("IUnknown_AddRef_Proxy", 20, 1, ndr::unknown_add_ref_proxy),
    stdcall("IUnknown_Release_Proxy", 22, 1, ndr::unknown_release_proxy),
    stdcall("NdrStubInitialize", 242, 4, ndr::stub_initialize),
    stdcall("NdrStubGetBuffer", 241, 3, ndr::stub_get_buffer),
    stdcall(
        "CStdStubBuffer_QueryInterface",
        15,
        3,
        ndr::stub_query_interface,
    ),
    stdcall("CStdStubBuffer_AddRef", 7, 1, ndr::stub_add_ref),
    stdcall("NdrCStdStubBuffer_Release", 104, 2, ndr::stub_release),
    stdcall("CStdStubBuffer_Connect", 8, 2, ndr::stub_connect),
    stdcall("CStdStubBuffer_Disconnect", 12, 1, ndr::stub_disconnect),
    stdcall("CStdStubBuffer_Invoke", 13, 3, ndr::stub_invoke),
    stdcall(
        "CStdStubBuffer_IsIIDSupported",
        14,
        2,
        ndr::stub_is_iid_supported,
    ),
    stdcall("CStdStubBuffer_CountRefs", 9, 1, ndr::stub_count_refs),
    stdcall(
        "CStdStubBuffer_DebugServerQueryInterface",
        10,
        2,
        ndr::stub_debug_server_query_interface,
    ),
    stdcall(
        "CStdStubBuffer_DebugServerRelease",
        11,
        2,
        ndr::stub_debug_server_release,
    ),
    stdcall("NdrPointerBufferSize", 203, 3, ndr::pointer_buffer_size),
    stdcall("NdrPointerMarshall", 205, 3, ndr::pointer_marshall),
    stdcall("NdrPointerUnmarshall", 207, 4, ndr::pointer_unmarshall),
    stdcall("NdrPointerFree", 204, 3, ndr::pointer_free),
    stdcall(
        "NdrSimpleStructBufferSize",
        231,
        3,
        ndr::simple_struct_buffer_size,
    ),
    stdcall(
        "NdrSimpleStructMarshall",
        233,
        3,
        ndr::simple_struct_marshall,
    ),
    stdcall(
        "NdrSimpleStructUnmarshall",
        235,
        4,
        ndr::simple_struct_unmarshall,
    ),
    stdcall(
        "NdrInterfacePointerBufferSize",
        177,
        3,
        ndr::interface_pointer_buffer_size,
    ),
    stdcall(
        "NdrInterfacePointerMarshall",
        179,
        3,
        ndr::interface_pointer_marshall,
    ),
    stdcall(
        "NdrInterfacePointerUnmarshall",
        181,
        4,
        ndr::interface_pointer_unmarshall,
    ),
    stdcall(
        "NdrInterfacePointerFree",
        178,
        3,
        ndr::interface_pointer_free,
    ),
    stdcall("NdrConvert", 148, 2, ndr::convert),
    stdcall("NdrOleAllocate", 201, 1, ndr::ole_allocate),
    stdcall("NdrOleFree", 202, 1, ndr::ole_free),
    stdcall("RpcRaiseException", 329, 1, ndr::rpc_raise_exception),
    stdcall(
        "IPSFactoryBuffer_QueryInterface",
        0,
        3,
        ndr::factory_query_interface,
    ),
    stdcall("IPSFactoryBuffer_AddRef", 0, 1, ndr::factory_add_ref),
    stdcall("IPSFactoryBuffer_Release", 0, 1, ndr::factory_release),
    stdcall(
        "IPSFactoryBuffer_CreateProxy",
        0,
        5,
        ndr::factory_create_proxy,
    ),
    stdcall(
        "IPSFactoryBuffer_CreateStub",
        0,
        4,
        ndr::factory_create_stub,
    ),
];

/// The vtables, by their places in this list.
static VTABLES: &[Vtable] = &[Vtable {
    name: "IPSFactoryBuffer",
    methods: &[
        "IPSFactoryBuffer_QueryInterface",
        "IPSFactoryBuffer_AddRef",
        "IPSFactoryBuffer_Release",
        "IPSFactoryBuffer_CreateProxy",
        "IPSFactoryBuffer_CreateStub",
    ],
}];

fn dll_index() -> usize {
    super::stubs::DLLS
        .iter()
        .position(|d| d.name == DLL.name)
        .expect("RPCRT4 is a built-in DLL")
}

/// The address of RPCRT4's function `name`.
pub fn address_of(name: &str) -> u32 {
    let index = APIS
        .iter()
        .position(|a| a.name == name)
        .unwrap_or_else(|| panic!("RPCRT4 has no {}", name));
    super::stubs::stub_address(dll_index(), index)
}

/// The address of RPCRT4's vtable `name`.
pub fn vtable_address(name: &str) -> u32 {
    let index = VTABLES
        .iter()
        .position(|v| v.name == name)
        .unwrap_or_else(|| panic!("RPCRT4 has no vtable {}", name));
    super::stubs::vtable_address(dll_index(), index)
}
