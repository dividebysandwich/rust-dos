//! The built-in Windows' Direct3D 8 (D3D8.DLL): Win32 programs that make
//! a device with software vertex processing on the Voodoo3's HAL and put
//! vertices through geometry blending with ProcessVertices, as Max Payne
//! draws its characters: XYZB FVFs with D3DFVF_LASTBETA_UBYTE4, the
//! world matrices weighed by the vertex's weights, indexed or not. The
//! programs are made as in win32_d3d_tests.rs, whose helpers they use.
//! On a ToadForce a program makes a device with hardware vertex
//! processing and a depth buffer with a stencil, and draws with a
//! compressed texture and the stencil test.

use super::win32_d3d_tests::{
    Arg, Calls, DONE, RESULTS, copy, finish, machine, machine_toadforce, peek, peek_f32, program,
    put_f32s, put32, results, run_until_done,
};
use iced_x86::code_asm::*;

// The data's layout. DONE and RESULTS are the helpers'.
const HWND: u32 = 0x00;
const D3D: u32 = 0x04;
const DEVICE: u32 = 0x08;
const SOURCE: u32 = 0x0C;
const SOURCE1: u32 = 0x10;
const DEST: u32 = 0x14;
const LOCKED: u32 = 0x18;
const CLASS: u32 = 0x100;
const PRESENT: u32 = 0x120;
const VIEW: u32 = 0x160;
const LIGHT: u32 = 0x180;
const MATERIAL: u32 = 0x200;
/// The world matrices the program sets: 0, 1, 5 and 9.
const MATRICES: u32 = 0x300;
const VERTICES: u32 = 0x400;
const VERTICES1: u32 = 0x480;
/// The adapter's D3DCAPS8, and then the device's.
const CAPS: u32 = 0x600;
const DEVICE_CAPS: u32 = 0x700;
/// Where, among the results, the program keeps how many calls it made.
const CALLS: u32 = 0x3F0;
const DATA_SIZE: usize = 0x2000;

// Methods' places in their vtables.
const D3D8_GET_DEVICE_CAPS: u32 = 13;
const D3D8_CREATE_DEVICE: u32 = 15;
const DEV_GET_DEVICE_CAPS: u32 = 7;
const DEV_CREATE_VERTEX_BUFFER: u32 = 23;
const DEV_SET_TRANSFORM: u32 = 37;
const DEV_SET_VIEWPORT: u32 = 40;
const DEV_SET_MATERIAL: u32 = 42;
const DEV_SET_LIGHT: u32 = 44;
const DEV_LIGHT_ENABLE: u32 = 46;
const DEV_SET_RENDER_STATE: u32 = 50;
const DEV_PROCESS_VERTICES: u32 = 74;
const DEV_SET_VERTEX_SHADER: u32 = 76;
const DEV_SET_STREAM_SOURCE: u32 = 83;
const VB_LOCK: u32 = 11;
const VB_UNLOCK: u32 = 12;

const D3D_SDK_VERSION: u32 = 220;
const D3DDEVTYPE_HAL: u32 = 1;
const D3DCREATE_SOFTWARE_VERTEXPROCESSING: u32 = 0x20;
const D3DFMT_R5G6B5: u32 = 23;
const D3DSWAPEFFECT_DISCARD: u32 = 1;
const D3DPOOL_SYSTEMMEM: u32 = 2;
const WS_POPUP: u32 = 0x8000_0000;
const WS_VISIBLE: u32 = 0x1000_0000;
/// XYZB2 with LASTBETA_UBYTE4 and a normal (one weight and the
/// indices), and XYZB1 with LASTBETA_UBYTE4 and a normal (the indices
/// only): Max Payne's 0x1118 and 0x1116 without the texture.
const FVF_ONE_WEIGHT: u32 = 0x1018;
const FVF_INDICES_ONLY: u32 = 0x1016;
const FVF_SCREEN: u32 = 0x44;
const D3DTS_WORLD: u32 = 256;
const D3DRS_VERTEXBLEND: u32 = 151;
const D3DRS_INDEXEDVERTEXBLENDENABLE: u32 = 167;
const D3DVBF_1WEIGHTS: u32 = 1;
const D3DVBF_0WEIGHTS: u32 = 256;

