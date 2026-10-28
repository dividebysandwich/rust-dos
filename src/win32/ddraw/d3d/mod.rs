//! Direct3D: IDirect3D to IDirect3D7 and the objects they make, as
//! DirectX 6 and 7 had them on Windows 9x with a 3dfx card.
//!
//! IDirect3D, 2 and 3 are interfaces of a DirectDraw object made with
//! DirectDrawCreate, IDirect3D7 one of an object made with
//! DirectDrawCreateEx; they count their references on the DirectDraw
//! interface they come from (IDirectDraw's, or IDirectDraw7's).
//! IDirect3DTexture and 2 are interfaces of a surface, counted on its
//! IDirectDrawSurface. Devices, viewports, materials, lights, execute
//! buffers and vertex buffers are objects of their own in DirectDraw's
//! table, each with a single count for all its interfaces.
//!
//! The only device is the card's HAL ("Direct3D HAL"), there when the
//! display adapter is a Banshee or Voodoo3 (`hal`). There is no software
//! rasterizer: the RGB, Ramp and MMX devices aren't there, nor the
//! reference rasterizer or a device with transform and lighting in
//! hardware. Transform, lighting, clipping and fog are done on the
//! processor (`pipeline.rs`), as Windows' Direct3D did them for the
//! card; what comes out goes to the card's rasterizer.
//!
//! Parts of this are ported from Wine 1.2.3's ddraw (device.c,
//! executebuffer.c, viewport.c and others), which is under the GNU
//! Lesser General Public License 2.1 or later:
//!
//! Copyright (c) 1998-2004 Lionel Ulmer, Copyright (c) 2002-2005
//! Christian Costa, Copyright (c) 2006 Stefan Dösinger, Copyright (c)
//! 2008 Alexander Dorofeyev, and others.

pub mod caps;
pub mod device;
pub mod direct3d;
pub mod draw;
pub mod execute;
pub mod hal;
pub mod light;
pub mod material;
pub mod pipeline;
pub mod shadow;
pub mod states;
pub mod texture;
pub mod vertexbuffer;
pub mod viewport;

use super::com::{self, Guid};
use super::{
    Object, V_D3D1, V_D3D2, V_D3D3, V_D3D7, V_DD1, V_DD7, V_DEVICE1, V_DEVICE2, V_DEVICE3,
    V_DEVICE7, V_EXECBUF, V_GAMMA, V_LIGHT, V_MATERIAL1, V_MATERIAL2, V_MATERIAL3, V_SURFACE1,
    V_TEXTURE1, V_TEXTURE2, V_VB1, V_VB7, V_VIEWPORT1, V_VIEWPORT2, V_VIEWPORT3, state,
};
use crate::cpu::Cpu;

pub use device::Device;
pub use execute::ExecuteBuffer;
pub use light::LightObject;
pub use material::MaterialObject;
pub use vertexbuffer::VertexBuffer;
pub use viewport::Viewport;

/// What a Direct3D object is: one of these is there.
#[derive(Clone, Debug, Default)]
pub struct D3dObject {
    pub device: Option<Device>,
    pub viewport: Option<Viewport>,
    pub material: Option<MaterialObject>,
    pub light: Option<LightObject>,
    pub execute: Option<ExecuteBuffer>,
    pub buffer: Option<VertexBuffer>,
}

crate::state_fields!(D3dObject {
    device,
    viewport,
    material,
    light,
    execute,
    buffer
});

// HRESULTs.

