//! GLIDE2X.DLL's functions by their stdcall names, as 3dfx's library
//! exports them: each reads what the program passed and calls `api` or
//! `gu`. The few that drove hardware the emulator doesn't have (the
//! PCI functions, the movie recorder, the multipass utilities) exist,
//! log their calls and return 0.

use super::api::{self, LfbInfo, TexInfo, Vertex};
use super::file3df;
use super::gu::{self, MipMap};
use super::{hw, state};
use crate::cpu::Cpu;
use crate::win32::stubs::{Api, Conv, Handler};

const FALSE: u32 = 0;
const TRUE: u32 = 1;

const fn stdcall(name: &'static str, ordinal: u16, args: u8, handler: Handler) -> Api {
    Api {
        name,
        ordinal,
        args,
        conv: Conv::Stdcall,
        handler,
    }
}

fn f(v: u32) -> f32 {
    f32::from_bits(v)
}

fn b(v: u32) -> bool {
    v != 0
}

/// A function this library has only as a name: the log says it was
/// called.
fn missing(cpu: &mut Cpu, name: &str) -> Option<u32> {
    if cpu.bus.logs() {
        cpu.bus
            .log_string(&format!("[W32] GLIDE2X: {} is not implemented", name));
    }
    Some(0)
}

macro_rules! missing {
    ($($handler:ident $name:literal),* $(,)?) => {
        $(fn $handler(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
            missing(cpu, $name)
        })*
    };
}

missing!(
    missing_convert_and_download_rle "ConvertAndDownloadRle",
    missing_gr_splash "grSplash",
    missing_gr_sst_config_pipeline "grSstConfigPipeline",
    missing_gr_sst_vid_mode "grSstVidMode",
    missing_gu_encode_rle16 "guEncodeRLE16",
    missing_gu_fb_read_region "guFbReadRegion",
    missing_gu_fb_write_region "guFbWriteRegion",
    missing_gu_mp_draw_triangle "guMPDrawTriangle",
    missing_gu_mp_init "guMPInit",
    missing_gu_mp_tex_combine_function "guMPTexCombineFunction",
    missing_gu_mp_tex_source "guMPTexSource",
    missing_gu_movie_set_name "guMovieSetName",
    missing_gu_movie_start "guMovieStart",
    missing_gu_movie_stop "guMovieStop",
    missing_gu_tex_create_color_mip_map "guTexCreateColorMipMap",
    missing_gu_tex_get_mip_map_info "guTexGetMipMapInfo",
    missing_pci_close "pciClose",
    missing_pci_device_exists "pciDeviceExists",
    missing_pci_find_card "pciFindCard",
    missing_pci_find_card_multi "pciFindCardMulti",
    missing_pci_find_free_mtrr "pciFindFreeMTRR",
    missing_pci_find_mtrr_match "pciFindMTRRMatch",
    missing_pci_get_config_data "pciGetConfigData",
    missing_pci_get_error_code "pciGetErrorCode",
    missing_pci_get_error_string "pciGetErrorString",
    missing_pci_map_card "pciMapCard",
    missing_pci_map_card_multi "pciMapCardMulti",
    missing_pci_map_physical_to_linear "pciMapPhysicalToLinear",
    missing_pci_open "pciOpen",
    missing_pci_output_debug_string "pciOutputDebugString",
    missing_pci_set_config_data "pciSetConfigData",
    missing_pci_set_mtrr "pciSetMTRR",
    missing_pci_set_pass_through_base "pciSetPassThroughBase",
    missing_pci_unmap_physical "pciUnmapPhysical",
);

// --- Reading what the program passes ---

/// The vertex (GrVertex) at `at`, or None if a page isn't there.
fn vertex(cpu: &mut Cpu, at: u32) -> Option<Vertex> {
    let mut bytes = [0; Vertex::SIZE];
    cpu.bus.guest_read_bytes(at, &mut bytes);
    (!cpu.bus.guest_faulted()).then(|| Vertex::from_bytes(&bytes))
}

/// The `n` vertices of an array at `at`.
fn vertices(cpu: &mut Cpu, n: u32, at: u32) -> Option<Vec<Vertex>> {
    // More than a polygon has; a bad count draws nothing.
    if n > 1024 {
        return Some(Vec::new());
    }
    (0..n)
        .map(|i| vertex(cpu, at.wrapping_add(i * Vertex::SIZE as u32)))
        .collect()
}

/// The texture (GrTexInfo) at `at` and where its texels are.
fn tex_info(cpu: &mut Cpu, at: u32) -> Option<(TexInfo, u32)> {
    let mut w = [0u32; 5];
    for (i, v) in w.iter_mut().enumerate() {
        *v = cpu.bus.guest_read_32(at.wrapping_add(4 * i as u32));
    }
    let info = TexInfo {
        small_lod: w[0],
        large_lod: w[1],
        aspect: w[2],
        format: w[3],
    };
    (!cpu.bus.guest_faulted()).then_some((info, w[4]))
}

/// `len` bytes at `at`.
fn bytes(cpu: &mut Cpu, at: u32, len: usize) -> Option<Vec<u8>> {
    let mut data = vec![0; len];
    cpu.bus.guest_read_bytes(at, &mut data);
    (!cpu.bus.guest_faulted()).then_some(data)
}

/// The bytes of a mipmap level's texels as the program passes them.
fn level_len(aspect: u32, lod: u32, format: u32) -> usize {
    let (w, h) = api::level_size(aspect.min(6), lod.min(8));
    (w * h * if format >= api::TEXFMT_16BIT { 2 } else { 1 }) as usize
}

/// The bytes of all a texture's levels.
fn mipmap_len(info: &TexInfo) -> usize {
    if info.large_lod > info.small_lod || info.small_lod > 8 {
        return 0;
    }
    (info.large_lod..=info.small_lod)
        .map(|lod| level_len(info.aspect, lod, info.format))
        .sum()
}

// --- Initialisation and the board ---

fn gr_glide_init(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(0)
}

/// grGlideShutdown: the screen back, if this process has the card.
fn gr_glide_shutdown(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    let pid = crate::win32::user32::current_pid(cpu);
    if state(cpu).owner == pid {
        api::close(cpu);
    }
    Some(0)
}

fn gr_glide_get_version(cpu: &mut Cpu, a: &[u32]) -> Option<u32> {
    let version = match api::card(cpu) {
        Some(hw::Card::Voodoo1) | None => "Glide Version 2.43",
        _ => "Glide Version 2.53",
    };
    let mut text = version.as_bytes().to_vec();
    text.push(0);
    cpu.bus.guest_write_bytes(a[0], &text);
    Some(0)
}

/// GrHwConfiguration's bytes: the number of boards, then four boards'
/// type and configuration.
const HWCONFIG_SIZE: usize = 4 + 4 * 36;