const USER32: (&str, &[&str]) = ("USER32.DLL", &["CreateWindowExA"]);
const D3D8: (&str, &[&str]) = ("D3D8.DLL", &["Direct3DCreate8"]);
const CREATE_WINDOW: usize = 0;
const DIRECT3D_CREATE8: usize = 1;

/// A matrix that turns x into y (90 degrees about z) and moves by `t`.
fn turn(t: [f32; 3]) -> [f32; 16] {
    [
        0.0, 1.0, 0.0, 0.0, -1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, t[0], t[1], t[2], 1.0,
    ]
}

/// A matrix that moves by `t`.
fn moving(t: [f32; 3]) -> [f32; 16] {
    [
        1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, t[0], t[1], t[2], 1.0,
    ]
}

/// The world matrices the program sets, by their index.
fn matrices() -> [(u32, [f32; 16]); 4] {
    [
        (0, moving([0.25, 0.0, 0.0])),
        (1, moving([0.0, 0.25, 0.0])),
        (5, turn([0.0, 0.5, 0.0])),
        (9, moving([-0.5, 0.0, 0.0])),
    ]
}

/// The program: a full-screen device with software vertex processing,
/// a white directional light shining toward -x, a white material, the
/// identity for view and projection and a 64x64 viewport. Its two
/// vertices of one weight and indices go through ProcessVertices with
/// D3DVBF_1WEIGHTS indexed (into screen vertices 0 and 1) and not
/// (2 and 3), and two vertices of indices only with D3DVBF_0WEIGHTS
/// indexed (4 and 5).
fn blend_program() -> (Vec<u8>, u32) {
    let mut d = vec![0u8; DATA_SIZE];
    d[CLASS as usize..CLASS as usize + 7].copy_from_slice(b"Static\0");
    // D3DPRESENT_PARAMETERS: 640x480 R5G6B5 in full screen, one back
    // buffer; the window is set by the program.
    for (i, v) in [640, 480, D3DFMT_R5G6B5, 1, 0, D3DSWAPEFFECT_DISCARD]
        .iter()
        .enumerate()
    {
        put32(&mut d, PRESENT + 4 * i as u32, *v);
    }
    // D3DVIEWPORT8: 64x64, depths 0 to 1.
    put32(&mut d, VIEW + 8, 64);
    put32(&mut d, VIEW + 12, 64);
    put_f32s(&mut d, VIEW + 16, &[0.0, 1.0]);
    // D3DLIGHT8: directional, white, along -x.
    put32(&mut d, LIGHT, 3);
    put_f32s(&mut d, LIGHT + 4, &[1.0, 1.0, 1.0, 1.0]);
    put_f32s(&mut d, LIGHT + 64, &[-1.0, 0.0, 0.0]);
    // D3DMATERIAL8: a white diffuse color.
    put_f32s(&mut d, MATERIAL, &[1.0, 1.0, 1.0, 1.0]);
    for (k, (_, m)) in matrices().iter().enumerate() {
        put_f32s(&mut d, MATRICES + 64 * k as u32, m);
    }
    // The vertices: x, y, z, a weight, the indices, the normal.
    let mut vertex = |at: u32, weight: Option<f32>, indices: [u8; 4]| {
        put_f32s(&mut d, at, &[0.0, 0.0, 0.5]);
        let mut at = at + 12;
        if let Some(w) = weight {
            put_f32s(&mut d, at, &[w]);
            at += 4;
        }
        put32(&mut d, at, u32::from_le_bytes(indices));
        put_f32s(&mut d, at + 4, &[1.0, 0.0, 0.0]);
    };
    vertex(VERTICES, Some(0.25), [5, 9, 0, 0]);
    vertex(VERTICES + 32, Some(1.0), [9, 5, 0, 0]);
    vertex(VERTICES1, None, [5, 0, 0, 0]);
    vertex(VERTICES1 + 28, None, [200, 0, 0, 0]);
    program(&[USER32, D3D8], &d, |a, l| {
        let mut c = Calls { next: 0 };
        c.api(
            a,
            l,
            CREATE_WINDOW,
            &[
                Arg::I(0),
                Arg::At(CLASS),
                Arg::At(CLASS),
                Arg::I(WS_POPUP | WS_VISIBLE),
                Arg::I(0),
                Arg::I(0),
                Arg::I(640),
                Arg::I(480),
                Arg::I(0),
                Arg::I(0),
                Arg::I(0x40_0000),
                Arg::I(0),
            ],
        )?;
        a.mov(dword_ptr(l.at(HWND)), eax)?;
        a.mov(dword_ptr(l.at(PRESENT + 24)), eax)?;
        c.api(a, l, DIRECT3D_CREATE8, &[Arg::I(D3D_SDK_VERSION)])?;
        a.mov(dword_ptr(l.at(D3D)), eax)?;
        c.com(
            a,
            l,
            D3D,
            D3D8_GET_DEVICE_CAPS,
            &[Arg::I(0), Arg::I(D3DDEVTYPE_HAL), Arg::At(CAPS)],
        )?;
        c.com(
            a,
            l,
            D3D,
            D3D8_CREATE_DEVICE,
            &[
                Arg::I(0),
                Arg::I(D3DDEVTYPE_HAL),
                Arg::Val(HWND),
                Arg::I(D3DCREATE_SOFTWARE_VERTEXPROCESSING),
                Arg::At(PRESENT),
                Arg::At(DEVICE),
            ],
        )?;
        c.com(a, l, DEVICE, DEV_GET_DEVICE_CAPS, &[Arg::At(DEVICE_CAPS)])?;
        // The source buffers, and the screen vertices'.
        for (fvf, size, buffer, from) in [
            (FVF_ONE_WEIGHT, 64, SOURCE, VERTICES),
            (FVF_INDICES_ONLY, 56, SOURCE1, VERTICES1),
        ] {
            c.com(
                a,
                l,
                DEVICE,
                DEV_CREATE_VERTEX_BUFFER,
                &[
                    Arg::I(size),
                    Arg::I(0),
                    Arg::I(fvf),
                    Arg::I(D3DPOOL_SYSTEMMEM),
                    Arg::At(buffer),
                ],
            )?;
            c.com(
                a,
                l,
                buffer,
                VB_LOCK,
                &[Arg::I(0), Arg::I(0), Arg::At(LOCKED), Arg::I(0)],
            )?;
            copy(a, l, from, LOCKED, size)?;
            c.com(a, l, buffer, VB_UNLOCK, &[])?;
        }
        c.com(
            a,
            l,
            DEVICE,
            DEV_CREATE_VERTEX_BUFFER,
            &[
                Arg::I(6 * 20),
                Arg::I(0),
                Arg::I(FVF_SCREEN),
                Arg::I(D3DPOOL_SYSTEMMEM),
                Arg::At(DEST),
            ],
        )?;
        c.com(a, l, DEVICE, DEV_SET_VIEWPORT, &[Arg::At(VIEW)])?;
        c.com(a, l, DEVICE, DEV_SET_LIGHT, &[Arg::I(0), Arg::At(LIGHT)])?;
        c.com(a, l, DEVICE, DEV_LIGHT_ENABLE, &[Arg::I(0), Arg::I(1)])?;
        c.com(a, l, DEVICE, DEV_SET_MATERIAL, &[Arg::At(MATERIAL)])?;
        for (k, (index, _)) in matrices().iter().enumerate() {
            c.com(
                a,
                l,
                DEVICE,
                DEV_SET_TRANSFORM,
                &[
                    Arg::I(D3DTS_WORLD + index),
                    Arg::At(MATRICES + 64 * k as u32),
                ],
            )?;
        }
        let state = |a: &mut CodeAssembler, c: &mut Calls, s: u32, v: u32| {
            c.com(a, l, DEVICE, DEV_SET_RENDER_STATE, &[Arg::I(s), Arg::I(v)])
        };
        let process = |a: &mut CodeAssembler, c: &mut Calls, count: u32, to: u32| {
            c.com(
                a,
                l,
                DEVICE,
                DEV_PROCESS_VERTICES,
                &[
                    Arg::I(0),
                    Arg::I(to),
                    Arg::I(count),
                    Arg::Val(DEST),
                    Arg::I(0),
                ],
            )
        };
        let source = |a: &mut CodeAssembler, c: &mut Calls, fvf: u32, buffer: u32, stride| {
            c.com(a, l, DEVICE, DEV_SET_VERTEX_SHADER, &[Arg::I(fvf)])?;
            c.com(
                a,
                l,
                DEVICE,
                DEV_SET_STREAM_SOURCE,
                &[Arg::I(0), Arg::Val(buffer), Arg::I(stride)],
            )
        };
        source(a, &mut c, FVF_ONE_WEIGHT, SOURCE, 32)?;
        state(a, &mut c, D3DRS_VERTEXBLEND, D3DVBF_1WEIGHTS)?;
        state(a, &mut c, D3DRS_INDEXEDVERTEXBLENDENABLE, 1)?;
        process(a, &mut c, 2, 0)?;
        state(a, &mut c, D3DRS_INDEXEDVERTEXBLENDENABLE, 0)?;
        process(a, &mut c, 2, 2)?;
        source(a, &mut c, FVF_INDICES_ONLY, SOURCE1, 28)?;
        state(a, &mut c, D3DRS_VERTEXBLEND, D3DVBF_0WEIGHTS)?;
        state(a, &mut c, D3DRS_INDEXEDVERTEXBLENDENABLE, 1)?;
        process(a, &mut c, 2, 4)?;
        c.com(
            a,
            l,
            DEST,
            VB_LOCK,
            &[Arg::I(0), Arg::I(0), Arg::At(LOCKED), Arg::I(0)],
        )?;
        a.mov(dword_ptr(l.at(RESULTS + 4 * CALLS)), c.next as i32)?;
        finish(a, l)
    })
}