pub const D3D_OK: u32 = 0;
pub const fn d3derr(code: u32) -> u32 {
    0x8876_0000 | code
}
pub const D3DERR_EXECUTE_LOCKED: u32 = d3derr(714);
pub const D3DERR_EXECUTE_NOT_LOCKED: u32 = d3derr(715);
pub const D3DERR_TEXTURE_LOAD_FAILED: u32 = d3derr(725);
pub const D3DERR_INVALIDPRIMITIVETYPE: u32 = d3derr(736);
pub const D3DERR_INVALIDVERTEXTYPE: u32 = d3derr(737);
pub const D3DERR_SURFACENOTINVIDMEM: u32 = d3derr(747);
pub const D3DERR_LIGHTHASVIEWPORT: u32 = d3derr(751);
pub const D3DERR_SCENE_IN_SCENE: u32 = d3derr(760);
pub const D3DERR_SCENE_NOT_IN_SCENE: u32 = d3derr(761);
pub const D3DERR_INBEGIN: u32 = d3derr(770);
pub const D3DERR_NOTINBEGIN: u32 = d3derr(771);
pub const D3DERR_NOVIEWPORTS: u32 = d3derr(772);
pub const D3DERR_VIEWPORTHASNODEVICE: u32 = d3derr(774);
pub const D3DERR_NOCURRENTVIEWPORT: u32 = d3derr(775);
pub const D3DERR_VERTEXBUFFEROPTIMIZED: u32 = d3derr(2060);
pub const D3DERR_VERTEXBUFFERLOCKED: u32 = d3derr(2062);
pub const D3DERR_ZBUFFER_NOTPRESENT: u32 = d3derr(2070);
pub const D3DERR_STENCILBUFFER_NOTPRESENT: u32 = d3derr(2071);
pub const D3DERR_UNSUPPORTEDCOLOROPERATION: u32 = d3derr(2073);
pub const D3DERR_UNSUPPORTEDALPHAOPERATION: u32 = d3derr(2075);
pub const D3DERR_TOOMANYOPERATIONS: u32 = d3derr(2077);
pub const D3DERR_INVALIDSTATEBLOCK: u32 = d3derr(2100);
pub const D3DERR_INBEGINSTATEBLOCK: u32 = d3derr(2101);
pub const D3DERR_NOTINBEGINSTATEBLOCK: u32 = d3derr(2102);
pub const DDERR_NOTONMIPMAPSUBLEVEL: u32 = d3derr(603);

// Interfaces and devices.

const fn guid(d1: u32, d2: u16, d3: u16, d4: [u8; 8]) -> Guid {
    Guid::new(d1, d2, d3, d4)
}
const A31A: [u8; 8] = [0xA3, 0x1A, 0x00, 0xAA, 0x00, 0xB9, 0x33, 0x56];
const DEV5: [u8; 8] = [0x89, 0xAB, 0x00, 0xA0, 0xC9, 0x05, 0x41, 0x29];
const DX7: [u8; 8] = [0xA4, 0x07, 0x00, 0xA0, 0xC9, 0x06, 0x29, 0xA8];
const LIGHTS: [u8; 8] = [0x81, 0x6F, 0x00, 0x00, 0xC0, 0x20, 0x15, 0x6E];

