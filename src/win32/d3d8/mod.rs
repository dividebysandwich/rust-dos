//! D3D8.DLL: Direct3D 8, as DirectX 8.1 had it on Windows 98 with a 3dfx
//! Banshee or Voodoo3: IDirect3D8 and the device, swap chains, surfaces,
//! textures and vertex and index buffers it makes.
//!
//! It is a layer over DirectDraw's Direct3D (`ddraw::d3d`). Each device
//! has a DirectDraw object of its own and a Direct3D device of version 8
//! in DirectDraw's table, which draws with the HAL on the card as
//! Direct3D 7's devices do; its surfaces and texture levels are
//! DirectDraw surfaces of that object. Direct3D 8's render states are
//! the inner device's, by the same numbers; its texture stages,
//! transforms and the rest go there through `states.rs`, which keeps
//! what Direct3D 8 has and Direct3D 7 doesn't.
//!
//! Each object here has one interface block (com.rs) and one reference
//! count, kept in D3D8's own table. A resource holds a reference on its
//! device while its count isn't 0. The swap chain's back buffers and the
//! automatic depth buffer start with a count of 0 and stay when it is 0
//! again, as do a render target, depth buffer, texture or buffer the
//! device is drawing with; they go with the device (or when the device
//! lets go of them). A texture's levels count on their texture.
//!
//! The only device type is the HAL, with software vertex processing:
//! transform, lighting and vertex shaders (vs.1.0 and 1.1, `shader.rs`)
//! run on the processor. There are no pixel shaders, cube or volume
//! textures, or reference rasterizer. On a machine whose display adapter
//! isn't a Banshee or Voodoo3 (one with a Voodoo Graphics or Voodoo2
//! add-on card) there is no device: CreateDevice fails with
//! D3DERR_NOTAVAILABLE.
//!
//! Parts of this follow Wine 1.2.3's d3d8 (device.c, directx.c,
//! vertexdeclaration.c and others), which is under the GNU Lesser General
//! Public License 2.1 or later: Copyright (c) 2002-2004 Jason Edmeades,
//! Copyright (c) 2003-2004 Raphael Junqueira, Copyright (c) 2004
//! Christian Costa, Copyright (c) 2005 Oliver Stieber, Copyright (c)
//! 2006-2008 Stefan Dösinger, Copyright (c) 2007 Henri Verbeet, and
//! others.

/// The value of a `Result` whose error is an HRESULT, or the method
/// returns the error.
macro_rules! hr {
    ($e:expr) => {
        match $e {
            Ok(v) => v,
            Err(e) => return Some(e),
        }
    };
}

mod caps;
pub mod copy;
mod device;
mod direct3d;
mod draw;
mod resource;
mod shader;
mod states;

use super::com::{self, Guid};
use super::stubs::{Api, Conv, Dll, Vtable};
use crate::cpu::Cpu;

pub use device::Device;
pub use resource::{Buffer, Private, Surface, SwapChain, Texture};