#[test]
fn geometry_blending_weighs_the_world_matrices_the_indices_name() {
    let (exe, data) = blend_program();
    let mut cpu = machine("d3d8_blend", exe);
    run_until_done(&mut cpu, data);
    assert_eq!(peek(&cpu, data + DONE), 1);
    let calls = peek(&cpu, data + RESULTS + 4 * CALLS);
    let r = results(&cpu, data, calls);
    // CreateWindowExA and Direct3DCreate8 give a handle and a pointer;
    // every other call succeeds, SetVertexShader with both FVFs among
    // them.
    assert!(r[2..].iter().all(|&hr| hr == 0), "results: {r:X?}");

    // MaxVertexBlendMatrices and MaxVertexBlendMatrixIndex: the HAL
    // blends nothing, the device's software vertex processing 4
    // matrices among 256. Only the device has D3DVTXPCAPS_TWEENING.
    let caps = |at: u32, word: u32| peek(&cpu, data + at + 4 * word);
    assert_eq!((caps(CAPS, 42), caps(CAPS, 43)), (0, 0));
    assert_eq!((caps(DEVICE_CAPS, 42), caps(DEVICE_CAPS, 43)), (4, 255));
    assert_eq!(caps(CAPS, 39) & 0x40, 0);
    assert_eq!(caps(DEVICE_CAPS, 39) & 0x40, 0x40);

    // The screen vertices: x, y, z, rhw and the diffuse color. The view
    // maps x and y from -1 to 1 onto 0 to 64, y down.
    let at = peek(&cpu, data + LOCKED);
    let vertex = |i: u32| {
        let v = at + 20 * i;
        (
            [0, 4, 8, 12].map(|o| peek_f32(&cpu, v + o)),
            peek(&cpu, v + 16),
        )
    };
    let expected = [
        // Matrix 5 takes 0.25 and matrix 9 the 0.75 left: (0, 0.5) and
        // (-0.5, 0) make (-0.375, 0.125). The normal (1, 0, 0) turns to
        // (0, 1) through matrix 5 and stays through matrix 9, which
        // makes (0.75, 0.25, 0): 0.75 of the light, 0xBF.
        ([20.0, 28.0, 0.5, 1.0], 0xFFBF_BFBF),
        // All of it matrix 9's: (-0.5, 0), lit fully.
        ([16.0, 32.0, 0.5, 1.0], 0xFFFF_FFFF),
        // Not indexed: matrices 0 and 1, 0.25 and 0.75: (0.0625,
        // 0.1875). Neither turns the normal.
        ([34.0, 26.0, 0.5, 1.0], 0xFFFF_FFFF),
        // The second vertex, all matrix 0's: (0.25, 0).
        ([40.0, 32.0, 0.5, 1.0], 0xFFFF_FFFF),
        // D3DVBF_0WEIGHTS: all matrix 5's, the first index: (0, 0.5),
        // the normal turned to (0, 1), away from the light.
        ([32.0, 16.0, 0.5, 1.0], 0xFF00_0000),
        // Matrix 200, which the program never set: the identity.
        ([32.0, 32.0, 0.5, 1.0], 0xFFFF_FFFF),
    ];
    for (i, (position, color)) in expected.iter().enumerate() {
        assert_eq!(vertex(i as u32), (*position, *color), "vertex {i}");
    }
}