pub const IID_D3D1: Guid = guid(0x3BBA_0080, 0x2421, 0x11CF, A31A);
pub const IID_D3D2: Guid = guid(
    0x6AAE_1EC1,
    0x662A,
    0x11D0,
    [0x88, 0x9D, 0x00, 0xAA, 0x00, 0xBB, 0xB7, 0x6A],
);
pub const IID_D3D3: Guid = guid(
    0xBB22_3240,
    0xE72B,
    0x11D0,
    [0xA9, 0xB4, 0x00, 0xAA, 0x00, 0xC0, 0x99, 0x3E],
);
pub const IID_D3D7: Guid = guid(0xF504_9E77, 0x4861, 0x11D2, DX7);
pub const IID_RAMP: Guid = guid(0xF208_6B20, 0x259F, 0x11CF, A31A);
pub const IID_RGB: Guid = guid(0xA466_5C60, 0x2673, 0x11CF, A31A);
pub const IID_HAL: Guid = guid(0x84E6_3DE0, 0x46AA, 0x11CF, LIGHTS);
pub const IID_MMX: Guid = guid(0x8819_49A1, 0xD6F3, 0x11D0, DEV5);
pub const IID_REF: Guid = guid(
    0x5093_6643,
    0x13E9,
    0x11D1,
    [0x89, 0xAA, 0x00, 0xA0, 0xC9, 0x05, 0x41, 0x29],
);
pub const IID_TNLHAL: Guid = guid(0xF504_9E78, 0x4861, 0x11D2, DX7);
pub const IID_NULL_DEVICE: Guid = guid(
    0x8767_DF22,
    0xBACC,
    0x11D1,
    [0x89, 0x69, 0x00, 0xA0, 0xC9, 0x06, 0x29, 0xA8],
);
pub const IID_DEVICE1: Guid = guid(0x6410_8800, 0x957D, 0x11D0, DEV5);
pub const IID_DEVICE2: Guid = guid(0x9328_1501, 0x8CF8, 0x11D0, DEV5);
pub const IID_DEVICE3: Guid = guid(
    0xB0AB_3B60,
    0x33D7,
    0x11D1,
    [0xA9, 0x81, 0x00, 0xC0, 0x4F, 0xD7, 0xB1, 0x74],
);
pub const IID_DEVICE7: Guid = guid(0xF504_9E79, 0x4861, 0x11D2, DX7);
pub const IID_TEXTURE1: Guid = guid(0x2CDC_D9E0, 0x25A0, 0x11CF, A31A);
pub const IID_TEXTURE2: Guid = guid(0x9328_1502, 0x8CF8, 0x11D0, DEV5);
pub const IID_LIGHT: Guid = guid(0x4417_C142, 0x33AD, 0x11CF, LIGHTS);
pub const IID_MATERIAL1: Guid = guid(0x4417_C144, 0x33AD, 0x11CF, LIGHTS);
pub const IID_MATERIAL2: Guid = guid(0x9328_1503, 0x8CF8, 0x11D0, DEV5);
pub const IID_MATERIAL3: Guid = guid(
    0xCA9C_46F4,
    0xD3C5,
    0x11D1,
    [0xB7, 0x5A, 0x00, 0x60, 0x08, 0x52, 0xB3, 0x12],
);
pub const IID_EXECBUF: Guid = guid(0x4417_C145, 0x33AD, 0x11CF, LIGHTS);
pub const IID_VIEWPORT1: Guid = guid(0x4417_C146, 0x33AD, 0x11CF, LIGHTS);
pub const IID_VIEWPORT2: Guid = guid(0x9328_1500, 0x8CF8, 0x11D0, DEV5);
pub const IID_VIEWPORT3: Guid = guid(
    0xB0AB_3B61,
    0x33D7,
    0x11D1,
    [0xA9, 0x81, 0x00, 0xC0, 0x4F, 0xD7, 0xB1, 0x74],
);
pub const IID_VB1: Guid = guid(
    0x7A50_3555,
    0x4A83,
    0x11D1,
    [0xA5, 0xDB, 0x00, 0xA0, 0xC9, 0x03, 0x67, 0xF8],
);
pub const IID_VB7: Guid = guid(0xF504_9E7D, 0x4861, 0x11D2, DX7);
pub const IID_GAMMA: Guid = guid(
    0x69C1_1C3E,
    0xB46B,
    0x11D1,
    [0xAD, 0x7A, 0x00, 0xC0, 0x4F, 0xC2, 0x9B, 0x4E],
);

/// The device GUIDs Direct3D knows, with none of them there but the HAL.
const DEVICES: [Guid; 7] = [
    IID_HAL,
    IID_RGB,
    IID_RAMP,
    IID_MMX,
    IID_REF,
    IID_TNLHAL,
    IID_NULL_DEVICE,
];

pub fn is_device_guid(g: &Guid) -> bool {
    DEVICES.contains(g)
}

// The vtables' methods, for DDRAW's VTABLES.

const IUNKNOWN: [&str; 3] = ["DD_QueryInterface", "DD_AddRef", "DD_Release"];

pub const D3D1: [&str; 9] = super::concat3(
    IUNKNOWN,
    ["IDirect3D_Initialize"],
    [
        "IDirect3D_EnumDevices",
        "IDirect3D_CreateLight",
        "IDirect3D_CreateMaterial",
        "IDirect3D_CreateViewport",
        "IDirect3D_FindDevice",
    ],
);
pub const D3D2: [&str; 9] = super::concat3(
    IUNKNOWN,
    [
        "IDirect3D_EnumDevices",
        "IDirect3D_CreateLight",
        "IDirect3D_CreateMaterial",
        "IDirect3D_CreateViewport",
        "IDirect3D_FindDevice",
    ],
    ["IDirect3D2_CreateDevice"],
);
pub const D3D3: [&str; 12] = super::concat3(
    IUNKNOWN,
    [
        "IDirect3D_EnumDevices",
        "IDirect3D_CreateLight",
        "IDirect3D_CreateMaterial",
        "IDirect3D_CreateViewport",
        "IDirect3D_FindDevice",
    ],
    [
        "IDirect3D3_CreateDevice",
        "IDirect3D3_CreateVertexBuffer",
        "IDirect3D3_EnumZBufferFormats",
        "IDirect3D3_EvictManagedTextures",
    ],
);
pub const D3D7: [&str; 8] = super::concat3(
    IUNKNOWN,
    [
        "IDirect3D7_EnumDevices",
        "IDirect3D7_CreateDevice",
        "IDirect3D7_CreateVertexBuffer",
    ],
    [
        "IDirect3D3_EnumZBufferFormats",
        "IDirect3D3_EvictManagedTextures",
    ],
);