fn hwconfig(cpu: &mut Cpu) -> Option<[u8; HWCONFIG_SIZE]> {
    let board = api::query(cpu)?;
    let mut out = [0u8; HWCONFIG_SIZE];
    // Every board shows as a Voodoo Graphics (type 0), as their Glide 2
    // did, so that programs which know only that one run. A Voodoo2's
    // FBI revision is raised by 80h with one TMU and 100h with two, and
    // FX_GLIDE_REPORT_REAL_HW makes it say Voodoo2 (type 3). A Banshee's
    // or Voodoo3's revisions are 1002h, and its memory shows as 2 MB per
    // TMU, the rest of it as frame buffer.
    let mb = |bytes: u32| bytes >> 20;
    let (mut fb_mb, mut tmu_mb) = (mb(board.fb_bytes), mb(board.tmu_bytes));
    if board.card == hw::Card::Banshee {
        tmu_mb = 2;
        fb_mb -= 2 * board.tmus;
    }
    let (kind, fbi_rev, tmu_rev) = match board.card {
        hw::Card::Voodoo1 => (0, 2, 1),
        hw::Card::Voodoo2 => {
            let real =
                crate::win32::kernel32::has_environment_variable(cpu, "FX_GLIDE_REPORT_REAL_HW");
            let fbi_rev = 4 + if board.tmus > 1 { 0x100 } else { 0x80 };
            (if real { 3 } else { 0 }, fbi_rev, 2)
        }
        _ => (0, 0x1002, 0x1002),
    };
    let words = [
        1,
        kind,
        fb_mb,
        fbi_rev,
        board.tmus,
        FALSE,
        tmu_rev,
        tmu_mb,
        if board.tmus > 1 { tmu_rev } else { 0 },
        if board.tmus > 1 { tmu_mb } else { 0 },
    ];
    for (i, w) in words.iter().enumerate() {
        out[4 * i..4 * i + 4].copy_from_slice(&w.to_le_bytes());
    }
    Some(out)
}

/// grSstQueryHardware: the board, or FALSE without a 3dfx card.
fn gr_sst_query_hardware(cpu: &mut Cpu, a: &[u32]) -> Option<u32> {
    let Some(config) = hwconfig(cpu) else {
        cpu.bus.guest_write_32(a[0], 0);
        return Some(FALSE);
    };
    cpu.bus.guest_write_bytes(a[0], &config);
    Some(TRUE)
}

/// grSstQueryBoards: how many boards there are.
fn gr_sst_query_boards(cpu: &mut Cpu, a: &[u32]) -> Option<u32> {
    let n = api::query(cpu).is_some() as u32;
    cpu.bus.guest_write_32(a[0], n);
    Some(TRUE)
}

fn gr_sst_select(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(0)
}

/// grSstWinOpen: resolutions by number only (GR_RESOLUTION_NONE, a
/// window's size, is for the Voodoo Rush).
fn gr_sst_win_open(cpu: &mut Cpu, a: &[u32]) -> Option<u32> {
    let Some((w, h)) = api::resolution_size(a[1]) else {
        return Some(FALSE);
    };
    let ok = api::open(cpu, w, h, api::refresh_hz(a[2]), a[3], a[4], a[5], a[6]);
    Some(ok as u32)
}

fn gr_sst_win_close(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    let pid = crate::win32::user32::current_pid(cpu);
    if state(cpu).owner == pid {
        api::close(cpu);
    }
    Some(0)
}

fn gr_sst_screen_width(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(api::screen_size(cpu).0)
}

fn gr_sst_screen_height(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(api::screen_size(cpu).1)
}

fn gr_sst_control(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(TRUE)
}

fn gr_sst_idle(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(0)
}

fn gr_sst_is_busy(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(api::sst_is_busy(cpu) as u32)
}

fn gr_sst_status(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(api::sst_status(cpu))
}

fn gr_sst_v_retrace_on(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(api::sst_vretrace_on(cpu) as u32)
}

fn gr_sst_video_line(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(api::sst_video_line(cpu))
}

fn gr_sst_origin(cpu: &mut Cpu, a: &[u32]) -> Option<u32> {
    api::sst_origin(cpu, a[0]);
    Some(0)
}

/// grSstPerfStats: the card's pixel counters (GrSstPerfStats_t).
fn gr_sst_perf_stats(cpu: &mut Cpu, a: &[u32]) -> Option<u32> {
    use crate::voodoo::regs::*;
    if cpu.bus.voodoo.is_none() {
        return Some(0);
    }
    let regs = [
        FBI_PIXELS_IN,
        FBI_CHROMA_FAIL,
        FBI_ZFUNC_FAIL,
        FBI_AFUNC_FAIL,
        FBI_PIXELS_OUT,
    ];
    for (i, reg) in regs.into_iter().enumerate() {
        let v = hw::read(cpu, reg) & 0xFF_FFFF;
        cpu.bus.guest_write_32(a[0].wrapping_add(4 * i as u32), v);
    }
    Some(0)
}

fn gr_sst_reset_perf_stats(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    if cpu.bus.voodoo.is_some() {
        hw::write(cpu, hw::ALL, crate::voodoo::regs::NOP_CMD, 1);
    }
    Some(0)
}

fn gr_tri_stats(cpu: &mut Cpu, a: &[u32]) -> Option<u32> {
    let [drawn, processed] = state(cpu).gc.triangles;
    if !cpu.bus.guest_probe(a[0], 4, true) || !cpu.bus.guest_probe(a[1], 4, true) {
        return None;
    }
    cpu.bus.guest_write_32(a[0], processed);
    cpu.bus.guest_write_32(a[1], drawn);
    Some(0)
}

fn gr_reset_tri_stats(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    state(cpu).gc.triangles = [0; 2];
    Some(0)
}

fn gr_check_for_room(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(0)
}

fn gr_glide_shameless_plug(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(0)
}

fn gr_error_set_callback(cpu: &mut Cpu, a: &[u32]) -> Option<u32> {
    state(cpu).gc.error_callback = a[0];
    Some(0)
}

fn gr_hints(cpu: &mut Cpu, a: &[u32]) -> Option<u32> {
    api::hints(cpu, a[0], a[1]);
    Some(0)
}

fn gr_glide_get_state(cpu: &mut Cpu, a: &[u32]) -> Option<u32> {
    let s = api::get_state(cpu);
    cpu.bus.guest_write_bytes(a[0], &s);
    Some(0)
}

fn gr_glide_set_state(cpu: &mut Cpu, a: &[u32]) -> Option<u32> {
    let data = bytes(cpu, a[0], api::STATE_SIZE)?;
    let mut s = [0; api::STATE_SIZE];
    s.copy_from_slice(&data);
    api::set_state(cpu, &s);
    Some(0)
}