// The ToadForce program's data, beside what the blend program has.
const TEXTURE: u32 = 0x1C;
const BACK: u32 = 0x20;
/// D3DLOCKED_RECTs: the texture's and the back buffer's.
const TEX_LOCKED: u32 = 0x28;
const BACK_LOCKED: u32 = 0x30;
/// The pixels the program reads from the back buffer.
const PIXELS: u32 = 0x40;
/// The texture's blocks and the two quads' vertices.
const BLOCKS: u32 = 0x800;
const QUAD: u32 = 0x840;
const QUAD1: u32 = 0x900;

const D3D8_CHECK_DEVICE_FORMAT: u32 = 10;
const DEV_GET_BACK_BUFFER: u32 = 16;
const DEV_CREATE_TEXTURE: u32 = 20;
const DEV_BEGIN_SCENE: u32 = 34;
const DEV_END_SCENE: u32 = 35;
const DEV_CLEAR: u32 = 36;
const DEV_SET_TEXTURE: u32 = 61;
const DEV_SET_TEXTURE_STAGE_STATE: u32 = 63;
const DEV_DRAW_PRIMITIVE_UP: u32 = 72;
const TEX_LOCK_RECT: u32 = 16;
const TEX_UNLOCK_RECT: u32 = 17;
const SURF_LOCK_RECT: u32 = 9;