pub const DEVICE1: [&str; 22] = super::concat3(
    IUNKNOWN,
    [
        "IDirect3DDevice_Initialize",
        "IDirect3DDevice_GetCaps",
        "IDirect3DDevice_SwapTextureHandles",
        "IDirect3DDevice_CreateExecuteBuffer",
        "IDirect3DDevice_GetStats",
        "IDirect3DDevice_Execute",
        "IDirect3DDevice_AddViewport",
        "IDirect3DDevice_DeleteViewport",
        "IDirect3DDevice_NextViewport",
        "IDirect3DDevice_Pick",
        "IDirect3DDevice_GetPickRecords",
        "IDirect3DDevice_EnumTextureFormats",
        "IDirect3DDevice_CreateMatrix",
        "IDirect3DDevice_SetMatrix",
        "IDirect3DDevice_GetMatrix",
        "IDirect3DDevice_DeleteMatrix",
    ],
    [
        "IDirect3DDevice_BeginScene",
        "IDirect3DDevice_EndScene",
        "IDirect3DDevice_GetDirect3D",
    ],
);

/// IDirect3DDevice2's methods from SetCurrentViewport to GetClipStatus,
/// which IDirect3DDevice3 has too.
const DEVICE2_TAIL: [&str; 20] = [
    "IDirect3DDevice2_SetCurrentViewport",
    "IDirect3DDevice2_GetCurrentViewport",
    "IDirect3DDevice2_SetRenderTarget",
    "IDirect3DDevice2_GetRenderTarget",
    "IDirect3DDevice2_Begin",
    "IDirect3DDevice2_BeginIndexed",
    "IDirect3DDevice2_Vertex",
    "IDirect3DDevice2_Index",
    "IDirect3DDevice2_End",
    "IDirect3DDevice2_GetRenderState",
    "IDirect3DDevice2_SetRenderState",
    "IDirect3DDevice2_GetLightState",
    "IDirect3DDevice2_SetLightState",
    "IDirect3DDevice2_SetTransform",
    "IDirect3DDevice2_GetTransform",
    "IDirect3DDevice2_MultiplyTransform",
    "IDirect3DDevice2_DrawPrimitive",
    "IDirect3DDevice2_DrawIndexedPrimitive",
    "IDirect3DDevice2_SetClipStatus",
    "IDirect3DDevice2_GetClipStatus",
];

pub const DEVICE2: [&str; 33] = super::concat3(
    IUNKNOWN,
    [
        "IDirect3DDevice_GetCaps",
        "IDirect3DDevice_SwapTextureHandles",
        "IDirect3DDevice_GetStats",
        "IDirect3DDevice_AddViewport",
        "IDirect3DDevice_DeleteViewport",
        "IDirect3DDevice_NextViewport",
        "IDirect3DDevice_EnumTextureFormats",
        "IDirect3DDevice_BeginScene",
        "IDirect3DDevice_EndScene",
        "IDirect3DDevice_GetDirect3D",
    ],
    DEVICE2_TAIL,
);

const DEVICE3_HEAD: [&str; 32] = super::concat3(
    IUNKNOWN,
    [
        "IDirect3DDevice_GetCaps",
        "IDirect3DDevice_GetStats",
        "IDirect3DDevice_AddViewport",
        "IDirect3DDevice_DeleteViewport",
        "IDirect3DDevice_NextViewport",
        "IDirect3DDevice_EnumTextureFormats",
        "IDirect3DDevice_BeginScene",
        "IDirect3DDevice_EndScene",
        "IDirect3DDevice_GetDirect3D",
    ],
    DEVICE2_TAIL,
);

