//! D3DRM.DLL: Direct3D Retained Mode, which every DirectX up to 8 puts in
//! the system directory, Windows 98 SE's DirectX 6.1 among them. A game
//! that imports Direct3DRMCreate doesn't start without it, whether it
//! draws with Retained Mode or not: Warhammer: Dark Omen's CD release
//! imports it and draws with Direct3D's immediate mode.
//!
//! The DLL exports Direct3DRMCreate and the math helpers on vectors,
//! quaternions and colors, with the ordinals DirectX 3's and 5's
//! D3DRM.DLL give them (the same 21, in alphabetical order). The helpers
//! are written, and compute as Wine's d3drm does, in single precision.
//! Direct3DRMCreate makes the Direct3DRM object; of its methods only
//! IUnknown's are written yet, and a call of another does what a call of
//! a missing function does (`super::not_written`).

use super::com::{self, E_NOINTERFACE, E_NOTIMPL, E_OUTOFMEMORY, Guid};
use super::stubs::{Api, Conv, Dll, Var, Vtable};
use crate::cpu::Cpu;

pub static DLL: Dll = Dll {
    name: "D3DRM.DLL",
    base: 0xBF4E_0000,
    apis: APIS,
    vars: VARS,
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

/// A method: D3DRM's own code, not exported. `args` counts `this`.
const fn method(name: &'static str, args: u8, handler: super::stubs::Handler) -> Api {
    stdcall(name, 0, args, handler)
}

/// The functions, by D3DRM.DLL's ordinals, then the methods. Append only
/// (see `stubs`).
static APIS: &[Api] = &[
    stdcall("D3DRMColorGetAlpha", 1, 1, color_get_alpha),
    stdcall("D3DRMColorGetBlue", 2, 1, color_get_blue),
    stdcall("D3DRMColorGetGreen", 3, 1, color_get_green),
    stdcall("D3DRMColorGetRed", 4, 1, color_get_red),
    stdcall("D3DRMCreateColorRGB", 5, 3, create_color_rgb),
    stdcall("D3DRMCreateColorRGBA", 6, 4, create_color_rgba),
    stdcall("D3DRMMatrixFromQuaternion", 7, 2, matrix_from_quaternion),
    stdcall(
        "D3DRMQuaternionFromRotation",
        8,
        3,
        quaternion_from_rotation,
    ),
    stdcall("D3DRMQuaternionMultiply", 9, 3, quaternion_multiply),
    stdcall("D3DRMQuaternionSlerp", 10, 4, quaternion_slerp),
    stdcall("D3DRMVectorAdd", 11, 3, vector_add),
    stdcall("D3DRMVectorCrossProduct", 12, 3, vector_cross_product),
    stdcall("D3DRMVectorDotProduct", 13, 2, vector_dot_product),
    stdcall("D3DRMVectorModulus", 14, 1, vector_modulus),
    stdcall("D3DRMVectorNormalize", 15, 1, vector_normalize),
    stdcall("D3DRMVectorRandom", 16, 1, vector_random),
    stdcall("D3DRMVectorReflect", 17, 3, vector_reflect),
    stdcall("D3DRMVectorRotate", 18, 4, vector_rotate),
    stdcall("D3DRMVectorScale", 19, 3, vector_scale),
    stdcall("D3DRMVectorSubtract", 20, 3, vector_subtract),
    stdcall("Direct3DRMCreate", 21, 1, direct3drm_create),
    // IUnknown, of every interface.
    method("IUnknown_QueryInterface", 3, query_interface),
    method("IUnknown_AddRef", 1, add_ref),
    method("IUnknown_Release", 1, release),
    // IDirect3DRM.
    method("D3DRM_CreateObject", 5, rm_create_object),
    method("D3DRM_CreateFrame", 3, rm_create_frame),
    method("D3DRM_CreateMesh", 2, rm_create_mesh),
    method("D3DRM_CreateMeshBuilder", 2, rm_create_mesh_builder),
    method("D3DRM_CreateFace", 2, rm_create_face),
    method("D3DRM_CreateAnimation", 2, rm_create_animation),
    method("D3DRM_CreateAnimationSet", 2, rm_create_animation_set),
    method("D3DRM_CreateTexture", 3, rm_create_texture),
    method("D3DRM_CreateLight", 4, rm_create_light),
    method("D3DRM_CreateLightRGB", 6, rm_create_light_rgb),
    method("D3DRM_CreateMaterial", 3, rm_create_material),
    method("D3DRM_CreateDevice", 4, rm_create_device),
    method(
        "D3DRM_CreateDeviceFromSurface",
        5,
        rm_create_device_from_surface,
    ),
    method("D3DRM_CreateDeviceFromD3D", 4, rm_create_device_from_d3d),
    method(
        "D3DRM_CreateDeviceFromClipper",
        6,
        rm_create_device_from_clipper,
    ),
    method(
        "D3DRM_CreateTextureFromSurface",
        3,
        rm_create_texture_from_surface,
    ),
    method("D3DRM_CreateShadow", 10, rm_create_shadow),
    method("D3DRM_CreateViewport", 8, rm_create_viewport),
    method("D3DRM_CreateWrap", 17, rm_create_wrap),
    method("D3DRM_CreateUserVisual", 4, rm_create_user_visual),
    method("D3DRM_LoadTexture", 3, rm_load_texture),
    method(
        "D3DRM_LoadTextureFromResource",
        3,
        rm_load_texture_from_resource,
    ),
    method("D3DRM_SetSearchPath", 2, rm_set_search_path),
    method("D3DRM_AddSearchPath", 2, rm_add_search_path),
    method("D3DRM_GetSearchPath", 3, rm_get_search_path),
    method(
        "D3DRM_SetDefaultTextureColors",
        2,
        rm_set_default_texture_colors,
    ),
    method(
        "D3DRM_SetDefaultTextureShades",
        2,
        rm_set_default_texture_shades,
    ),
    method("D3DRM_GetDevices", 2, rm_get_devices),
    method("D3DRM_GetNamedObject", 3, rm_get_named_object),
    method("D3DRM_EnumerateObjects", 3, rm_enumerate_objects),
    method("D3DRM_Load", 11, rm_load),
    method("D3DRM_Tick", 2, rm_tick),
];

/// The vtables, in `VTABLES` order.
const VT_D3DRM: usize = 0;

static VTABLES: &[Vtable] = &[Vtable {
    name: "IDirect3DRM",
    methods: &[
        "IUnknown_QueryInterface",
        "IUnknown_AddRef",
        "IUnknown_Release",
        "D3DRM_CreateObject",
        "D3DRM_CreateFrame",
        "D3DRM_CreateMesh",
        "D3DRM_CreateMeshBuilder",
        "D3DRM_CreateFace",
        "D3DRM_CreateAnimation",
        "D3DRM_CreateAnimationSet",
        "D3DRM_CreateTexture",
        "D3DRM_CreateLight",
        "D3DRM_CreateLightRGB",
        "D3DRM_CreateMaterial",
        "D3DRM_CreateDevice",
        "D3DRM_CreateDeviceFromSurface",
        "D3DRM_CreateDeviceFromD3D",
        "D3DRM_CreateDeviceFromClipper",
        "D3DRM_CreateTextureFromSurface",
        "D3DRM_CreateShadow",
        "D3DRM_CreateViewport",
        "D3DRM_CreateWrap",
        "D3DRM_CreateUserVisual",
        "D3DRM_LoadTexture",
        "D3DRM_LoadTextureFromResource",
        "D3DRM_SetSearchPath",
        "D3DRM_AddSearchPath",
        "D3DRM_GetSearchPath",
        "D3DRM_SetDefaultTextureColors",
        "D3DRM_SetDefaultTextureShades",
        "D3DRM_GetDevices",
        "D3DRM_GetNamedObject",
        "D3DRM_EnumerateObjects",
        "D3DRM_Load",
        "D3DRM_Tick",
    ],
}];

/// The DLL's own state, in each process: the seed of D3DRMVectorRandom's
/// random numbers.
static VARS: &[Var] = &[Var {
    name: "seed",
    ordinal: 0,
    size: 4,
}];

const VAR_SEED: usize = 0;

pub const IID_IDIRECT3DRM: Guid = Guid::new(
    0x2BC4_9361,
    0x8327,
    0x11CF,
    [0xAC, 0x4A, 0x00, 0x00, 0xC0, 0x38, 0x25, 0xA1],
);

/// Bytes of an object's block after com.rs's header: its reference
/// count. The objects keep all their state in their blocks, so they need
/// none on the Rust side.
const EXTRA: u32 = 4;
const REFS: u32 = com::HEADER;

/// The place of this DLL in `stubs::DLLS`.
fn dll_index() -> usize {
    super::stubs::find_dll(DLL.name).expect("D3DRM.DLL is a built-in DLL")
}

// The methods not written yet. Each returns an HRESULT, so from guest code
// it fails with E_NOTIMPL: 0 would be D3DRM_OK.
super::not_written_handlers! {
    DLL.name;
    rm_create_object: "IDirect3DRM::CreateObject" => E_NOTIMPL,
    rm_create_frame: "IDirect3DRM::CreateFrame" => E_NOTIMPL,
    rm_create_mesh: "IDirect3DRM::CreateMesh" => E_NOTIMPL,
    rm_create_mesh_builder: "IDirect3DRM::CreateMeshBuilder" => E_NOTIMPL,
    rm_create_face: "IDirect3DRM::CreateFace" => E_NOTIMPL,
    rm_create_animation: "IDirect3DRM::CreateAnimation" => E_NOTIMPL,
    rm_create_animation_set: "IDirect3DRM::CreateAnimationSet" => E_NOTIMPL,
    rm_create_texture: "IDirect3DRM::CreateTexture" => E_NOTIMPL,
    rm_create_light: "IDirect3DRM::CreateLight" => E_NOTIMPL,
    rm_create_light_rgb: "IDirect3DRM::CreateLightRGB" => E_NOTIMPL,
    rm_create_material: "IDirect3DRM::CreateMaterial" => E_NOTIMPL,
    rm_create_device: "IDirect3DRM::CreateDevice" => E_NOTIMPL,
    rm_create_device_from_surface: "IDirect3DRM::CreateDeviceFromSurface" => E_NOTIMPL,
    rm_create_device_from_d3d: "IDirect3DRM::CreateDeviceFromD3D" => E_NOTIMPL,
    rm_create_device_from_clipper: "IDirect3DRM::CreateDeviceFromClipper" => E_NOTIMPL,
    rm_create_texture_from_surface: "IDirect3DRM::CreateTextureFromSurface" => E_NOTIMPL,
    rm_create_shadow: "IDirect3DRM::CreateShadow" => E_NOTIMPL,
    rm_create_viewport: "IDirect3DRM::CreateViewport" => E_NOTIMPL,
    rm_create_wrap: "IDirect3DRM::CreateWrap" => E_NOTIMPL,
    rm_create_user_visual: "IDirect3DRM::CreateUserVisual" => E_NOTIMPL,
    rm_load_texture: "IDirect3DRM::LoadTexture" => E_NOTIMPL,
    rm_load_texture_from_resource: "IDirect3DRM::LoadTextureFromResource" => E_NOTIMPL,
    rm_set_search_path: "IDirect3DRM::SetSearchPath" => E_NOTIMPL,
    rm_add_search_path: "IDirect3DRM::AddSearchPath" => E_NOTIMPL,
    rm_get_search_path: "IDirect3DRM::GetSearchPath" => E_NOTIMPL,
    rm_set_default_texture_colors: "IDirect3DRM::SetDefaultTextureColors" => E_NOTIMPL,
    rm_set_default_texture_shades: "IDirect3DRM::SetDefaultTextureShades" => E_NOTIMPL,
    rm_get_devices: "IDirect3DRM::GetDevices" => E_NOTIMPL,
    rm_get_named_object: "IDirect3DRM::GetNamedObject" => E_NOTIMPL,
    rm_enumerate_objects: "IDirect3DRM::EnumerateObjects" => E_NOTIMPL,
    rm_load: "IDirect3DRM::Load" => E_NOTIMPL,
    rm_tick: "IDirect3DRM::Tick" => E_NOTIMPL,
}

/// Direct3DRMCreate(lplpDirect3DRM): a new Direct3DRM object, its
/// IDirect3DRM at `lplpDirect3DRM`, with one reference.
fn direct3drm_create(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let out = args[0];
    if out == 0 || !cpu.bus.guest_probe(out, 4, true) {
        return Some(D3DRMERR_BADVALUE);
    }
    let Some(this) = com::new_interface(cpu, dll_index(), VT_D3DRM, 0, EXTRA) else {
        return Some(E_OUTOFMEMORY);
    };
    cpu.bus.guest_write_32(this + REFS, 1);
    com::write_out(cpu, out, this);
    Some(D3DRM_OK)
}

const D3DRM_OK: u32 = 0;
/// D3DRMERR_BADVALUE: MAKE_DDHRESULT(781).
const D3DRMERR_BADVALUE: u32 = 0x8876_030D;

/// The IDirect3DRM `this` is, if it is one of this DLL's objects.
fn d3drm_of(cpu: &mut Cpu, this: u32) -> Option<u32> {
    let (vtable, _) = com::interface_of(cpu, dll_index(), this)?;
    (vtable == VT_D3DRM).then_some(this)
}

fn query_interface(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, iid, out] = args[..] else {
        return Some(D3DRMERR_BADVALUE);
    };
    let Some(this) = d3drm_of(cpu, this) else {
        return Some(D3DRMERR_BADVALUE);
    };
    if !com::write_out(cpu, out, 0) {
        return Some(D3DRMERR_BADVALUE);
    }
    match Guid::read(cpu, iid) {
        Some(g) if g == com::IID_IUNKNOWN || g == IID_IDIRECT3DRM => {
            add_ref(cpu, &[this]);
            com::write_out(cpu, out, this);
            Some(D3DRM_OK)
        }
        _ => Some(E_NOINTERFACE),
    }
}