const D3DCREATE_HARDWARE_VERTEXPROCESSING: u32 = 0x40;
const D3DFMT_A8R8G8B8: u32 = 21;
const D3DFMT_D24S8: u32 = 75;
const D3DFMT_DXT1: u32 = u32::from_le_bytes(*b"DXT1");
const D3DUSAGE_DEPTHSTENCIL: u32 = 2;
const D3DRTYPE_SURFACE: u32 = 1;
const D3DRTYPE_TEXTURE: u32 = 3;
const D3DPOOL_DEFAULT: u32 = 0;
const D3DPOOL_MANAGED: u32 = 1;
const D3DFMT_L8: u32 = 50;
const D3DFMT_V8U8: u32 = 60;
const D3DPRESENTFLAG_LOCKABLE_BACKBUFFER: u32 = 1;
const D3DPT_TRIANGLELIST: u32 = 4;
/// XYZRHW with one set of texture coordinates.
const FVF_TEXTURED: u32 = 0x104;
const D3DRS_STENCILENABLE: u32 = 52;
const D3DRS_STENCILPASS: u32 = 55;
const D3DRS_STENCILFUNC: u32 = 56;
const D3DRS_STENCILREF: u32 = 57;
const D3DRS_TEXTUREFACTOR: u32 = 60;
const D3DTSS_COLOROP: u32 = 1;
const D3DTSS_COLORARG1: u32 = 2;
const D3DTOP_SELECTARG1: u32 = 2;
const D3DTA_TFACTOR: u32 = 3;
const D3DSTENCILOP_REPLACE: u32 = 3;
const D3DCMP_EQUAL: u32 = 3;
const D3DCMP_ALWAYS: u32 = 8;

/// The pixels the program reads: x and y in the back buffer.
const READ: [(u32, u32); 4] = [(16, 16), (40, 16), (16, 48), (60, 60)];