pub const DEVICE3: [&str; 42] = super::concat3(
    DEVICE3_HEAD,
    [
        "IDirect3DDevice3_DrawPrimitiveStrided",
        "IDirect3DDevice3_DrawIndexedPrimitiveStrided",
        "IDirect3DDevice3_DrawPrimitiveVB",
        "IDirect3DDevice3_DrawIndexedPrimitiveVB",
        "IDirect3DDevice3_ComputeSphereVisibility",
    ],
    [
        "IDirect3DDevice3_GetTexture",
        "IDirect3DDevice3_SetTexture",
        "IDirect3DDevice3_GetTextureStageState",
        "IDirect3DDevice3_SetTextureStageState",
        "IDirect3DDevice3_ValidateDevice",
    ],
);

pub const DEVICE7: [&str; 49] = super::concat3(
    IUNKNOWN,
    [
        "IDirect3DDevice7_GetCaps",
        "IDirect3DDevice7_EnumTextureFormats",
        "IDirect3DDevice7_BeginScene",
        "IDirect3DDevice7_EndScene",
        "IDirect3DDevice7_GetDirect3D",
        "IDirect3DDevice7_SetRenderTarget",
        "IDirect3DDevice7_GetRenderTarget",
        "IDirect3DDevice7_Clear",
        "IDirect3DDevice7_SetTransform",
        "IDirect3DDevice7_GetTransform",
        "IDirect3DDevice7_SetViewport",
        "IDirect3DDevice7_MultiplyTransform",
        "IDirect3DDevice7_GetViewport",
        "IDirect3DDevice7_SetMaterial",
        "IDirect3DDevice7_GetMaterial",
        "IDirect3DDevice7_SetLight",
        "IDirect3DDevice7_GetLight",
        "IDirect3DDevice7_SetRenderState",
        "IDirect3DDevice7_GetRenderState",
        "IDirect3DDevice7_BeginStateBlock",
        "IDirect3DDevice7_EndStateBlock",
        "IDirect3DDevice7_PreLoad",
        "IDirect3DDevice7_DrawPrimitive",
        "IDirect3DDevice7_DrawIndexedPrimitive",
        "IDirect3DDevice7_SetClipStatus",
        "IDirect3DDevice7_GetClipStatus",
        "IDirect3DDevice7_DrawPrimitiveStrided",
        "IDirect3DDevice7_DrawIndexedPrimitiveStrided",
        "IDirect3DDevice7_DrawPrimitiveVB",
        "IDirect3DDevice7_DrawIndexedPrimitiveVB",
        "IDirect3DDevice7_ComputeSphereVisibility",
    ],
    [
        "IDirect3DDevice7_GetTexture",
        "IDirect3DDevice7_SetTexture",
        "IDirect3DDevice7_GetTextureStageState",
        "IDirect3DDevice7_SetTextureStageState",
        "IDirect3DDevice7_ValidateDevice",
        "IDirect3DDevice7_ApplyStateBlock",
        "IDirect3DDevice7_CaptureStateBlock",
        "IDirect3DDevice7_DeleteStateBlock",
        "IDirect3DDevice7_CreateStateBlock",
        "IDirect3DDevice7_Load",
        "IDirect3DDevice7_LightEnable",
        "IDirect3DDevice7_GetLightEnable",
        "IDirect3DDevice7_SetClipPlane",
        "IDirect3DDevice7_GetClipPlane",
        "IDirect3DDevice7_GetInfo",
    ],
);

pub const VIEWPORT3: [&str; 21] = super::concat3(
    IUNKNOWN,
    [
        "IDirect3DViewport_Initialize",
        "IDirect3DViewport_GetViewport",
        "IDirect3DViewport_SetViewport",
        "IDirect3DViewport_TransformVertices",
        "IDirect3DViewport_LightElements",
        "IDirect3DViewport_SetBackground",
        "IDirect3DViewport_GetBackground",
        "IDirect3DViewport_SetBackgroundDepth",
        "IDirect3DViewport_GetBackgroundDepth",
        "IDirect3DViewport_Clear",
        "IDirect3DViewport_AddLight",
        "IDirect3DViewport_DeleteLight",
        "IDirect3DViewport_NextLight",
        "IDirect3DViewport2_GetViewport2",
        "IDirect3DViewport2_SetViewport2",
    ],
    [
        "IDirect3DViewport3_SetBackgroundDepth2",
        "IDirect3DViewport3_GetBackgroundDepth2",
        "IDirect3DViewport3_Clear2",
    ],
);