fn add_ref(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(this) = d3drm_of(cpu, args[0]) else {
        return Some(0);
    };
    let refs = cpu.bus.guest_read_32(this + REFS).wrapping_add(1);
    cpu.bus.guest_write_32(this + REFS, refs);
    Some(refs)
}

/// Release(this): the count left; the object goes with the last.
fn release(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(this) = d3drm_of(cpu, args[0]) else {
        return Some(0);
    };
    let refs = cpu.bus.guest_read_32(this + REFS).saturating_sub(1);
    if refs == 0 {
        com::free_interface(cpu, this);
    } else {
        cpu.bus.guest_write_32(this + REFS, refs);
    }
    Some(refs)
}

// The math helpers. D3DVALUE is a float, a D3DVECTOR three of them, a
// D3DRMQUATERNION the scalar and then the vector, a D3DRMMATRIX4D 16 in
// rows. Functions returning a D3DVALUE return it in ST(0); the others
// return the pointer they were given for the result.

type Vector = [f32; 3];

/// The float of the argument `word`.
fn float(word: u32) -> f32 {
    f32::from_bits(word)
}

/// The `N` floats at `at`, or None if a page of them is missing.
fn read_floats<const N: usize>(cpu: &mut Cpu, at: u32) -> Option<[f32; N]> {
    let mut out = [0.0; N];
    for (i, value) in out.iter_mut().enumerate() {
        *value = f32::from_bits(cpu.bus.guest_read_32(at.wrapping_add(4 * i as u32)));
    }
    (!cpu.bus.guest_faulted()).then_some(out)
}