/// The program: a full-screen device with hardware vertex processing
/// and a D24S8 depth buffer. It clears colour, depth and stencil, and
/// draws a quad of 48 by 32 pixels with an 8x8 DXT1 texture of one
/// green, which writes 1 to the stencil. A red quad of 32 by 64 pixels
/// then draws where the stencil is 1. Before the draws it also makes an
/// L8 and a V8U8 texture and a DXT1 one in D3DPOOL_DEFAULT. The pixels
/// of `READ` go to the data.
fn toadforce_program() -> (Vec<u8>, u32) {
    let mut d = vec![0u8; DATA_SIZE];
    d[CLASS as usize..CLASS as usize + 7].copy_from_slice(b"Static\0");
    // D3DPRESENT_PARAMETERS: 640x480 R5G6B5 in full screen, one back
    // buffer that can be locked, and a depth buffer with a stencil.
    for (i, v) in [640, 480, D3DFMT_R5G6B5, 1, 0, D3DSWAPEFFECT_DISCARD]
        .iter()
        .enumerate()
    {
        put32(&mut d, PRESENT + 4 * i as u32, *v);
    }
    put32(&mut d, PRESENT + 32, 1);
    put32(&mut d, PRESENT + 36, D3DFMT_D24S8);
    put32(&mut d, PRESENT + 40, D3DPRESENTFLAG_LOCKABLE_BACKBUFFER);
    // Four blocks whose two colours are green, every pixel the first.
    for block in 0..4 {
        put32(&mut d, BLOCKS + 8 * block, 0x07E0_07E0);
    }
    // A quad from the top left corner: two triangles, clockwise.
    let mut quad = |at: u32, w: f32, h: f32| {
        let corners = [[0.0, 0.0], [w, 0.0], [0.0, h], [w, 0.0], [w, h], [0.0, h]];
        for (i, [x, y]) in corners.into_iter().enumerate() {
            put_f32s(&mut d, at + 24 * i as u32, &[x, y, 0.5, 1.0, x / w, y / h]);
        }
    };
    quad(QUAD, 48.0, 32.0);
    quad(QUAD1, 32.0, 64.0);
    program(&[USER32, D3D8], &d, |a, l| {
        let mut c = Calls { next: 0 };
        c.api(
            a,
            l,
            CREATE_WINDOW,
            &[
                Arg::I(0),
                Arg::At(CLASS),
                Arg::At(CLASS),
                Arg::I(WS_POPUP | WS_VISIBLE),
                Arg::I(0),
                Arg::I(0),
                Arg::I(640),
                Arg::I(480),
                Arg::I(0),
                Arg::I(0),
                Arg::I(0x40_0000),
                Arg::I(0),
            ],
        )?;
        a.mov(dword_ptr(l.at(HWND)), eax)?;
        a.mov(dword_ptr(l.at(PRESENT + 24)), eax)?;
        c.api(a, l, DIRECT3D_CREATE8, &[Arg::I(D3D_SDK_VERSION)])?;
        a.mov(dword_ptr(l.at(D3D)), eax)?;
        c.com(
            a,
            l,
            D3D,
            D3D8_GET_DEVICE_CAPS,
            &[Arg::I(0), Arg::I(D3DDEVTYPE_HAL), Arg::At(CAPS)],
        )?;
        // The formats a Voodoo3 has none of: a compressed and a 32-bit
        // texture, and a depth buffer with a stencil.
        for (usage, kind, format) in [
            (0, D3DRTYPE_TEXTURE, D3DFMT_DXT1),
            (0, D3DRTYPE_TEXTURE, D3DFMT_A8R8G8B8),
            (D3DUSAGE_DEPTHSTENCIL, D3DRTYPE_SURFACE, D3DFMT_D24S8),
        ] {
            c.com(
                a,
                l,
                D3D,
                D3D8_CHECK_DEVICE_FORMAT,
                &[
                    Arg::I(0),
                    Arg::I(D3DDEVTYPE_HAL),
                    Arg::I(D3DFMT_R5G6B5),
                    Arg::I(usage),
                    Arg::I(kind),
                    Arg::I(format),
                ],
            )?;
        }
        c.com(
            a,
            l,
            D3D,
            D3D8_CREATE_DEVICE,
            &[
                Arg::I(0),
                Arg::I(D3DDEVTYPE_HAL),
                Arg::Val(HWND),
                Arg::I(D3DCREATE_HARDWARE_VERTEXPROCESSING),
                Arg::At(PRESENT),
                Arg::At(DEVICE),
            ],
        )?;
        c.com(a, l, DEVICE, DEV_GET_DEVICE_CAPS, &[Arg::At(DEVICE_CAPS)])?;
        // Blue, the farthest depth, and a stencil of 0.
        let clear = [
            Arg::I(0),
            Arg::I(0),
            Arg::I(7),
            Arg::I(0xFF00_0080),
            Arg::I(1.0f32.to_bits()),
            Arg::I(0),
        ];
        c.com(a, l, DEVICE, DEV_CLEAR, &clear)?;
        c.com(
            a,
            l,
            DEVICE,
            DEV_CREATE_TEXTURE,
            &[
                Arg::I(8),
                Arg::I(8),
                Arg::I(1),
                Arg::I(0),
                Arg::I(D3DFMT_DXT1),
                Arg::I(D3DPOOL_MANAGED),
                Arg::At(TEXTURE),
            ],
        )?;
        let lock = [Arg::I(0), Arg::At(TEX_LOCKED), Arg::I(0), Arg::I(0)];
        c.com(a, l, TEXTURE, TEX_LOCK_RECT, &lock)?;
        copy(a, l, BLOCKS, TEX_LOCKED + 4, 32)?;
        c.com(a, l, TEXTURE, TEX_UNLOCK_RECT, &[Arg::I(0)])?;
        // Textures of other formats the card samples: luminance, bump,
        // and a compressed one in D3DPOOL_DEFAULT.
        for (format, pool, out) in [
            (D3DFMT_L8, D3DPOOL_MANAGED, SOURCE),
            (D3DFMT_V8U8, D3DPOOL_MANAGED, SOURCE1),
            (D3DFMT_DXT1, D3DPOOL_DEFAULT, DEST),
        ] {
            let args = [
                Arg::I(8),
                Arg::I(8),
                Arg::I(1),
                Arg::I(0),
                Arg::I(format),
                Arg::I(pool),
                Arg::At(out),
            ];
            c.com(a, l, DEVICE, DEV_CREATE_TEXTURE, &args)?;
        }
        let state = |a: &mut CodeAssembler, c: &mut Calls, s: u32, v: u32| {
            c.com(a, l, DEVICE, DEV_SET_RENDER_STATE, &[Arg::I(s), Arg::I(v)])
        };
        let stage = |a: &mut CodeAssembler, c: &mut Calls, s: u32, v: u32| {
            let args = [Arg::I(0), Arg::I(s), Arg::I(v)];
            c.com(a, l, DEVICE, DEV_SET_TEXTURE_STAGE_STATE, &args)
        };
        let draw = |a: &mut CodeAssembler, c: &mut Calls, quad: u32| {
            let args = [
                Arg::I(D3DPT_TRIANGLELIST),
                Arg::I(2),
                Arg::At(quad),
                Arg::I(24),
            ];
            c.com(a, l, DEVICE, DEV_DRAW_PRIMITIVE_UP, &args)
        };
        c.com(a, l, DEVICE, DEV_SET_VERTEX_SHADER, &[Arg::I(FVF_TEXTURED)])?;
        c.com(a, l, DEVICE, DEV_BEGIN_SCENE, &[])?;
        // The textured quad, which marks the stencil.
        let texture = [Arg::I(0), Arg::Val(TEXTURE)];
        c.com(a, l, DEVICE, DEV_SET_TEXTURE, &texture)?;
        state(a, &mut c, D3DRS_STENCILENABLE, 1)?;
        state(a, &mut c, D3DRS_STENCILFUNC, D3DCMP_ALWAYS)?;
        state(a, &mut c, D3DRS_STENCILREF, 1)?;
        state(a, &mut c, D3DRS_STENCILPASS, D3DSTENCILOP_REPLACE)?;
        draw(a, &mut c, QUAD)?;
        // The red one, where the stencil is marked.
        c.com(a, l, DEVICE, DEV_SET_TEXTURE, &[Arg::I(0), Arg::I(0)])?;
        state(a, &mut c, D3DRS_TEXTUREFACTOR, 0xFFFF_0000)?;
        stage(a, &mut c, D3DTSS_COLOROP, D3DTOP_SELECTARG1)?;
        stage(a, &mut c, D3DTSS_COLORARG1, D3DTA_TFACTOR)?;
        state(a, &mut c, D3DRS_STENCILFUNC, D3DCMP_EQUAL)?;
        state(a, &mut c, D3DRS_STENCILPASS, 1)?;
        draw(a, &mut c, QUAD1)?;
        c.com(a, l, DEVICE, DEV_END_SCENE, &[])?;
        let back = [Arg::I(0), Arg::I(0), Arg::At(BACK)];
        c.com(a, l, DEVICE, DEV_GET_BACK_BUFFER, &back)?;
        let lock = [Arg::At(BACK_LOCKED), Arg::I(0), Arg::I(0)];
        c.com(a, l, BACK, SURF_LOCK_RECT, &lock)?;
        for (i, (x, y)) in READ.into_iter().enumerate() {
            a.mov(eax, dword_ptr(l.at(BACK_LOCKED)))?;
            a.imul_3(eax, eax, y as i32)?;
            a.add(eax, dword_ptr(l.at(BACK_LOCKED + 4)))?;
            a.movzx(eax, word_ptr(eax + 2 * x))?;
            a.mov(dword_ptr(l.at(PIXELS + 4 * i as u32)), eax)?;
        }
        a.mov(dword_ptr(l.at(RESULTS + 4 * CALLS)), c.next as i32)?;
        finish(a, l)
    })
}