// --- State ---

fn gr_alpha_blend_function(cpu: &mut Cpu, a: &[u32]) -> Option<u32> {
    api::alpha_blend_function(cpu, a[0], a[1], a[2], a[3]);
    Some(0)
}

fn gr_alpha_combine(cpu: &mut Cpu, a: &[u32]) -> Option<u32> {
    api::alpha_combine(cpu, a[0], a[1], a[2], a[3], b(a[4]));
    Some(0)
}

fn gr_alpha_controls_itrgb_lighting(cpu: &mut Cpu, a: &[u32]) -> Option<u32> {
    api::alpha_controls_itrgb_lighting(cpu, b(a[0]));
    Some(0)
}

fn gr_alpha_test_function(cpu: &mut Cpu, a: &[u32]) -> Option<u32> {
    api::alpha_test_function(cpu, a[0]);
    Some(0)
}

fn gr_alpha_test_reference_value(cpu: &mut Cpu, a: &[u32]) -> Option<u32> {
    api::alpha_test_reference_value(cpu, a[0] & 0xFF);
    Some(0)
}

fn gr_chromakey_mode(cpu: &mut Cpu, a: &[u32]) -> Option<u32> {
    api::chromakey_mode(cpu, a[0]);
    Some(0)
}

fn gr_chromakey_value(cpu: &mut Cpu, a: &[u32]) -> Option<u32> {
    api::chromakey_value(cpu, a[0]);
    Some(0)
}

fn gr_clip_window(cpu: &mut Cpu, a: &[u32]) -> Option<u32> {
    api::clip_window(cpu, a[0], a[1], a[2], a[3]);
    Some(0)
}

fn gr_color_combine(cpu: &mut Cpu, a: &[u32]) -> Option<u32> {
    api::color_combine(cpu, a[0], a[1], a[2], a[3], b(a[4]));
    Some(0)
}

fn gr_color_mask(cpu: &mut Cpu, a: &[u32]) -> Option<u32> {
    api::color_mask(cpu, b(a[0]), b(a[1]));
    Some(0)
}

fn gr_constant_color_value(cpu: &mut Cpu, a: &[u32]) -> Option<u32> {
    api::constant_color_value(cpu, a[0]);
    Some(0)
}

fn gr_constant_color_value4(cpu: &mut Cpu, a: &[u32]) -> Option<u32> {
    api::constant_color_value4(cpu, f(a[0]), f(a[1]), f(a[2]), f(a[3]));
    Some(0)
}

fn gr_cull_mode(cpu: &mut Cpu, a: &[u32]) -> Option<u32> {
    api::cull_mode(cpu, a[0]);
    Some(0)
}

fn gr_depth_bias_level(cpu: &mut Cpu, a: &[u32]) -> Option<u32> {
    api::depth_bias_level(cpu, a[0] as u16 as i16);
    Some(0)
}

fn gr_depth_buffer_function(cpu: &mut Cpu, a: &[u32]) -> Option<u32> {
    api::depth_buffer_function(cpu, a[0]);
    Some(0)
}

fn gr_depth_buffer_mode(cpu: &mut Cpu, a: &[u32]) -> Option<u32> {
    api::depth_buffer_mode(cpu, a[0]);
    Some(0)
}

fn gr_depth_mask(cpu: &mut Cpu, a: &[u32]) -> Option<u32> {
    api::depth_mask(cpu, b(a[0]));
    Some(0)
}

fn gr_disable_all_effects(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    api::disable_all_effects(cpu);
    Some(0)
}

fn gr_dither_mode(cpu: &mut Cpu, a: &[u32]) -> Option<u32> {
    api::dither_mode(cpu, a[0]);
    Some(0)
}

fn gr_fog_color_value(cpu: &mut Cpu, a: &[u32]) -> Option<u32> {
    api::fog_color_value(cpu, a[0]);
    Some(0)
}

fn gr_fog_mode(cpu: &mut Cpu, a: &[u32]) -> Option<u32> {
    // Glide 2's sources of fog end at iterated Z: a higher one, such as
    // the HAL's own FOG_WITH_ITERATED_W, turns fog off as in Glide.
    let mut mode = a[0];
    if mode & 0xFF > api::FOG_WITH_ITERATED_Z {
        mode &= !0xFF;
    }
    api::fog_mode(cpu, mode);
    Some(0)
}

fn gr_fog_table(cpu: &mut Cpu, a: &[u32]) -> Option<u32> {
    let data = bytes(cpu, a[0], api::FOG_TABLE_SIZE)?;
    let mut table = [0; api::FOG_TABLE_SIZE];
    table.copy_from_slice(&data);
    api::fog_table(cpu, &table);
    Some(0)
}

fn gr_gamma_correction_value(cpu: &mut Cpu, a: &[u32]) -> Option<u32> {
    api::gamma_correction_value(cpu, f(a[0]));
    Some(0)
}

fn gr_render_buffer(cpu: &mut Cpu, a: &[u32]) -> Option<u32> {
    api::render_buffer(cpu, a[0]);
    Some(0)
}

// --- Drawing ---

fn gr_draw_triangle(cpu: &mut Cpu, a: &[u32]) -> Option<u32> {
    let (p, q, r) = (vertex(cpu, a[0])?, vertex(cpu, a[1])?, vertex(cpu, a[2])?);
    api::draw_triangle(cpu, &p, &q, &r);
    Some(0)
}

fn gr_draw_line(cpu: &mut Cpu, a: &[u32]) -> Option<u32> {
    let (p, q) = (vertex(cpu, a[0])?, vertex(cpu, a[1])?);
    api::draw_line(cpu, &p, &q);
    Some(0)
}

fn gr_draw_point(cpu: &mut Cpu, a: &[u32]) -> Option<u32> {
    let p = vertex(cpu, a[0])?;
    api::draw_point(cpu, &p);
    Some(0)
}

/// grDrawPolygon and grDrawPlanarPolygon: vertices by their indices.
fn gr_draw_polygon(cpu: &mut Cpu, a: &[u32]) -> Option<u32> {
    let n = a[0].min(1024);
    let mut list = Vec::with_capacity(n as usize);
    for i in 0..n {
        let index = cpu.bus.guest_read_32(a[1].wrapping_add(4 * i));
        if cpu.bus.guest_faulted() {
            return None;
        }
        list.push(vertex(
            cpu,
            a[2].wrapping_add(index.wrapping_mul(Vertex::SIZE as u32)),
        )?);
    }
    api::draw_polygon(cpu, &list);
    Some(0)
}

/// grDrawPolygonVertexList and grDrawPlanarPolygonVertexList.
fn gr_draw_polygon_vertex_list(cpu: &mut Cpu, a: &[u32]) -> Option<u32> {
    let list = vertices(cpu, a[0], a[1])?;
    api::draw_polygon(cpu, &list);
    Some(0)
}