/// The first `N` methods of IDirect3DViewport3: IDirect3DViewport's
/// (16) and IDirect3DViewport2's (18).
pub const fn viewport<const N: usize>() -> [&'static str; N] {
    let mut out = [""; N];
    let mut i = 0;
    while i < N {
        out[i] = VIEWPORT3[i];
        i += 1;
    }
    out
}

pub const MATERIAL1: [&str; 9] = super::concat3(
    IUNKNOWN,
    [
        "IDirect3DMaterial_Initialize",
        "IDirect3DMaterial_SetMaterial",
        "IDirect3DMaterial_GetMaterial",
    ],
    [
        "IDirect3DMaterial_GetHandle",
        "IDirect3DMaterial_Reserve",
        "IDirect3DMaterial_Unreserve",
    ],
);
pub const MATERIAL2: [&str; 6] = super::concat3(
    IUNKNOWN,
    [
        "IDirect3DMaterial_SetMaterial",
        "IDirect3DMaterial_GetMaterial",
    ],
    ["IDirect3DMaterial_GetHandle"],
);
pub const LIGHT: [&str; 6] = super::concat3(
    IUNKNOWN,
    ["IDirect3DLight_Initialize"],
    ["IDirect3DLight_SetLight", "IDirect3DLight_GetLight"],
);
pub const EXECBUF: [&str; 10] = super::concat3(
    IUNKNOWN,
    [
        "IDirect3DExecuteBuffer_Initialize",
        "IDirect3DExecuteBuffer_Lock",
        "IDirect3DExecuteBuffer_Unlock",
        "IDirect3DExecuteBuffer_SetExecuteData",
    ],
    [
        "IDirect3DExecuteBuffer_GetExecuteData",
        "IDirect3DExecuteBuffer_Validate",
        "IDirect3DExecuteBuffer_Optimize",
    ],
);
pub const TEXTURE1: [&str; 8] = super::concat3(
    IUNKNOWN,
    [
        "IDirect3DTexture_Initialize",
        "IDirect3DTexture_GetHandle",
        "IDirect3DTexture_PaletteChanged",
    ],
    ["IDirect3DTexture_Load", "IDirect3DTexture_Unload"],
);
pub const TEXTURE2: [&str; 6] = super::concat3(
    IUNKNOWN,
    [
        "IDirect3DTexture_GetHandle",
        "IDirect3DTexture_PaletteChanged",
    ],
    ["IDirect3DTexture_Load"],
);
pub const VB1: [&str; 8] = super::concat3(
    IUNKNOWN,
    [
        "IDirect3DVertexBuffer_Lock",
        "IDirect3DVertexBuffer_Unlock",
        "IDirect3DVertexBuffer_ProcessVertices",
    ],
    [
        "IDirect3DVertexBuffer_GetVertexBufferDesc",
        "IDirect3DVertexBuffer_Optimize",
    ],
);
pub const VB7: [&str; 9] =
    super::concat3(VB1, ["IDirect3DVertexBuffer7_ProcessVerticesStrided"], []);
pub const GAMMA: [&str; 5] = super::concat3(
    IUNKNOWN,
    ["DirectDrawGammaControl_GetGammaRamp"],
    ["DirectDrawGammaControl_SetGammaRamp"],
);

// The guest's memory.

/// Whether `len` bytes at `at` can be read (or written).
pub fn probe(cpu: &mut Cpu, at: u32, len: u32, write: bool) -> bool {
    at != 0 && cpu.bus.guest_probe(at, len as usize, write)
}

pub fn read_f32(cpu: &mut Cpu, at: u32) -> f32 {
    f32::from_bits(cpu.bus.guest_read_32(at))
}

pub fn write_f32(cpu: &mut Cpu, at: u32, v: f32) {
    cpu.bus.guest_write_32(at, v.to_bits());
}

/// `N` dwords at `at`, which the caller has probed.
pub fn read_dwords<const N: usize>(cpu: &mut Cpu, at: u32) -> [u32; N] {
    let mut out = [0; N];
    for (i, o) in out.iter_mut().enumerate() {
        *o = cpu.bus.guest_read_32(at + 4 * i as u32);
    }
    out
}