pub static DLL: Dll = Dll {
    name: "D3D8.DLL",
    base: 0xBFF0_0000,
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

/// A method: stdcall, not exported.
const fn method(name: &'static str, args: u8, handler: super::stubs::Handler) -> Api {
    stdcall(name, 0, args, handler)
}

/// The functions. Append only (see `stubs`).
static APIS: &[Api] = &[
    stdcall("D3D8GetSWInfo", 1, 0, d3d8_get_sw_info),
    stdcall("DebugSetMute", 2, 0, debug_set_mute),
    stdcall("Direct3DCreate8", 3, 1, direct3d::direct3d_create8),
    stdcall("ValidatePixelShader", 4, 4, validate_pixel_shader),
    stdcall("ValidateVertexShader", 5, 5, validate_vertex_shader),
    method("D3D8_QueryInterface", 3, query_interface),
    method("D3D8_AddRef", 1, add_ref_method),
    method("D3D8_Release", 1, release_method),
    method(
        "IDirect3D8_RegisterSoftwareDevice",
        2,
        direct3d::register_software_device,
    ),
    method("IDirect3D8_GetAdapterCount", 1, direct3d::get_adapter_count),
    method(
        "IDirect3D8_GetAdapterIdentifier",
        4,
        direct3d::get_adapter_identifier,
    ),
    method(
        "IDirect3D8_GetAdapterModeCount",
        2,
        direct3d::get_adapter_mode_count,
    ),
    method(
        "IDirect3D8_EnumAdapterModes",
        4,
        direct3d::enum_adapter_modes,
    ),
    method(
        "IDirect3D8_GetAdapterDisplayMode",
        3,
        direct3d::get_adapter_display_mode,
    ),
    method("IDirect3D8_CheckDeviceType", 6, direct3d::check_device_type),
    method(
        "IDirect3D8_CheckDeviceFormat",
        7,
        direct3d::check_device_format,
    ),
    method(
        "IDirect3D8_CheckDeviceMultiSampleType",
        6,
        direct3d::check_multi_sample_type,
    ),
    method(
        "IDirect3D8_CheckDepthStencilMatch",
        6,
        direct3d::check_depth_stencil_match,
    ),
    method("IDirect3D8_GetDeviceCaps", 4, direct3d::get_device_caps),
    method(
        "IDirect3D8_GetAdapterMonitor",
        2,
        direct3d::get_adapter_monitor,
    ),
    method("IDirect3D8_CreateDevice", 7, direct3d::create_device),
    method(
        "Device8_TestCooperativeLevel",
        1,
        device::test_cooperative_level,
    ),
    method(
        "Device8_GetAvailableTextureMem",
        1,
        device::get_available_texture_mem,
    ),
    method(
        "Device8_ResourceManagerDiscardBytes",
        2,
        device::discard_bytes,
    ),
    method("Device8_GetDirect3D", 2, device::get_direct3d),
    method("Device8_GetDeviceCaps", 2, device::get_device_caps),
    method("Device8_GetDisplayMode", 2, device::get_display_mode),
    method(
        "Device8_GetCreationParameters",
        2,
        device::get_creation_parameters,
    ),
    method(
        "Device8_SetCursorProperties",
        4,
        device::set_cursor_properties,
    ),
    method("Device8_SetCursorPosition", 4, device::set_cursor_position),
    method("Device8_ShowCursor", 2, device::show_cursor),
    method(
        "Device8_CreateAdditionalSwapChain",
        3,
        device::create_additional_swap_chain,
    ),
    method("Device8_Reset", 2, device::reset),
    method("Device8_Present", 5, device::present),
    method("Device8_GetBackBuffer", 4, device::get_back_buffer),
    method("Device8_GetRasterStatus", 2, device::get_raster_status),
    method("Device8_SetGammaRamp", 3, device::set_gamma_ramp),
    method("Device8_GetGammaRamp", 2, device::get_gamma_ramp),
    method("Device8_CreateTexture", 8, resource::create_texture),
    method(
        "Device8_CreateVolumeTexture",
        9,
        resource::create_volume_texture,
    ),
    method(
        "Device8_CreateCubeTexture",
        7,
        resource::create_cube_texture,
    ),
    method(
        "Device8_CreateVertexBuffer",
        6,
        resource::create_vertex_buffer,
    ),
    method(
        "Device8_CreateIndexBuffer",
        6,
        resource::create_index_buffer,
    ),
    method(
        "Device8_CreateRenderTarget",
        7,
        resource::create_render_target,
    ),
    method(
        "Device8_CreateDepthStencilSurface",
        6,
        resource::create_depth_stencil,
    ),
    method(
        "Device8_CreateImageSurface",
        5,
        resource::create_image_surface,
    ),
    method("Device8_CopyRects", 6, copy::copy_rects),
    method("Device8_UpdateTexture", 3, copy::update_texture),
    method("Device8_GetFrontBuffer", 2, copy::get_front_buffer),
    method("Device8_SetRenderTarget", 3, device::set_render_target),
    method("Device8_GetRenderTarget", 2, device::get_render_target),
    method(
        "Device8_GetDepthStencilSurface",
        2,
        device::get_depth_stencil_surface,
    ),
    method("Device8_BeginScene", 1, device::begin_scene),
    method("Device8_EndScene", 1, device::end_scene),
    method("Device8_Clear", 7, device::clear),
    method("Device8_SetTransform", 3, states::set_transform),
    method("Device8_GetTransform", 3, states::get_transform),
    method("Device8_MultiplyTransform", 3, states::multiply_transform),
    method("Device8_SetViewport", 2, states::set_viewport),
    method("Device8_GetViewport", 2, states::get_viewport),
    method("Device8_SetMaterial", 2, states::set_material),
    method("Device8_GetMaterial", 2, states::get_material),
    method("Device8_SetLight", 3, states::set_light),
    method("Device8_GetLight", 3, states::get_light),
    method("Device8_LightEnable", 3, states::light_enable),
    method("Device8_GetLightEnable", 3, states::get_light_enable),
    method("Device8_SetClipPlane", 3, states::set_clip_plane),
    method("Device8_GetClipPlane", 3, states::get_clip_plane),
    method("Device8_SetRenderState", 3, states::set_render_state),
    method("Device8_GetRenderState", 3, states::get_render_state),
    method("Device8_BeginStateBlock", 1, states::begin_state_block),
    method("Device8_EndStateBlock", 2, states::end_state_block),
    method("Device8_ApplyStateBlock", 2, states::apply_state_block),
    method("Device8_CaptureStateBlock", 2, states::capture_state_block),
    method("Device8_DeleteStateBlock", 2, states::delete_state_block),
    method("Device8_CreateStateBlock", 3, states::create_state_block),
    method("Device8_SetClipStatus", 2, states::set_clip_status),
    method("Device8_GetClipStatus", 2, states::get_clip_status),
    method("Device8_GetTexture", 3, states::get_texture),
    method("Device8_SetTexture", 3, states::set_texture),
    method(
        "Device8_GetTextureStageState",
        4,
        states::get_texture_stage_state,
    ),
    method(
        "Device8_SetTextureStageState",
        4,
        states::set_texture_stage_state,
    ),
    method("Device8_ValidateDevice", 2, device::validate_device),
    method("Device8_GetInfo", 4, device::get_info),
    method("Device8_SetPaletteEntries", 3, states::set_palette_entries),
    method("Device8_GetPaletteEntries", 3, states::get_palette_entries),
    method(
        "Device8_SetCurrentTexturePalette",
        2,
        states::set_current_texture_palette,
    ),
    method(
        "Device8_GetCurrentTexturePalette",
        2,
        states::get_current_texture_palette,
    ),
    method("Device8_DrawPrimitive", 4, draw::draw_primitive),
    method(
        "Device8_DrawIndexedPrimitive",
        6,
        draw::draw_indexed_primitive,
    ),
    method("Device8_DrawPrimitiveUP", 5, draw::draw_primitive_up),
    method(
        "Device8_DrawIndexedPrimitiveUP",
        9,
        draw::draw_indexed_primitive_up,
    ),
    method("Device8_ProcessVertices", 6, draw::process_vertices),
    method(
        "Device8_CreateVertexShader",
        5,
        shader::create_vertex_shader,
    ),
    method("Device8_SetVertexShader", 2, states::set_vertex_shader),
    method("Device8_GetVertexShader", 2, states::get_vertex_shader),
    method(
        "Device8_DeleteVertexShader",
        2,
        shader::delete_vertex_shader,
    ),
    method(
        "Device8_SetVertexShaderConstant",
        4,
        states::set_vertex_shader_constant,
    ),
    method(
        "Device8_GetVertexShaderConstant",
        4,
        states::get_vertex_shader_constant,
    ),
    method(
        "Device8_GetVertexShaderDeclaration",
        4,
        shader::get_declaration,
    ),
    method("Device8_GetVertexShaderFunction", 4, shader::get_function),
    method("Device8_SetStreamSource", 4, states::set_stream_source),
    method("Device8_GetStreamSource", 4, states::get_stream_source),
    method("Device8_SetIndices", 3, states::set_indices),
    method("Device8_GetIndices", 3, states::get_indices),
    method("Device8_CreatePixelShader", 3, shader::create_pixel_shader),
    method("Device8_SetPixelShader", 2, states::set_pixel_shader),
    method("Device8_GetPixelShader", 2, states::get_pixel_shader),
    method("Device8_DeletePixelShader", 2, shader::delete_pixel_shader),
    method(
        "Device8_SetPixelShaderConstant",
        4,
        states::set_pixel_shader_constant,
    ),
    method(
        "Device8_GetPixelShaderConstant",
        4,
        states::get_pixel_shader_constant,
    ),
    method(
        "Device8_GetPixelShaderFunction",
        4,
        shader::get_pixel_function,
    ),
    method("Device8_DrawRectPatch", 4, draw::draw_patch),
    method("Device8_DrawTriPatch", 4, draw::draw_patch),
    method("Device8_DeletePatch", 2, draw::delete_patch),
    method("SwapChain8_Present", 5, device::swap_chain_present),
    method(
        "SwapChain8_GetBackBuffer",
        4,
        device::swap_chain_get_back_buffer,
    ),
    method("Resource8_GetDevice", 2, resource::get_device),
    method("Resource8_SetPrivateData", 5, resource::set_private_data),
    method("Resource8_GetPrivateData", 4, resource::get_private_data),
    method("Resource8_FreePrivateData", 2, resource::free_private_data),
    method("Resource8_SetPriority", 2, resource::set_priority),
    method("Resource8_GetPriority", 1, resource::get_priority),
    method("Resource8_PreLoad", 1, resource::preload),
    method("Resource8_GetType", 1, resource::get_type),
    method("Surface8_GetContainer", 3, resource::get_container),
    method("Surface8_GetDesc", 2, resource::surface_get_desc),
    method("Surface8_LockRect", 4, resource::surface_lock_rect),
    method("Surface8_UnlockRect", 1, resource::surface_unlock_rect),
    method("BaseTexture8_SetLOD", 2, resource::set_lod),
    method("BaseTexture8_GetLOD", 1, resource::get_lod),
    method("BaseTexture8_GetLevelCount", 1, resource::get_level_count),
    method("Texture8_GetLevelDesc", 3, resource::get_level_desc),
    method("Texture8_GetSurfaceLevel", 3, resource::get_surface_level),
    method("Texture8_LockRect", 5, resource::texture_lock_rect),
    method("Texture8_UnlockRect", 2, resource::texture_unlock_rect),
    method("Texture8_AddDirtyRect", 2, resource::add_dirty_rect),
    method("Buffer8_Lock", 5, resource::buffer_lock),
    method("Buffer8_Unlock", 1, resource::buffer_unlock),
    method("VertexBuffer8_GetDesc", 2, resource::vertex_buffer_get_desc),
    method("IndexBuffer8_GetDesc", 2, resource::index_buffer_get_desc),
];

const IUNKNOWN: [&str; 3] = ["D3D8_QueryInterface", "D3D8_AddRef", "D3D8_Release"];

const RESOURCE: [&str; 8] = [
    "Resource8_GetDevice",
    "Resource8_SetPrivateData",
    "Resource8_GetPrivateData",
    "Resource8_FreePrivateData",
    "Resource8_SetPriority",
    "Resource8_GetPriority",
    "Resource8_PreLoad",
    "Resource8_GetType",
];

/// Lists of names one after the other, as one.
const fn concat<const A: usize, const B: usize, const N: usize>(
    a: [&'static str; A],
    b: [&'static str; B],
) -> [&'static str; N] {
    let mut out = [""; N];
    let mut i = 0;
    while i < A {
        out[i] = a[i];
        i += 1;
    }
    while i < A + B {
        out[i] = b[i - A];
        i += 1;
    }
    out
}

const DIRECT3D8: [&str; 16] = concat(
    IUNKNOWN,
    [
        "IDirect3D8_RegisterSoftwareDevice",
        "IDirect3D8_GetAdapterCount",
        "IDirect3D8_GetAdapterIdentifier",
        "IDirect3D8_GetAdapterModeCount",
        "IDirect3D8_EnumAdapterModes",
        "IDirect3D8_GetAdapterDisplayMode",
        "IDirect3D8_CheckDeviceType",
        "IDirect3D8_CheckDeviceFormat",
        "IDirect3D8_CheckDeviceMultiSampleType",
        "IDirect3D8_CheckDepthStencilMatch",
        "IDirect3D8_GetDeviceCaps",
        "IDirect3D8_GetAdapterMonitor",
        "IDirect3D8_CreateDevice",
    ],
);

const DEVICE8: [&str; 97] = concat(
    IUNKNOWN,
    [
        "Device8_TestCooperativeLevel",
        "Device8_GetAvailableTextureMem",
        "Device8_ResourceManagerDiscardBytes",
        "Device8_GetDirect3D",
        "Device8_GetDeviceCaps",
        "Device8_GetDisplayMode",
        "Device8_GetCreationParameters",
        "Device8_SetCursorProperties",
        "Device8_SetCursorPosition",
        "Device8_ShowCursor",
        "Device8_CreateAdditionalSwapChain",
        "Device8_Reset",
        "Device8_Present",
        "Device8_GetBackBuffer",
        "Device8_GetRasterStatus",
        "Device8_SetGammaRamp",
        "Device8_GetGammaRamp",
        "Device8_CreateTexture",
        "Device8_CreateVolumeTexture",
        "Device8_CreateCubeTexture",
        "Device8_CreateVertexBuffer",
        "Device8_CreateIndexBuffer",
        "Device8_CreateRenderTarget",
        "Device8_CreateDepthStencilSurface",
        "Device8_CreateImageSurface",
        "Device8_CopyRects",
        "Device8_UpdateTexture",
        "Device8_GetFrontBuffer",
        "Device8_SetRenderTarget",
        "Device8_GetRenderTarget",
        "Device8_GetDepthStencilSurface",
        "Device8_BeginScene",
        "Device8_EndScene",
        "Device8_Clear",
        "Device8_SetTransform",
        "Device8_GetTransform",
        "Device8_MultiplyTransform",
        "Device8_SetViewport",
        "Device8_GetViewport",
        "Device8_SetMaterial",
        "Device8_GetMaterial",
        "Device8_SetLight",
        "Device8_GetLight",
        "Device8_LightEnable",
        "Device8_GetLightEnable",
        "Device8_SetClipPlane",
        "Device8_GetClipPlane",
        "Device8_SetRenderState",
        "Device8_GetRenderState",
        "Device8_BeginStateBlock",
        "Device8_EndStateBlock",
        "Device8_ApplyStateBlock",
        "Device8_CaptureStateBlock",
        "Device8_DeleteStateBlock",
        "Device8_CreateStateBlock",
        "Device8_SetClipStatus",
        "Device8_GetClipStatus",
        "Device8_GetTexture",
        "Device8_SetTexture",
        "Device8_GetTextureStageState",
        "Device8_SetTextureStageState",
        "Device8_ValidateDevice",
        "Device8_GetInfo",
        "Device8_SetPaletteEntries",
        "Device8_GetPaletteEntries",
        "Device8_SetCurrentTexturePalette",
        "Device8_GetCurrentTexturePalette",
        "Device8_DrawPrimitive",
        "Device8_DrawIndexedPrimitive",
        "Device8_DrawPrimitiveUP",
        "Device8_DrawIndexedPrimitiveUP",
        "Device8_ProcessVertices",
        "Device8_CreateVertexShader",
        "Device8_SetVertexShader",
        "Device8_GetVertexShader",
        "Device8_DeleteVertexShader",
        "Device8_SetVertexShaderConstant",
        "Device8_GetVertexShaderConstant",
        "Device8_GetVertexShaderDeclaration",
        "Device8_GetVertexShaderFunction",
        "Device8_SetStreamSource",
        "Device8_GetStreamSource",
        "Device8_SetIndices",
        "Device8_GetIndices",
        "Device8_CreatePixelShader",
        "Device8_SetPixelShader",
        "Device8_GetPixelShader",
        "Device8_DeletePixelShader",
        "Device8_SetPixelShaderConstant",
        "Device8_GetPixelShaderConstant",
        "Device8_GetPixelShaderFunction",
        "Device8_DrawRectPatch",
        "Device8_DrawTriPatch",
        "Device8_DeletePatch",
    ],
);

const SWAPCHAIN8: [&str; 5] = concat(IUNKNOWN, ["SwapChain8_Present", "SwapChain8_GetBackBuffer"]);

const SURFACE8: [&str; 11] = concat(
    IUNKNOWN,
    [
        "Resource8_GetDevice",
        "Resource8_SetPrivateData",
        "Resource8_GetPrivateData",
        "Resource8_FreePrivateData",
        "Surface8_GetContainer",
        "Surface8_GetDesc",
        "Surface8_LockRect",
        "Surface8_UnlockRect",
    ],
);

const RESOURCE8: [&str; 11] = concat(IUNKNOWN, RESOURCE);

const BASETEXTURE8: [&str; 14] = concat(
    RESOURCE8,
    [
        "BaseTexture8_SetLOD",
        "BaseTexture8_GetLOD",
        "BaseTexture8_GetLevelCount",
    ],
);

const TEXTURE8: [&str; 19] = concat(
    BASETEXTURE8,
    [
        "Texture8_GetLevelDesc",
        "Texture8_GetSurfaceLevel",
        "Texture8_LockRect",
        "Texture8_UnlockRect",
        "Texture8_AddDirtyRect",
    ],
);

const VERTEXBUFFER8: [&str; 14] = concat(
    RESOURCE8,
    ["Buffer8_Lock", "Buffer8_Unlock", "VertexBuffer8_GetDesc"],
);

const INDEXBUFFER8: [&str; 14] = concat(
    RESOURCE8,
    ["Buffer8_Lock", "Buffer8_Unlock", "IndexBuffer8_GetDesc"],
);

/// The vtables, by their places in this list. Append only.
static VTABLES: &[Vtable] = &[
    Vtable {
        name: "IDirect3D8",
        methods: &DIRECT3D8,
    },
    Vtable {
        name: "IDirect3DDevice8",
        methods: &DEVICE8,
    },
    Vtable {
        name: "IDirect3DSwapChain8",
        methods: &SWAPCHAIN8,
    },
    Vtable {
        name: "IDirect3DSurface8",
        methods: &SURFACE8,
    },
    Vtable {
        name: "IDirect3DTexture8",
        methods: &TEXTURE8,
    },
    Vtable {
        name: "IDirect3DVertexBuffer8",
        methods: &VERTEXBUFFER8,
    },
    Vtable {
        name: "IDirect3DIndexBuffer8",
        methods: &INDEXBUFFER8,
    },
];

pub const V_DIRECT3D8: usize = 0;
pub const V_DEVICE8: usize = 1;
pub const V_SWAPCHAIN8: usize = 2;
pub const V_SURFACE8: usize = 3;
pub const V_TEXTURE8: usize = 4;
pub const V_VERTEXBUFFER8: usize = 5;
pub const V_INDEXBUFFER8: usize = 6;

// HRESULTs.

pub const D3D_OK: u32 = 0;
const fn d3derr(code: u32) -> u32 {
    0x8876_0000 | code
}
pub const D3DERR_UNSUPPORTEDCOLOROPERATION: u32 = d3derr(2073);
pub const D3DERR_UNSUPPORTEDALPHAOPERATION: u32 = d3derr(2075);
pub const D3DERR_TOOMANYOPERATIONS: u32 = d3derr(2077);
pub const D3DERR_NOTFOUND: u32 = d3derr(2150);
pub const D3DERR_MOREDATA: u32 = d3derr(2151);
pub const D3DERR_NOTAVAILABLE: u32 = d3derr(2154);
pub const D3DERR_OUTOFVIDEOMEMORY: u32 = d3derr(380);
pub const D3DERR_INVALIDCALL: u32 = d3derr(2156);

// Interfaces.

const fn guid(d1: u32, d2: u16, d3: u16, d4: [u8; 8]) -> Guid {
    Guid::new(d1, d2, d3, d4)
}
pub const IID_DIRECT3D8: Guid = guid(
    0x1DD9_E8DA,
    0x1C77,
    0x4D40,
    [0xB0, 0xCF, 0x98, 0xFE, 0xFD, 0xFF, 0x95, 0x12],
);
pub const IID_DEVICE8: Guid = guid(
    0x7385_E5DF,
    0x8FE8,
    0x41D5,
    [0x86, 0xB6, 0xD7, 0xB4, 0x85, 0x47, 0xB6, 0xCF],
);
pub const IID_RESOURCE8: Guid = guid(
    0x1B36_BB7B,
    0x09B7,
    0x410A,
    [0xB4, 0x45, 0x7D, 0x14, 0x30, 0xD7, 0xB3, 0x3F],
);
pub const IID_VERTEXBUFFER8: Guid = guid(
    0x8AEE_EAC7,
    0x05F9,
    0x44D4,
    [0xB5, 0x91, 0x00, 0x0B, 0x0D, 0xF1, 0xCB, 0x95],
);
pub const IID_SWAPCHAIN8: Guid = guid(
    0x928C_088B,
    0x76B9,
    0x4C6B,
    [0xA5, 0x36, 0xA5, 0x90, 0x85, 0x38, 0x76, 0xCD],
);
pub const IID_SURFACE8: Guid = guid(
    0xB96E_EBCA,
    0xB326,
    0x4EA5,
    [0x88, 0x2F, 0x2F, 0xF5, 0xBA, 0xE0, 0x21, 0xDD],
);
pub const IID_INDEXBUFFER8: Guid = guid(
    0x0E68_9C9A,
    0x053D,
    0x44A0,
    [0x9D, 0x92, 0xDB, 0x0E, 0x3D, 0x75, 0x0F, 0x86],
);
pub const IID_BASETEXTURE8: Guid = guid(
    0xB421_1CFA,
    0x51B9,
    0x4A9F,
    [0xAB, 0x78, 0xDB, 0x99, 0xB2, 0xBB, 0x67, 0x8E],
);
pub const IID_TEXTURE8: Guid = guid(
    0xE4CD_D575,
    0x2866,
    0x4F01,
    [0xB1, 0x2E, 0x7E, 0xEC, 0xE1, 0xEC, 0x93, 0x58],
);

// The state.

/// An object of Direct3D 8's: its interface and reference count, the
/// device it belongs to, and what its kind has.
#[derive(Clone, Debug, Default)]
pub struct Object {
    /// The process it is in.
    pub pid: u32,
    /// Its interface block, and that block's vtable.
    pub iface: u32,
    pub vtable: u32,
    pub refs: u32,
    /// The device it belongs to (0 for IDirect3D8 and the devices).
    pub device: u32,
    /// Whether it holds a reference on `device` now.
    pub holds: bool,
    /// The texture whose count a texture's level counts on, or 0.
    pub container: u32,
    /// The swap chain whose back buffer it is, or 0.
    pub swapchain: u32,
    /// Whether it stays at a count of 0 until its device or swap chain
    /// goes: an implicit surface.
    pub implicit: bool,
    pub private: Vec<Private>,
    pub priority: u32,
    /// What it is: one of these.
    pub d3d: bool,
    pub dev: Option<Device>,
    pub surface: Option<Surface>,
    pub texture: Option<Texture>,
    pub buffer: Option<Buffer>,
    pub chain: Option<SwapChain>,
}

/// Direct3D 8's state: the system's.
#[derive(Clone, Debug, Default)]
pub struct State {
    /// The objects, by their ids less 1.
    pub objects: Vec<Option<Object>>,
}

pub fn state(cpu: &mut Cpu) -> &mut State {
    &mut cpu.bus.win32.d3d8
}

fn dll_index() -> usize {
    super::stubs::DLLS
        .iter()
        .position(|d| d.name == DLL.name)
        .expect("D3D8 is a built-in DLL")
}

impl State {
    pub fn object(&self, id: u32) -> Option<&Object> {
        self.objects.get(id.checked_sub(1)? as usize)?.as_ref()
    }

    pub fn object_mut(&mut self, id: u32) -> Option<&mut Object> {
        self.objects.get_mut(id.checked_sub(1)? as usize)?.as_mut()
    }

    /// The ids of the objects for which `f` holds.
    pub fn ids(&self, f: impl Fn(&Object) -> bool) -> Vec<u32> {
        self.objects
            .iter()
            .enumerate()
            .filter(|(_, o)| o.as_ref().is_some_and(&f))
            .map(|(i, _)| i as u32 + 1)
            .collect()
    }
}

pub fn object(cpu: &mut Cpu, id: u32) -> Option<&mut Object> {
    state(cpu).object_mut(id)
}

/// Put `object` in the table, as the running process's, with its
/// interface `vtable` and `refs` references: its id, or 0 when the heap
/// is full.
pub fn add(cpu: &mut Cpu, mut object: Object, vtable: usize, refs: u32) -> u32 {
    object.pid = super::user32::current_pid(cpu);
    object.vtable = vtable as u32;
    object.refs = refs;
    let s = state(cpu);
    let id = match s.objects.iter().position(Option::is_none) {
        Some(i) => {
            s.objects[i] = Some(object);
            i as u32 + 1
        }
        None => {
            s.objects.push(Some(object));
            s.objects.len() as u32
        }
    };
    let Some(iface) = com::new_interface(cpu, dll_index(), vtable, id, 0) else {
        state(cpu).objects[id as usize - 1] = None;
        return 0;
    };
    let holds = refs > 0;
    let device = {
        let o = state(cpu).object_mut(id).expect("just added");
        o.iface = iface;
        o.holds = holds && o.device != 0;
        o.device
    };
    if holds && device != 0 {
        add_ref(cpu, device);
    }
    id
}

/// The object `this` is an interface of: its vtable's place and its id.
pub fn of(cpu: &mut Cpu, this: u32) -> Option<(usize, u32)> {
    let (vtable, id) = com::interface_of(cpu, dll_index(), this)?;
    let o = state(cpu).object(id)?;
    (o.iface == this).then_some((vtable, id))
}

/// The interface of the object `id`.
pub fn iface(cpu: &mut Cpu, id: u32) -> u32 {
    state(cpu).object(id).map_or(0, |o| o.iface)
}

/// Give the program the interface of the object `id` at `out`, with a
/// reference: false if `out` can't be written.
pub fn give(cpu: &mut Cpu, id: u32, out: u32) -> bool {
    let at = iface(cpu, id);
    if !com::write_out(cpu, out, at) {
        return false;
    }
    add_ref(cpu, id);
    true
}

/// Add a reference on the object `id`: its new count.
pub fn add_ref(cpu: &mut Cpu, id: u32) -> u32 {
    let Some(o) = state(cpu).object_mut(id) else {
        return 0;
    };
    if o.container != 0 {
        let c = o.container;
        return add_ref(cpu, c);
    }
    o.refs += 1;
    let refs = o.refs;
    let device = o.device;
    if refs == 1 && !o.holds && device != 0 {
        o.holds = true;
        add_ref(cpu, device);
    }
    refs
}

/// Take a reference off the object `id`: its new count. It goes at 0,
/// unless it is kept (`kept`).
pub fn release(cpu: &mut Cpu, id: u32) -> u32 {
    let Some(o) = state(cpu).object_mut(id) else {
        return 0;
    };
    if o.container != 0 {
        let c = o.container;
        return release(cpu, c);
    }
    if o.refs == 0 {
        return 0;
    }
    o.refs -= 1;
    if o.refs > 0 {
        return o.refs;
    }
    let device = o.device;
    let holds = std::mem::take(&mut o.holds);
    if !kept(cpu, id) {
        destroy(cpu, id);
    }
    if holds {
        release(cpu, device);
    }
    0
}

/// Whether the object `id`, at a count of 0, stays: an implicit surface,
/// or one its device draws with.
fn kept(cpu: &mut Cpu, id: u32) -> bool {
    let Some(o) = state(cpu).object(id) else {
        return false;
    };
    if o.implicit {
        return true;
    }
    let device = o.device;
    if device == 0 {
        return false;
    }
    device::uses(cpu, device, id)
}

/// The device let go of the object `id`: it goes if nothing has it.
pub fn let_go(cpu: &mut Cpu, id: u32) {
    let refs = state(cpu).object(id).map(|o| (o.refs, o.container));
    if let Some((0, 0)) = refs
        && !kept(cpu, id)
    {
        destroy(cpu, id);
    }
}

/// The object `id` goes, and what it holds is let go.
pub fn destroy(cpu: &mut Cpu, id: u32) {
    let Some(o) = state(cpu).object(id).cloned() else {
        return;
    };
    if let Some(slot) = state(cpu).objects.get_mut(id as usize - 1) {
        *slot = None;
    }
    if o.dev.is_some() {
        device::destroyed(cpu, id, &o);
    }
    if o.surface.is_some() || o.texture.is_some() || o.buffer.is_some() || o.chain.is_some() {
        resource::destroyed(cpu, id, &o);
    }
    resource::release_private(cpu, &o.private);
    com::free_interface(cpu, o.iface);
    if o.d3d {
        // Nothing more: the devices hold references on it.
    }
}

/// A process ended: its objects go. DirectDraw's state goes with its own
/// `process_ended`, which takes what the devices had there.
pub fn process_ended(cpu: &mut Cpu, pid: u32) {
    for slot in state(cpu).objects.iter_mut() {
        if slot.as_ref().is_some_and(|o| o.pid == pid) {
            *slot = None;
        }
    }
}

// The exported functions.

/// D3D8GetSWInfo(): what the reference rasterizer would say.
fn d3d8_get_sw_info(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(0)
}

/// DebugSetMute(): the debug runtime's, which there is none of.
fn debug_set_mute(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(0)
}

const E_FAIL: u32 = 0x8000_4005;

/// ValidateVertexShader(function, reserved1, reserved2, flag, out): a
/// vs.1.0 or 1.1 shader, with the reserved arguments 0.
fn validate_vertex_shader(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [at, r1, r2, _, _] = args[..] else {
        return Some(E_FAIL);
    };
    if at == 0 || r1 != 0 || r2 != 0 || !cpu.bus.guest_probe(at, 4, false) {
        return Some(E_FAIL);
    }
    let version = cpu.bus.guest_read_32(at);
    Some(if matches!(version, 0xFFFE_0100 | 0xFFFE_0101) {
        com::S_OK
    } else {
        E_FAIL
    })
}

/// ValidatePixelShader(function, reserved, flag, out): ps.1.0 to 1.4.
fn validate_pixel_shader(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [at, r1, _, _] = args[..] else {
        return Some(E_FAIL);
    };
    if at == 0 || r1 != 0 || !cpu.bus.guest_probe(at, 4, false) {
        return Some(E_FAIL);
    }
    let version = cpu.bus.guest_read_32(at);
    Some(if (0xFFFF_0100..=0xFFFF_0104).contains(&version) {
        com::S_OK
    } else {
        E_FAIL
    })
}

// IUnknown, which all the interfaces share.

/// Whether the interface `iid` is one an object of the kind of `vtable`
/// has.
fn has_interface(vtable: usize, iid: &Guid) -> bool {
    if *iid == com::IID_IUNKNOWN {
        return true;
    }
    match vtable {
        V_DIRECT3D8 => *iid == IID_DIRECT3D8,
        V_DEVICE8 => *iid == IID_DEVICE8,
        V_SWAPCHAIN8 => *iid == IID_SWAPCHAIN8,
        V_SURFACE8 => *iid == IID_SURFACE8 || *iid == IID_RESOURCE8,
        V_TEXTURE8 => [IID_RESOURCE8, IID_BASETEXTURE8, IID_TEXTURE8].contains(iid),
        V_VERTEXBUFFER8 => *iid == IID_RESOURCE8 || *iid == IID_VERTEXBUFFER8,
        V_INDEXBUFFER8 => *iid == IID_RESOURCE8 || *iid == IID_INDEXBUFFER8,
        _ => false,
    }
}

/// QueryInterface for the object `id`: the HRESULT, with the interface
/// or NULL at `out`.
pub fn query(cpu: &mut Cpu, id: u32, iid: &Guid, out: u32) -> u32 {
    let vtable = state(cpu)
        .object(id)
        .map_or(usize::MAX, |o| o.vtable as usize);
    if has_interface(vtable, iid) {
        give(cpu, id, out);
        return com::S_OK;
    }
    com::write_out(cpu, out, 0);
    com::E_NOINTERFACE
}

/// QueryInterface(this, iid, out).
fn query_interface(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, iid, out] = args[..] else {
        return Some(D3DERR_INVALIDCALL);
    };
    let Some((_, id)) = of(cpu, this) else {
        return Some(D3DERR_INVALIDCALL);
    };
    if out == 0 || !cpu.bus.guest_probe(out, 4, true) {
        return Some(com::E_POINTER);
    }
    let Some(iid) = Guid::read(cpu, iid) else {
        com::write_out(cpu, out, 0);
        return Some(com::E_POINTER);
    };
    Some(query(cpu, id, &iid, out))
}

fn add_ref_method(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some((_, id)) = of(cpu, args[0]) else {
        return Some(0);
    };
    Some(add_ref(cpu, id))
}

fn release_method(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some((_, id)) = of(cpu, args[0]) else {
        return Some(0);
    };
    Some(release(cpu, id))
}

crate::state_fields!(Object {
    pid,
    iface,
    vtable,
    refs,
    device,
    holds,
    container,
    swapchain,
    implicit,
    private,
    priority,
    d3d,
    dev,
    surface,
    texture,
    buffer,
    chain
});
crate::state_fields!(State { objects });