/// grAADrawTriangle: drawn without its antialiased edges.
fn gr_aa_draw_triangle(cpu: &mut Cpu, a: &[u32]) -> Option<u32> {
    gr_draw_triangle(cpu, a)
}

// --- Buffers ---

fn gr_buffer_clear(cpu: &mut Cpu, a: &[u32]) -> Option<u32> {
    api::buffer_clear(cpu, a[0], a[1] & 0xFF, a[2] & 0xFFFF);
    Some(0)
}

fn gr_buffer_swap(cpu: &mut Cpu, a: &[u32]) -> Option<u32> {
    api::buffer_swap(cpu, a[0]);
    Some(0)
}

fn gr_buffer_num_pending(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(api::buffer_num_pending(cpu))
}

// --- The linear frame buffer ---

fn gr_lfb_lock(cpu: &mut Cpu, a: &[u32]) -> Option<u32> {
    if !cpu.bus.guest_probe(a[5], 20, true) {
        return None;
    }
    let Some(LfbInfo {
        ptr,
        stride,
        write_mode,
        origin,
    }) = api::lfb_lock(cpu, a[0], a[1], a[2], a[3], b(a[4]))
    else {
        return Some(FALSE);
    };
    for (i, v) in [ptr, stride, write_mode, origin].into_iter().enumerate() {
        cpu.bus
            .guest_write_32(a[5].wrapping_add(4 + 4 * i as u32), v);
    }
    Some(TRUE)
}

fn gr_lfb_unlock(cpu: &mut Cpu, a: &[u32]) -> Option<u32> {
    Some(api::lfb_unlock(cpu, a[0], a[1]) as u32)
}

fn gr_lfb_constant_alpha(cpu: &mut Cpu, a: &[u32]) -> Option<u32> {
    api::lfb_constant_alpha(cpu, a[0]);
    Some(0)
}

fn gr_lfb_constant_depth(cpu: &mut Cpu, a: &[u32]) -> Option<u32> {
    api::lfb_constant_depth(cpu, a[0]);
    Some(0)
}

fn gr_lfb_write_color_format(cpu: &mut Cpu, a: &[u32]) -> Option<u32> {
    api::lfb_write_color_format(cpu, a[0]);
    Some(0)
}

fn gr_lfb_write_color_swizzle(cpu: &mut Cpu, a: &[u32]) -> Option<u32> {
    api::lfb_write_color_swizzle(cpu, b(a[0]), b(a[1]));
    Some(0)
}

/// grLfbReadRegion: 16-bit pixels into rows `dst_stride` apart.
fn gr_lfb_read_region(cpu: &mut Cpu, a: &[u32]) -> Option<u32> {
    let [buffer, x, y, w, h, stride, dst] = a[..7].try_into().ok()?;
    if w == 0 || h == 0 || w > 2048 || h > 2048 || x > 2048 || y > 2048 {
        return Some(FALSE);
    }
    let len = stride as usize * (h as usize - 1) + 2 * w as usize;
    if !cpu.bus.guest_probe(dst, len, true) {
        return None;
    }
    let Some(pixels) = api::lfb_read_region(cpu, buffer, x, y, w, h) else {
        return Some(FALSE);
    };
    for row in 0..h {
        let line: Vec<u8> = pixels[(row * w) as usize..((row + 1) * w) as usize]
            .iter()
            .flat_map(|p| p.to_le_bytes())
            .collect();
        cpu.bus
            .guest_write_bytes(dst.wrapping_add(row.wrapping_mul(stride)), &line);
    }
    Some(TRUE)
}

/// grLfbWriteRegion: pixels from rows `src_stride` apart, a signed
/// stride (an image stored bottom up has a negative one).
fn gr_lfb_write_region(cpu: &mut Cpu, a: &[u32]) -> Option<u32> {
    let [buffer, x, y, format, w, h, stride, src] = a[..8].try_into().ok()?;
    if w == 0 || h == 0 || w > 2048 || h > 2048 || x > 2048 || y > 2048 {
        return Some(FALSE);
    }
    let bpp = if matches!(format, 4 | 5 | 0xC..=0xE) {
        4
    } else {
        2
    };
    let row_len = bpp * w;
    let mut data = Vec::with_capacity((row_len * h) as usize);
    for row in 0..h {
        let at = src.wrapping_add(row.wrapping_mul(stride));
        data.extend(bytes(cpu, at, row_len as usize)?);
    }
    Some(api::lfb_write_region(cpu, buffer, x, y, format, w, h, row_len, &data) as u32)
}

// --- Textures ---

fn gr_tex_calc_mem_required(cpu: &mut Cpu, a: &[u32]) -> Option<u32> {
    let info = TexInfo {
        small_lod: a[0],
        large_lod: a[1],
        aspect: a[2],
        format: a[3],
    };
    Some(api::tex_texture_mem_required(
        cpu,
        api::MIPMAPLEVELMASK_BOTH,
        &info,
    ))
}

fn gr_tex_texture_mem_required(cpu: &mut Cpu, a: &[u32]) -> Option<u32> {
    let (info, _) = tex_info(cpu, a[1])?;
    Some(api::tex_texture_mem_required(cpu, a[0], &info))
}

fn gr_tex_min_address(cpu: &mut Cpu, a: &[u32]) -> Option<u32> {
    Some(api::tex_min_address(cpu, a[0]))
}

fn gr_tex_max_address(cpu: &mut Cpu, a: &[u32]) -> Option<u32> {
    Some(api::tex_max_address(cpu, a[0]))
}

fn gr_tex_source(cpu: &mut Cpu, a: &[u32]) -> Option<u32> {
    let (info, _) = tex_info(cpu, a[3])?;
    api::tex_source(cpu, a[0], a[1], a[2], &info);
    Some(0)
}

fn gr_tex_multibase(cpu: &mut Cpu, a: &[u32]) -> Option<u32> {
    api::tex_multibase(cpu, a[0], b(a[1]));
    Some(0)
}

fn gr_tex_multibase_address(cpu: &mut Cpu, a: &[u32]) -> Option<u32> {
    let (info, _) = tex_info(cpu, a[4])?;
    api::tex_multibase_address(cpu, a[0], a[1], a[2], a[3], &info);
    Some(0)
}

fn gr_tex_clamp_mode(cpu: &mut Cpu, a: &[u32]) -> Option<u32> {
    api::tex_clamp_mode(cpu, a[0], a[1], a[2]);
    Some(0)
}

fn gr_tex_combine(cpu: &mut Cpu, a: &[u32]) -> Option<u32> {
    api::tex_combine(cpu, a[0], a[1], a[2], a[3], a[4], b(a[5]), b(a[6]));
    Some(0)
}