pub fn write_dwords(cpu: &mut Cpu, at: u32, words: &[u32]) {
    for (i, w) in words.iter().enumerate() {
        cpu.bus.guest_write_32(at + 4 * i as u32, *w);
    }
}

/// A D3DMATRIX at `at`, which the caller has probed.
pub fn read_matrix(cpu: &mut Cpu, at: u32) -> pipeline::Matrix {
    read_dwords::<16>(cpu, at).map(f32::from_bits)
}

pub fn write_matrix(cpu: &mut Cpu, at: u32, m: &pipeline::Matrix) {
    write_dwords(cpu, at, &m.map(f32::to_bits));
}

/// The bytes at `at`, `len` of them; zeros where there is no memory.
pub fn read_bytes(cpu: &mut Cpu, at: u32, len: u32) -> Vec<u8> {
    let mut out = vec![0; len as usize];
    if probe(cpu, at, len, false) {
        cpu.bus.guest_read_bytes(at, &mut out);
    }
    out
}

// The objects.

/// The Direct3D object `id`.
pub fn object(cpu: &mut Cpu, id: u32) -> Option<&mut D3dObject> {
    state(cpu).object_mut(id)?.d3d.as_mut()
}

/// Put a new Direct3D object of the DirectDraw object `ddraw` in the
/// table, and give the program its interface `vtable` at `out`: D3D_OK,
/// or the error, with the object gone again.
pub fn add(cpu: &mut Cpu, ddraw: u32, d3d: D3dObject, vtable: usize, out: u32) -> Result<u32, u32> {
    let id = super::add_object(
        cpu,
        Object {
            owner: ddraw,
            d3d: Some(d3d),
            ..Object::default()
        },
    );
    let hr = super::give(cpu, id, vtable, out);
    if hr != D3D_OK {
        super::destroy(cpu, id);
        return Err(hr);
    }
    Ok(id)
}

/// The Direct3D object `this` is an interface of: its vtable and id.
pub fn of(cpu: &mut Cpu, this: u32) -> Option<(usize, u32)> {
    let (vtable, id) = super::this(cpu, this)?;
    state(cpu).object(id)?.d3d.as_ref()?;
    Some((vtable, id))
}

/// The number of texture units of the card the DirectDraw object `dd`
/// draws on, if Direct3D has a HAL there: the display adapter's when it
/// is a Banshee (1) or a Voodoo3 (2), or the add-on card's for the 3dfx
/// device's DirectDraw object (a Voodoo Graphics or Voodoo2's).
pub fn hal(cpu: &mut Cpu, dd: u32) -> Option<u32> {
    if super::voodoo::is_card_object(cpu, dd) {
        return crate::win32::glide::hw::board(cpu).map(|b| b.tmus);
    }
    match cpu.bus.vga.adapter.voodoo_board()? {
        crate::voodoo::Board::Banshee => Some(1),
        _ => Some(2),
    }
}

/// Whether a DirectDraw object was made by DirectDrawCreateEx: its
/// first interface is IDirectDraw7.
fn made_ex(object: &Object) -> bool {
    object
        .ifaces
        .first()
        .is_some_and(|i| i.vtable == V_DD7 as u32)
}

/// The vtable whose reference count the interface `vtable` of the
/// object `id` counts on, if not its own.
pub fn shares(cpu: &mut Cpu, id: u32, vtable: usize) -> Option<usize> {
    let object = state(cpu).object(id)?;
    if object.d3d.is_some() {
        return object.ifaces.first().map(|i| i.vtable as usize);
    }
    match vtable {
        V_D3D1 | V_D3D2 | V_D3D3 => Some(V_DD1),
        V_D3D7 => Some(V_DD7),
        V_TEXTURE1 | V_TEXTURE2 => Some(V_SURFACE1),
        _ => None,
    }
}

/// The vtable whose kind of object `vtable` is, for QueryInterface's
/// IDirectDraw and IDirectDrawSurface interfaces: a Direct3D interface
/// of a DirectDraw object is one of a DirectDraw object, a texture one
/// of a surface.
pub fn base_vtable(vtable: usize) -> usize {
    match vtable {
        V_D3D1 | V_D3D2 | V_D3D3 => V_DD1,
        V_D3D7 => V_DD7,
        V_TEXTURE1 | V_TEXTURE2 | V_GAMMA => V_SURFACE1,
        v => v,
    }
}