#[test]
fn a_toadforce_has_a_device_with_a_stencil_and_compressed_textures() {
    let (exe, data) = toadforce_program();
    let mut cpu = machine_toadforce("d3d8_toadforce", exe);
    run_until_done(&mut cpu, data);
    assert_eq!(peek(&cpu, data + DONE), 1);
    let calls = peek(&cpu, data + RESULTS + 4 * CALLS);
    let r = results(&cpu, data, calls);
    // Every call after CreateWindowExA and Direct3DCreate8 succeeds:
    // the format checks, CreateDevice with hardware vertex processing,
    // the Clear of the stencil and the texture among them.
    assert!(r[2..].iter().all(|&hr| hr == 0), "results: {r:X?}");

    let caps = |at: u32, word: u32| peek(&cpu, data + at + 4 * word);
    // D3DDEVCAPS_HWTRANSFORMANDLIGHT, textures up to 2048, the eight
    // stencil operations, eight stages with four textures at once.
    assert_eq!(caps(CAPS, 7) & 0x1_0000, 0x1_0000);
    assert_eq!((caps(CAPS, 22), caps(CAPS, 23)), (2048, 2048));
    assert_eq!(caps(CAPS, 34), 0xFF);
    assert_eq!((caps(CAPS, 37), caps(CAPS, 38)), (8, 4));
    // D3DPMISCCAPS_: the color write mask, D3DTA_TEMP and the blend
    // operations.
    assert_eq!(caps(CAPS, 8) & 0xC80, 0xC80);
    // The card blends between four matrices and picks none by index.
    // Vertex shaders run on the processor, which a device with
    // hardware vertex processing alone doesn't have.
    for at in [CAPS, DEVICE_CAPS] {
        assert_eq!((caps(at, 42), caps(at, 43)), (4, 0));
        assert_eq!(caps(at, 49), 0);
    }

    // A row of the 8x8 DXT1 texture is two blocks.
    assert_eq!(peek(&cpu, data + TEX_LOCKED), 16);
    let pixel = |i: u32| peek(&cpu, data + PIXELS + 4 * i);
    // Red where both quads are, green where the textured one alone
    // is, and the blue of the Clear where the stencil kept the red one
    // out and where nothing drew.
    assert_eq!(
        [pixel(0), pixel(1), pixel(2), pixel(3)],
        [0xF800, 0x07E0, 0x0010, 0x0010]
    );
    let counts = cpu.bus.toadforce.as_ref().expect("a ToadForce").counts();
    assert_eq!(counts.rejected, 0);
    assert_eq!(counts.draws, 2);
}