fn gr_tex_combine_function(cpu: &mut Cpu, a: &[u32]) -> Option<u32> {
    gu::tex_combine_function(cpu, a[0], a[1]);
    Some(0)
}

fn gr_tex_detail_control(cpu: &mut Cpu, a: &[u32]) -> Option<u32> {
    api::tex_detail_control(cpu, a[0], a[1] as i32, a[2] & 0xFF, f(a[3]));
    Some(0)
}

fn gr_tex_filter_mode(cpu: &mut Cpu, a: &[u32]) -> Option<u32> {
    api::tex_filter_mode(cpu, a[0], a[1], a[2]);
    Some(0)
}

fn gr_tex_lod_bias_value(cpu: &mut Cpu, a: &[u32]) -> Option<u32> {
    api::tex_lod_bias_value(cpu, a[0], f(a[1]));
    Some(0)
}

fn gr_tex_mip_map_mode(cpu: &mut Cpu, a: &[u32]) -> Option<u32> {
    api::tex_mipmap_mode(cpu, a[0], a[1], b(a[2]));
    Some(0)
}

fn gr_tex_ncc_table(cpu: &mut Cpu, a: &[u32]) -> Option<u32> {
    api::tex_ncc_table(cpu, a[0], a[1]);
    Some(0)
}

/// The packed registers of a GuNccTable at `data`, which follow its Y,
/// I and Q.
fn ncc_table(cpu: &mut Cpu, data: u32) -> Option<[u32; 12]> {
    let raw = bytes(cpu, data.wrapping_add(64), 48)?;
    Some(std::array::from_fn(|i| {
        u32::from_le_bytes([raw[4 * i], raw[4 * i + 1], raw[4 * i + 2], raw[4 * i + 3]])
    }))
}

/// An NCC table (GuNccTable) or a palette (GuTexPalette) for texture
/// unit `tmu`, entries `start` to `end` of a palette.
fn download_table(
    cpu: &mut Cpu,
    tmu: u32,
    kind: u32,
    data: u32,
    start: u32,
    end: u32,
) -> Option<u32> {
    match kind {
        api::TEXTABLE_NCC0 | api::TEXTABLE_NCC1 => {
            let packed = ncc_table(cpu, data)?;
            api::tex_download_ncc_table(cpu, tmu, kind, &packed);
        }
        api::TEXTABLE_PALETTE => {
            let raw = bytes(cpu, data, 1024)?;
            let palette: [u32; 256] = std::array::from_fn(|i| {
                u32::from_le_bytes([raw[4 * i], raw[4 * i + 1], raw[4 * i + 2], raw[4 * i + 3]])
            });
            api::tex_download_palette(cpu, tmu, &palette, start, end);
        }
        _ => {}
    }
    Some(0)
}

fn gr_tex_download_table(cpu: &mut Cpu, a: &[u32]) -> Option<u32> {
    download_table(cpu, a[0], a[1], a[2], 0, 255)
}

fn gr_tex_download_table_partial(cpu: &mut Cpu, a: &[u32]) -> Option<u32> {
    download_table(cpu, a[0], a[1], a[2], a[3], a[4])
}

fn gr_tex_download_mip_map(cpu: &mut Cpu, a: &[u32]) -> Option<u32> {
    let (info, data) = tex_info(cpu, a[3])?;
    let texels = bytes(cpu, data, mipmap_len(&info))?;
    api::tex_download_mipmap(cpu, a[0], a[1], a[2], &info, &texels);
    Some(0)
}

/// Rows `t0` to `t1` of a level, from `data`.
#[allow(clippy::too_many_arguments)]
fn download_level(cpu: &mut Cpu, a: &[u32], t0: u32, t1: u32) -> Option<u32> {
    let [tmu, start, lod, large, aspect, format, even_odd, data] = a[..8].try_into().ok()?;
    if lod > 8 || aspect > 6 {
        return Some(0);
    }
    let (w, h) = api::level_size(aspect, lod);
    let t1 = t1.min(h - 1);
    if t0 > t1 {
        return Some(0);
    }
    let row = (w * if format >= api::TEXFMT_16BIT { 2 } else { 1 }) as usize;
    let texels = bytes(cpu, data, row * (t1 - t0 + 1) as usize)?;
    api::tex_download_level(
        cpu, tmu, start, lod, large, aspect, format, even_odd, &texels, t0, t1,
    );
    Some(0)
}

fn gr_tex_download_mip_map_level(cpu: &mut Cpu, a: &[u32]) -> Option<u32> {
    download_level(cpu, a, 0, 255)
}

fn gr_tex_download_mip_map_level_partial(cpu: &mut Cpu, a: &[u32]) -> Option<u32> {
    download_level(cpu, a, a[8], a[9])
}

// --- The utility library ---

fn gu_alpha_source(cpu: &mut Cpu, a: &[u32]) -> Option<u32> {
    gu::alpha_source(cpu, a[0]);
    Some(0)
}

fn gu_color_combine_function(cpu: &mut Cpu, a: &[u32]) -> Option<u32> {
    gu::color_combine_function(cpu, a[0]);
    Some(0)
}

fn gu_tex_combine_function(cpu: &mut Cpu, a: &[u32]) -> Option<u32> {
    gu::tex_combine_function(cpu, a[0], a[1]);
    Some(0)
}

/// guFogTableIndexToW: a float, returned on the FPU's stack.
fn gu_fog_table_index_to_w(cpu: &mut Cpu, a: &[u32]) -> Option<u32> {
    cpu.fpu_push_f64(gu::fog_table_index_to_w(a[0] as i32) as f64);
    Some(0)
}

fn write_fog(cpu: &mut Cpu, at: u32, table: &[u8; api::FOG_TABLE_SIZE]) -> Option<u32> {
    cpu.bus.guest_write_bytes(at, table);
    Some(0)
}

fn gu_fog_generate_exp(cpu: &mut Cpu, a: &[u32]) -> Option<u32> {
    write_fog(cpu, a[0], &gu::fog_generate_exp(f(a[1])))
}

fn gu_fog_generate_exp2(cpu: &mut Cpu, a: &[u32]) -> Option<u32> {
    write_fog(cpu, a[0], &gu::fog_generate_exp2(f(a[1])))
}

fn gu_fog_generate_linear(cpu: &mut Cpu, a: &[u32]) -> Option<u32> {
    write_fog(cpu, a[0], &gu::fog_generate_linear(f(a[1]), f(a[2])))
}

fn gu_endian_swap_bytes(_: &mut Cpu, a: &[u32]) -> Option<u32> {
    Some((a[0] as u16).swap_bytes() as u32)
}

fn gu_endian_swap_words(_: &mut Cpu, a: &[u32]) -> Option<u32> {
    Some(a[0].rotate_left(16))
}