fn write_floats(cpu: &mut Cpu, at: u32, values: &[f32]) {
    for (i, value) in values.iter().enumerate() {
        cpu.bus
            .guest_write_32(at.wrapping_add(4 * i as u32), value.to_bits());
    }
}

/// Return `value` as a D3DVALUE function does: in ST(0).
fn ret_float(cpu: &mut Cpu, value: f32) -> Option<u32> {
    cpu.fpu_push_f64(value as f64);
    Some(0)
}

fn add(a: Vector, b: Vector) -> Vector {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

fn sub(a: Vector, b: Vector) -> Vector {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn scale(a: Vector, factor: f32) -> Vector {
    [a[0] * factor, a[1] * factor, a[2] * factor]
}

fn dot(a: Vector, b: Vector) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn cross(a: Vector, b: Vector) -> Vector {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

/// The vector of length 1 in the direction of `v`; a vector of length 0
/// becomes (1, 0, 0).
fn normalize(v: Vector) -> Vector {
    let modulus = dot(v, v).sqrt();
    if modulus == 0.0 {
        [1.0, 0.0, 0.0]
    } else {
        scale(v, 1.0 / modulus)
    }
}

/// A quaternion: the scalar and the vector.
type Quaternion = (f32, Vector);

fn read_quaternion(cpu: &mut Cpu, at: u32) -> Option<Quaternion> {
    let [s, x, y, z] = read_floats::<4>(cpu, at)?;
    Some((s, [x, y, z]))
}

fn write_quaternion(cpu: &mut Cpu, at: u32, (s, v): Quaternion) {
    write_floats(cpu, at, &[s, v[0], v[1], v[2]]);
}

fn multiply((sa, va): Quaternion, (sb, vb): Quaternion) -> Quaternion {
    let c = cross(va, vb);
    (
        sa * sb - dot(va, vb),
        [
            sa * vb[0] + sb * va[0] + c[0],
            sa * vb[1] + sb * va[1] + c[1],
            sa * vb[2] + sb * va[2] + c[2],
        ],
    )
}

/// D3DRMColorGetAlpha(color): the alpha of the D3DCOLOR, 0.0 to 1.0.
fn color_get_alpha(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    ret_float(cpu, (args[0] >> 24 & 0xFF) as f32 / 255.0)
}

fn color_get_red(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    ret_float(cpu, (args[0] >> 16 & 0xFF) as f32 / 255.0)
}

fn color_get_green(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    ret_float(cpu, (args[0] >> 8 & 0xFF) as f32 / 255.0)
}

fn color_get_blue(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    ret_float(cpu, (args[0] & 0xFF) as f32 / 255.0)
}

/// A component of a D3DCOLOR from 0.0 to 1.0, clamped, and cut to a
/// whole number of 255ths.
fn component(value: f32) -> u32 {
    if value <= 0.0 {
        0
    } else if value >= 1.0 {
        0xFF
    } else {
        (value * 255.0) as u32
    }
}

fn color(red: f32, green: f32, blue: f32, alpha: f32) -> u32 {
    component(alpha) << 24 | component(red) << 16 | component(green) << 8 | component(blue)
}

/// D3DRMCreateColorRGB(red, green, blue): the opaque D3DCOLOR.
fn create_color_rgb(_: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(color(float(args[0]), float(args[1]), float(args[2]), 1.0))
}

/// D3DRMCreateColorRGBA(red, green, blue, alpha).
fn create_color_rgba(_: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(color(
        float(args[0]),
        float(args[1]),
        float(args[2]),
        float(args[3]),
    ))
}

/// D3DRMMatrixFromQuaternion(m, q): the rotation of the unit quaternion
/// `q` as a matrix for row vectors. Returns nothing.
fn matrix_from_quaternion(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (w, [x, y, z]) = read_quaternion(cpu, args[1])?;
    #[rustfmt::skip]
    let m = [
        1.0 - 2.0 * (y * y + z * z), 2.0 * (x * y - z * w), 2.0 * (x * z + y * w), 0.0,
        2.0 * (x * y + z * w), 1.0 - 2.0 * (x * x + z * z), 2.0 * (y * z - x * w), 0.0,
        2.0 * (x * z - y * w), 2.0 * (y * z + x * w), 1.0 - 2.0 * (x * x + y * y), 0.0,
        0.0, 0.0, 0.0, 1.0,
    ];
    write_floats(cpu, args[0], &m);
    Some(0)
}

/// D3DRMQuaternionFromRotation(q, v, theta): the rotation by `theta`
/// about the axis `v`. Native D3DRM normalizes `v` where it is, as Wine's
/// tests show, and so does this.
fn quaternion_from_rotation(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [q, v, theta] = args[..] else {
        return Some(0);
    };
    let axis = normalize(read_floats::<3>(cpu, v)?);
    let half = float(theta) / 2.0;
    write_floats(cpu, v, &axis);
    write_quaternion(cpu, q, (half.cos(), scale(axis, half.sin())));
    Some(q)
}

/// D3DRMQuaternionMultiply(q, a, b): a times b.
fn quaternion_multiply(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let a = read_quaternion(cpu, args[1])?;
    let b = read_quaternion(cpu, args[2])?;
    write_quaternion(cpu, args[0], multiply(a, b));
    Some(args[0])
}

/// D3DRMQuaternionSlerp(q, a, b, alpha): the spherical interpolation from
/// `a` (alpha 0) to `b` (alpha 1), along the shorter arc.
fn quaternion_slerp(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (sa, va) = read_quaternion(cpu, args[1])?;
    let (sb, vb) = read_quaternion(cpu, args[2])?;
    let alpha = float(args[3]);
    let mut d = sa * sb + dot(va, vb);
    let mut epsilon = 1.0;
    if d < 0.0 {
        epsilon = -1.0;
        d = -d;
    }
    let (mut from, mut to) = (1.0 - alpha, alpha);
    if 1.0 - d > 0.001 {
        let theta = d.acos();
        from = (theta * from).sin() / theta.sin();
        to = (theta * alpha).sin() / theta.sin();
    }
    let s = from * sa + epsilon * to * sb;
    let v = add(scale(va, from), scale(vb, epsilon * to));
    write_quaternion(cpu, args[0], (s, v));
    Some(args[0])
}

/// The two vectors at `args[1]` and `args[2]`.
fn two_vectors(cpu: &mut Cpu, args: &[u32]) -> Option<(Vector, Vector)> {
    Some((read_floats(cpu, args[1])?, read_floats(cpu, args[2])?))
}

fn vector_add(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (a, b) = two_vectors(cpu, args)?;
    write_floats(cpu, args[0], &add(a, b));
    Some(args[0])
}

fn vector_subtract(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (a, b) = two_vectors(cpu, args)?;
    write_floats(cpu, args[0], &sub(a, b));
    Some(args[0])
}

fn vector_cross_product(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (a, b) = two_vectors(cpu, args)?;
    write_floats(cpu, args[0], &cross(a, b));
    Some(args[0])
}

/// D3DRMVectorDotProduct(a, b).
fn vector_dot_product(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let a = read_floats(cpu, args[0])?;
    let b = read_floats(cpu, args[1])?;
    ret_float(cpu, dot(a, b))
}

/// D3DRMVectorModulus(v): its length.
fn vector_modulus(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let v: Vector = read_floats(cpu, args[0])?;
    ret_float(cpu, dot(v, v).sqrt())
}

/// D3DRMVectorNormalize(v): `v` made length 1 where it is, or (1, 0, 0)
/// if it has none.
fn vector_normalize(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let v = read_floats(cpu, args[0])?;
    write_floats(cpu, args[0], &normalize(v));
    Some(args[0])
}

/// D3DRMVectorRandom(d): a random vector of length 1, from the C
/// runtime's rand(), which D3DRM.DLL has its own copy of: each process
/// starts the same series.
fn vector_random(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let seed_at = super::stubs::var_address(dll_index(), VAR_SEED);
    let mut seed = cpu.bus.guest_read_32(seed_at);
    if cpu.bus.guest_faulted() {
        return None;
    }
    let mut rand = || {
        seed = seed.wrapping_mul(214_013).wrapping_add(2_531_011);
        (seed >> 16 & 0x7FFF) as f32
    };
    let v = [rand(), rand(), rand()];
    cpu.bus.guest_write_32(seed_at, seed);
    write_floats(cpu, args[0], &normalize(v));
    Some(args[0])
}

/// D3DRMVectorReflect(r, ray, norm): `ray` reflected about the normal
/// `norm`, 2 (ray . norm) norm - ray.
fn vector_reflect(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (ray, norm) = two_vectors(cpu, args)?;
    write_floats(cpu, args[0], &sub(scale(norm, 2.0 * dot(ray, norm)), ray));
    Some(args[0])
}

/// D3DRMVectorRotate(r, v, axis, theta): `v` rotated by `theta` about
/// `axis`, made length 1 as native D3DRM makes it (Wine's tests). Like
/// D3DRMQuaternionFromRotation it normalizes `axis` where it is.
fn vector_rotate(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (r, v, axis_at, theta) = (args[0], args[1], args[2], float(args[3]));
    let v = read_floats(cpu, v)?;
    let axis = normalize(read_floats::<3>(cpu, axis_at)?);
    let (sin, cos) = (theta * 0.5).sin_cos();
    let q1 = (cos, scale(axis, sin));
    let q2 = (cos, scale(axis, -sin));
    let (_, rotated) = multiply(multiply(q1, (0.0, v)), q2);
    write_floats(cpu, axis_at, &axis);
    write_floats(cpu, r, &normalize(rotated));
    Some(r)
}

/// D3DRMVectorScale(d, s, factor).
fn vector_scale(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let v = read_floats(cpu, args[1])?;
    write_floats(cpu, args[0], &scale(v, float(args[2])));
    Some(args[0])
}