/// QueryInterface's answer for `iid` on the interface `vtable` of the
/// object `id`, where Direct3D has a say: the vtable to give, or the
/// HRESULT to return (a device made of a surface is given at `out`
/// here). None leaves it to DirectDraw.
pub fn query(
    cpu: &mut Cpu,
    id: u32,
    vtable: usize,
    iid: &Guid,
    out: u32,
) -> Option<Result<usize, u32>> {
    let object = state(cpu).object(id)?.clone();
    if object.ddraw.is_some() {
        let ex = made_ex(&object);
        let asked = [
            (IID_D3D1, V_D3D1),
            (IID_D3D2, V_D3D2),
            (IID_D3D3, V_D3D3),
            (IID_D3D7, V_D3D7),
        ];
        let &(_, v) = asked.iter().find(|(g, _)| g == iid)?;
        let fine = if v == V_D3D7 {
            ex && matches!(vtable, V_DD7 | V_D3D7)
        } else {
            !ex && vtable != V_D3D7
        };
        return Some(if fine { Ok(v) } else { Err(com::E_NOINTERFACE) });
    }
    if object.surface.is_some() {
        if *iid == IID_TEXTURE1 {
            return Some(Ok(V_TEXTURE1));
        }
        if *iid == IID_TEXTURE2 {
            return Some(Ok(V_TEXTURE2));
        }
        if *iid == IID_GAMMA {
            return Some(Ok(V_GAMMA));
        }
        if is_device_guid(iid) {
            return Some(Err(direct3d::surface_device(cpu, id, vtable, iid, out)));
        }
        return None;
    }
    let d3d = object.d3d.as_ref()?;
    let kinds: &[(Guid, usize)] = if d3d.device.is_some() {
        let version = d3d.device.as_ref().map_or(7, |d| d.version);
        if version == 7 {
            &[(IID_DEVICE7, V_DEVICE7)]
        } else {
            &[
                (IID_DEVICE1, V_DEVICE1),
                (IID_DEVICE2, V_DEVICE2),
                (IID_DEVICE3, V_DEVICE3),
            ]
        }
    } else if d3d.viewport.is_some() {
        &[
            (IID_VIEWPORT1, V_VIEWPORT1),
            (IID_VIEWPORT2, V_VIEWPORT2),
            (IID_VIEWPORT3, V_VIEWPORT3),
        ]
    } else if d3d.material.is_some() {
        &[
            (IID_MATERIAL1, V_MATERIAL1),
            (IID_MATERIAL2, V_MATERIAL2),
            (IID_MATERIAL3, V_MATERIAL3),
        ]
    } else if d3d.light.is_some() {
        &[(IID_LIGHT, V_LIGHT)]
    } else if d3d.execute.is_some() {
        &[(IID_EXECBUF, V_EXECBUF)]
    } else if vtable == V_VB7 {
        &[(IID_VB7, V_VB7)]
    } else {
        &[(IID_VB1, V_VB1)]
    };
    if *iid == com::IID_IUNKNOWN {
        return Some(Ok(vtable));
    }
    Some(
        kinds
            .iter()
            .find(|(g, _)| g == iid)
            .map(|&(_, v)| v)
            .ok_or(com::E_NOINTERFACE),
    )
}

/// The Direct3D object `id` goes: what it holds is let go.
pub fn destroyed(cpu: &mut Cpu, id: u32) {
    let Some(d3d) = state(cpu).object(id).and_then(|o| o.d3d.clone()) else {
        return;
    };
    if let Some(d) = d3d.device {
        device::destroyed(cpu, id, d);
    }
    if let Some(v) = d3d.viewport {
        viewport::destroyed(cpu, id, v);
    }
    if let Some(l) = d3d.light {
        light::destroyed(cpu, id, l);
    }
    if let Some(e) = d3d.execute {
        execute::destroyed(cpu, e);
    }
    if let Some(b) = d3d.buffer {
        vertexbuffer::destroyed(cpu, b);
    }
    if d3d.material.is_some() {
        material::destroyed(cpu, id);
    }
}

/// The surface `id` goes: devices forget it as a texture's handle.
pub fn surface_destroyed(cpu: &mut Cpu, id: u32) {
    device::forget_handles(cpu, device::HANDLE_TEXTURE, id);
}