fn gu_draw_triangle_with_clip(cpu: &mut Cpu, a: &[u32]) -> Option<u32> {
    let (p, q, r) = (vertex(cpu, a[0])?, vertex(cpu, a[1])?, vertex(cpu, a[2])?);
    gu::draw_triangle_with_clip(cpu, &p, &q, &r);
    Some(0)
}

fn gu_draw_polygon_vertex_list_with_clip(cpu: &mut Cpu, a: &[u32]) -> Option<u32> {
    let list = vertices(cpu, a[0], a[1])?;
    gu::draw_polygon_with_clip(cpu, &list);
    Some(0)
}

/// The texture of guTexAllocateMemory's arguments from the format on:
/// format, mipmap mode, smallest and largest level, aspect, clamping
/// and filters.
fn mipmap_from(m: &mut MipMap, a: &[u32]) {
    m.info = TexInfo {
        format: a[0],
        small_lod: a[2],
        large_lod: a[3],
        aspect: a[4],
    };
    m.mm_mode = a[1];
    m.clamp = [a[5], a[6]];
    m.filter = [a[7], a[8]];
}

fn gu_tex_allocate_memory(cpu: &mut Cpu, a: &[u32]) -> Option<u32> {
    let mut m = MipMap {
        tmu: a[0],
        even_odd: a[1] & 0xFF,
        width: a[2],
        height: a[3],
        lod_bias: f(a[13]),
        trilinear: b(a[14]),
        ..MipMap::default()
    };
    mipmap_from(&mut m, &a[4..13]);
    Some(gu::tex_allocate(cpu, m))
}

/// guTexChangeAttributes: an argument of -1 keeps what the texture had.
fn gu_tex_change_attributes(cpu: &mut Cpu, a: &[u32]) -> Option<u32> {
    let ok = gu::tex_change_attributes(cpu, a[0], |m| {
        let keep = |arg: u32, old: u32| if arg == u32::MAX { old } else { arg };
        m.width = keep(a[1], m.width);
        m.height = keep(a[2], m.height);
        m.info.format = keep(a[3], m.info.format);
        m.mm_mode = keep(a[4], m.mm_mode);
        m.info.small_lod = keep(a[5], m.info.small_lod);
        m.info.large_lod = keep(a[6], m.info.large_lod);
        m.info.aspect = keep(a[7], m.info.aspect);
        for i in 0..2 {
            m.clamp[i] = keep(a[8 + i], m.clamp[i]);
            m.filter[i] = keep(a[10 + i], m.filter[i]);
        }
    });
    Some(ok as u32)
}

fn gu_tex_source(cpu: &mut Cpu, a: &[u32]) -> Option<u32> {
    gu::tex_source(cpu, a[0]);
    Some(0)
}

fn gu_tex_get_current_mip_map(cpu: &mut Cpu, a: &[u32]) -> Option<u32> {
    Some(gu::tex_current(cpu, a[0]))
}

fn gu_tex_mem_query_avail(cpu: &mut Cpu, a: &[u32]) -> Option<u32> {
    Some(gu::tex_mem_query_avail(cpu, a[0]))
}

fn gu_tex_mem_reset(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    gu::tex_mem_reset(cpu);
    Some(0)
}

/// guTexDownloadMipMap: all the texture's levels, and its NCC table
/// for a YIQ texture, which the texture keeps for guTexSource.
fn gu_tex_download_mip_map(cpu: &mut Cpu, a: &[u32]) -> Option<u32> {
    let Some(m) = gu::mipmap(cpu, a[0]) else {
        return Some(0);
    };
    let texels = bytes(cpu, a[1], mipmap_len(&m.info))?;
    if gu::yiq(&m) && a[2] != 0 {
        let packed = ncc_table(cpu, a[2])?;
        gu::set_ncc(cpu, a[0], packed);
        api::tex_download_ncc_table(cpu, m.tmu, api::TEXTABLE_NCC0, &packed);
        api::tex_ncc_table(cpu, m.tmu, api::TEXTABLE_NCC0);
    }
    api::tex_download_mipmap(cpu, m.tmu, m.start, m.even_odd, &m.info, &texels);
    Some(0)
}

/// guTexDownloadMipMapLevel: one level from `*src`, which moves on
/// past it.
fn gu_tex_download_mip_map_level(cpu: &mut Cpu, a: &[u32]) -> Option<u32> {
    let Some(m) = gu::mipmap(cpu, a[0]) else {
        return Some(0);
    };
    let src = cpu.bus.guest_read_32(a[2]);
    if cpu.bus.guest_faulted() || !cpu.bus.guest_probe(a[2], 4, true) {
        return None;
    }
    let lod = a[1];
    if lod > 8 {
        return Some(0);
    }
    let len = level_len(m.info.aspect, lod, m.info.format);
    let texels = bytes(cpu, src, len)?;
    let (_, h) = api::level_size(m.info.aspect, lod);
    api::tex_download_level(
        cpu,
        m.tmu,
        m.start,
        lod,
        m.info.large_lod,
        m.info.aspect,
        m.info.format,
        m.even_odd,
        &texels,
        0,
        h - 1,
    );
    cpu.bus.guest_write_32(a[2], src.wrapping_add(len as u32));
    Some(0)
}

/// guAADrawTriangleWithClip: clipped, without antialiased edges.
fn gu_aa_draw_triangle_with_clip(cpu: &mut Cpu, a: &[u32]) -> Option<u32> {
    gu_draw_triangle_with_clip(cpu, a)
}

/// The functions, in the order of their names. Append only (see
/// `stubs`).
pub static APIS: &[Api] = &[
    stdcall(
        "_ConvertAndDownloadRle@64",
        1,
        16,
        missing_convert_and_download_rle,
    ),
    stdcall("_grAADrawLine@8", 2, 2, gr_draw_line),
    stdcall("_grAADrawPoint@4", 3, 1, gr_draw_point),
    stdcall("_grAADrawPolygon@12", 4, 3, gr_draw_polygon),
    stdcall(
        "_grAADrawPolygonVertexList@8",
        5,
        2,
        gr_draw_polygon_vertex_list,
    ),
    stdcall("_grAADrawTriangle@24", 6, 6, gr_aa_draw_triangle),
    stdcall("_grAlphaBlendFunction@16", 7, 4, gr_alpha_blend_function),
    stdcall("_grAlphaCombine@20", 8, 5, gr_alpha_combine),
    stdcall(
        "_grAlphaControlsITRGBLighting@4",
        9,
        1,
        gr_alpha_controls_itrgb_lighting,
    ),
    stdcall("_grAlphaTestFunction@4", 10, 1, gr_alpha_test_function),
    stdcall(
        "_grAlphaTestReferenceValue@4",
        11,
        1,
        gr_alpha_test_reference_value,
    ),
    stdcall("_grBufferClear@12", 12, 3, gr_buffer_clear),
    stdcall("_grBufferNumPending@0", 13, 0, gr_buffer_num_pending),
    stdcall("_grBufferSwap@4", 14, 1, gr_buffer_swap),
    stdcall("_grCheckForRoom@4", 15, 1, gr_check_for_room),
    stdcall("_grChromakeyMode@4", 16, 1, gr_chromakey_mode),
    stdcall("_grChromakeyValue@4", 17, 1, gr_chromakey_value),
    stdcall("_grClipWindow@16", 18, 4, gr_clip_window),
    stdcall("_grColorCombine@20", 19, 5, gr_color_combine),
    stdcall("_grColorMask@8", 20, 2, gr_color_mask),
    stdcall("_grConstantColorValue4@16", 21, 4, gr_constant_color_value4),
    stdcall("_grConstantColorValue@4", 22, 1, gr_constant_color_value),
    stdcall("_grCullMode@4", 23, 1, gr_cull_mode),
    stdcall("_grDepthBiasLevel@4", 24, 1, gr_depth_bias_level),
    stdcall("_grDepthBufferFunction@4", 25, 1, gr_depth_buffer_function),
    stdcall("_grDepthBufferMode@4", 26, 1, gr_depth_buffer_mode),
    stdcall("_grDepthMask@4", 27, 1, gr_depth_mask),
    stdcall("_grDisableAllEffects@0", 28, 0, gr_disable_all_effects),
    stdcall("_grDitherMode@4", 29, 1, gr_dither_mode),
    stdcall("_grDrawLine@8", 30, 2, gr_draw_line),
    stdcall("_grDrawPlanarPolygon@12", 31, 3, gr_draw_polygon),
    stdcall(
        "_grDrawPlanarPolygonVertexList@8",
        32,
        2,
        gr_draw_polygon_vertex_list,
    ),
    stdcall("_grDrawPoint@4", 33, 1, gr_draw_point),
    stdcall("_grDrawPolygon@12", 34, 3, gr_draw_polygon),
    stdcall(
        "_grDrawPolygonVertexList@8",
        35,
        2,
        gr_draw_polygon_vertex_list,
    ),
    stdcall("_grDrawTriangle@12", 36, 3, gr_draw_triangle),
    stdcall("_grErrorSetCallback@4", 37, 1, gr_error_set_callback),
    stdcall("_grFogColorValue@4", 38, 1, gr_fog_color_value),
    stdcall("_grFogMode@4", 39, 1, gr_fog_mode),
    stdcall("_grFogTable@4", 40, 1, gr_fog_table),
    stdcall(
        "_grGammaCorrectionValue@4",
        41,
        1,
        gr_gamma_correction_value,
    ),
    stdcall("_grGlideGetState@4", 42, 1, gr_glide_get_state),
    stdcall("_grGlideGetVersion@4", 43, 1, gr_glide_get_version),
    stdcall("_grGlideInit@0", 44, 0, gr_glide_init),
    stdcall("_grGlideSetState@4", 45, 1, gr_glide_set_state),
    stdcall("_grGlideShamelessPlug@4", 46, 1, gr_glide_shameless_plug),
    stdcall("_grGlideShutdown@0", 47, 0, gr_glide_shutdown),
    stdcall("_grHints@8", 48, 2, gr_hints),
    stdcall("_grLfbConstantAlpha@4", 49, 1, gr_lfb_constant_alpha),
    stdcall("_grLfbConstantDepth@4", 50, 1, gr_lfb_constant_depth),
    stdcall("_grLfbLock@24", 51, 6, gr_lfb_lock),
    stdcall("_grLfbReadRegion@28", 52, 7, gr_lfb_read_region),
    stdcall("_grLfbUnlock@8", 53, 2, gr_lfb_unlock),
    stdcall("_grLfbWriteColorFormat@4", 54, 1, gr_lfb_write_color_format),
    stdcall(
        "_grLfbWriteColorSwizzle@8",
        55,
        2,
        gr_lfb_write_color_swizzle,
    ),
    stdcall("_grLfbWriteRegion@32", 56, 8, gr_lfb_write_region),
    stdcall("_grRenderBuffer@4", 57, 1, gr_render_buffer),
    stdcall("_grResetTriStats@0", 58, 0, gr_reset_tri_stats),
    stdcall("_grSplash@20", 59, 5, missing_gr_splash),
    stdcall(
        "_grSstConfigPipeline@12",
        60,
        3,
        missing_gr_sst_config_pipeline,
    ),
    stdcall("_grSstControl@4", 61, 1, gr_sst_control),
    stdcall("_grSstIdle@0", 62, 0, gr_sst_idle),
    stdcall("_grSstIsBusy@0", 63, 0, gr_sst_is_busy),
    stdcall("_grSstOrigin@4", 64, 1, gr_sst_origin),
    stdcall("_grSstPerfStats@4", 65, 1, gr_sst_perf_stats),
    stdcall("_grSstQueryBoards@4", 66, 1, gr_sst_query_boards),
    stdcall("_grSstQueryHardware@4", 67, 1, gr_sst_query_hardware),
    stdcall("_grSstResetPerfStats@0", 68, 0, gr_sst_reset_perf_stats),
    stdcall("_grSstScreenHeight@0", 69, 0, gr_sst_screen_height),
    stdcall("_grSstScreenWidth@0", 70, 0, gr_sst_screen_width),
    stdcall("_grSstSelect@4", 71, 1, gr_sst_select),
    stdcall("_grSstStatus@0", 72, 0, gr_sst_status),
    stdcall("_grSstVRetraceOn@0", 73, 0, gr_sst_v_retrace_on),
    stdcall("_grSstVidMode@8", 74, 2, missing_gr_sst_vid_mode),
    stdcall("_grSstVideoLine@0", 75, 0, gr_sst_video_line),
    stdcall("_grSstWinClose@0", 76, 0, gr_sst_win_close),
    stdcall("_grSstWinOpen@28", 77, 7, gr_sst_win_open),
    stdcall("_grTexCalcMemRequired@16", 78, 4, gr_tex_calc_mem_required),
    stdcall("_grTexClampMode@12", 79, 3, gr_tex_clamp_mode),
    stdcall("_grTexCombine@28", 80, 7, gr_tex_combine),
    stdcall("_grTexCombineFunction@8", 81, 2, gr_tex_combine_function),
    stdcall("_grTexDetailControl@16", 82, 4, gr_tex_detail_control),
    stdcall("_grTexDownloadMipMap@16", 83, 4, gr_tex_download_mip_map),
    stdcall(
        "_grTexDownloadMipMapLevel@32",
        84,
        8,
        gr_tex_download_mip_map_level,
    ),
    stdcall(
        "_grTexDownloadMipMapLevelPartial@40",
        85,
        10,
        gr_tex_download_mip_map_level_partial,
    ),
    stdcall("_grTexDownloadTable@12", 86, 3, gr_tex_download_table),
    stdcall(
        "_grTexDownloadTablePartial@20",
        87,
        5,
        gr_tex_download_table_partial,
    ),
    stdcall("_grTexFilterMode@12", 88, 3, gr_tex_filter_mode),
    stdcall("_grTexLodBiasValue@8", 89, 2, gr_tex_lod_bias_value),
    stdcall("_grTexMaxAddress@4", 90, 1, gr_tex_max_address),
    stdcall("_grTexMinAddress@4", 91, 1, gr_tex_min_address),
    stdcall("_grTexMipMapMode@12", 92, 3, gr_tex_mip_map_mode),
    stdcall("_grTexMultibase@8", 93, 2, gr_tex_multibase),
    stdcall("_grTexMultibaseAddress@20", 94, 5, gr_tex_multibase_address),
    stdcall("_grTexNCCTable@8", 95, 2, gr_tex_ncc_table),
    stdcall("_grTexSource@16", 96, 4, gr_tex_source),
    stdcall(
        "_grTexTextureMemRequired@8",
        97,
        2,
        gr_tex_texture_mem_required,
    ),
    stdcall("_grTriStats@8", 98, 2, gr_tri_stats),
    stdcall("_gu3dfGetInfo@8", 99, 2, file3df::get_info),
    stdcall("_gu3dfLoad@8", 100, 2, file3df::load),
    stdcall(
        "_guAADrawTriangleWithClip@12",
        101,
        3,
        gu_aa_draw_triangle_with_clip,
    ),
    stdcall("_guAlphaSource@4", 102, 1, gu_alpha_source),
    stdcall(
        "_guColorCombineFunction@4",
        103,
        1,
        gu_color_combine_function,
    ),
    stdcall(
        "_guDrawPolygonVertexListWithClip@8",
        104,
        2,
        gu_draw_polygon_vertex_list_with_clip,
    ),
    stdcall(
        "_guDrawTriangleWithClip@12",
        105,
        3,
        gu_draw_triangle_with_clip,
    ),
    stdcall("_guEncodeRLE16@16", 106, 4, missing_gu_encode_rle16),
    stdcall("_guEndianSwapBytes@4", 107, 1, gu_endian_swap_bytes),
    stdcall("_guEndianSwapWords@4", 108, 1, gu_endian_swap_words),
    stdcall("_guFbReadRegion@24", 109, 6, missing_gu_fb_read_region),
    stdcall("_guFbWriteRegion@24", 110, 6, missing_gu_fb_write_region),
    stdcall("_guFogGenerateExp2@8", 111, 2, gu_fog_generate_exp2),
    stdcall("_guFogGenerateExp@8", 112, 2, gu_fog_generate_exp),
    stdcall("_guFogGenerateLinear@12", 113, 3, gu_fog_generate_linear),
    stdcall("_guFogTableIndexToW@4", 114, 1, gu_fog_table_index_to_w),
    stdcall("_guMPDrawTriangle@12", 115, 3, missing_gu_mp_draw_triangle),
    stdcall("_guMPInit@0", 116, 0, missing_gu_mp_init),
    stdcall(
        "_guMPTexCombineFunction@4",
        117,
        1,
        missing_gu_mp_tex_combine_function,
    ),
    stdcall("_guMPTexSource@8", 118, 2, missing_gu_mp_tex_source),
    stdcall("_guMovieSetName@4", 119, 1, missing_gu_movie_set_name),
    stdcall("_guMovieStart@0", 120, 0, missing_gu_movie_start),
    stdcall("_guMovieStop@0", 121, 0, missing_gu_movie_stop),
    stdcall("_guTexAllocateMemory@60", 122, 15, gu_tex_allocate_memory),
    stdcall(
        "_guTexChangeAttributes@48",
        123,
        12,
        gu_tex_change_attributes,
    ),
    stdcall("_guTexCombineFunction@8", 124, 2, gu_tex_combine_function),
    stdcall(
        "_guTexCreateColorMipMap@0",
        125,
        0,
        missing_gu_tex_create_color_mip_map,
    ),
    stdcall("_guTexDownloadMipMap@12", 126, 3, gu_tex_download_mip_map),
    stdcall(
        "_guTexDownloadMipMapLevel@12",
        127,
        3,
        gu_tex_download_mip_map_level,
    ),
    stdcall(
        "_guTexGetCurrentMipMap@4",
        128,
        1,
        gu_tex_get_current_mip_map,
    ),
    stdcall(
        "_guTexGetMipMapInfo@4",
        129,
        1,
        missing_gu_tex_get_mip_map_info,
    ),
    stdcall("_guTexMemQueryAvail@4", 130, 1, gu_tex_mem_query_avail),
    stdcall("_guTexMemReset@0", 131, 0, gu_tex_mem_reset),
    stdcall("_guTexSource@4", 132, 1, gu_tex_source),
    stdcall("_pciClose@0", 133, 0, missing_pci_close),
    stdcall("_pciDeviceExists@4", 134, 1, missing_pci_device_exists),
    stdcall("_pciFindCard@12", 135, 3, missing_pci_find_card),
    stdcall("_pciFindCardMulti@16", 136, 4, missing_pci_find_card_multi),
    stdcall("_pciFindFreeMTRR@4", 137, 1, missing_pci_find_free_mtrr),
    stdcall("_pciFindMTRRMatch@16", 138, 4, missing_pci_find_mtrr_match),
    stdcall("_pciGetConfigData@20", 139, 5, missing_pci_get_config_data),
    stdcall("_pciGetErrorCode@0", 140, 0, missing_pci_get_error_code),
    stdcall("_pciGetErrorString@0", 141, 0, missing_pci_get_error_string),
    stdcall("_pciMapCard@20", 142, 5, missing_pci_map_card),
    stdcall("_pciMapCardMulti@24", 143, 6, missing_pci_map_card_multi),
    stdcall(
        "_pciMapPhysicalToLinear@12",
        144,
        3,
        missing_pci_map_physical_to_linear,
    ),
    stdcall("_pciOpen@0", 145, 0, missing_pci_open),
    stdcall(
        "_pciOutputDebugString@4",
        146,
        1,
        missing_pci_output_debug_string,
    ),
    stdcall("_pciSetConfigData@20", 147, 5, missing_pci_set_config_data),
    stdcall("_pciSetMTRR@16", 148, 4, missing_pci_set_mtrr),
    stdcall(
        "_pciSetPassThroughBase@4",
        149,
        1,
        missing_pci_set_pass_through_base,
    ),
    stdcall("_pciUnmapPhysical@8", 150, 2, missing_pci_unmap_physical),
];
