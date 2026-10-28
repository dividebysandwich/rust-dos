//! Saved-game state usage, collected on the emulation thread in tests.
//! Normal builds have no collector or calls into it. Projected area is
//! unclipped triangle area, not pixel work or a timing measurement.

use super::RasterState;
use std::cell::RefCell;
use std::collections::HashMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
struct Key {
    tmus: usize,
    registers: [u32; 6],
}

#[derive(Default)]
struct Count {
    triangles: u64,
    projected_area: f64,
    variant: Option<usize>,
}

thread_local! {
    static COUNTS: RefCell<Option<HashMap<Key, Count>>> = const { RefCell::new(None) };
}

pub(crate) fn record(st: &RasterState, tmus: usize, verts: [(f32, f32); 3]) {
    COUNTS.with(|counts| {
        let mut guard = counts.borrow_mut();
        let Some(counts) = guard.as_mut() else { return };
        let tex = |i: usize| st.tmu[i].as_ref().map_or(0, |t| t.mode);
        let registers = [
            st.fbz_mode,
            st.fbz_color_path,
            st.alpha_mode,
            st.fog_mode,
            tex(0),
            tex(1),
        ];
        let fixed = super::full(tmus, registers);
        let key = Key {
            tmus,
            registers: fixed.map(|(mask, value)| value & mask),
        };
        let [(ax, ay), (bx, by), (cx, cy)] = verts.map(|(x, y)| (f64::from(x), f64::from(y)));
        let area = ((bx - ax) * (cy - ay) - (by - ay) * (cx - ax)).abs() * 0.5;
        let count = counts.entry(key).or_insert_with(|| Count {
            variant: super::variant(st, tmus),
            ..Default::default()
        });
        count.triangles += 1;
        if area.is_finite() {
            count.projected_area += area;
        }
    });
}

struct Capture;

impl Capture {
    fn start() -> Self {
        COUNTS.with(|counts| {
            let previous = counts.replace(Some(HashMap::new()));
            assert!(previous.is_none(), "a profile is already active");
        });
        Self
    }

    fn finish(self) -> HashMap<Key, Count> {
        COUNTS.with(|counts| counts.take().unwrap())
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        COUNTS.with(|counts| counts.replace(None));
    }
}

/// RUST_DOS_VOODOO_STATE is read-only. PROFILE_DIR must already contain a
/// scratch copy named run.C.img; loading can revert and write that copy.
/// BATCHES and INSTRUCTIONS select the observation length. This collects
/// submitted states and a final picture, not benchmark timings.
#[test]
#[ignore = "requires a saved Voodoo scene and a separate scratch disk"]
fn profile_saved_voodoo_scene() {
    use crate::cpu::Cpu;
    use crate::exec::{NoHook, run_batch};
    use crate::savestate::{machine, slots};
    use std::{fs, path::PathBuf};

    let path = PathBuf::from(std::env::var_os("RUST_DOS_VOODOO_STATE").expect("state path"));
    let dir =
        PathBuf::from(std::env::var_os("RUST_DOS_VOODOO_PROFILE_DIR").expect("output directory"))
            .canonicalize()
            .unwrap();
    let scratch = dir
        .join("run.C.img")
        .canonicalize()
        .expect("scratch disk copy");
    assert_ne!(dir, path.canonicalize().unwrap().parent().unwrap());
    let (header, state) = slots::decode(&fs::read(&path).unwrap()).unwrap();
    let mut settings = slots::machine_settings(&header.machine, &Default::default());
    settings.voodoo.renderer = crate::voodoo::Renderer::Software;
    let mut cpu = Cpu::with_memory(dir.clone(), header.memsize);
    crate::hardware::configure(&mut cpu, &settings, crate::keylayout::Layout::us());
    cpu.bus
        .disk
        .folders_from_state
        .push((crate::disk::DRIVE_C, scratch.clone()));
    crate::savestate::disks::offer_copies(&mut cpu, &path);
    machine::load(&mut cpu, &state).unwrap();
    let disk = cpu
        .bus
        .disk
        .bios_image(crate::disk::DRIVE_C)
        .expect("boot disk");
    assert_eq!(disk.path().canonicalize().unwrap(), scratch);
    cpu.bus
        .voodoo
        .as_mut()
        .expect("Voodoo card")
        .set_software_picture(true);

    let batches: u64 = std::env::var("RUST_DOS_VOODOO_BATCHES")
        .ok()
        .map(|v| v.parse().unwrap())
        .unwrap_or(60);
    let instructions: u64 = std::env::var("RUST_DOS_VOODOO_INSTRUCTIONS")
        .ok()
        .map(|v| v.parse().unwrap())
        .unwrap_or(1_600_000);
    let capture = Capture::start();
    for _ in 0..batches {
        cpu.bus.start_batch(cpu.bus.clock.icount + instructions);
        run_batch(&mut cpu, &mut NoHook, false);
        cpu.bus.sync_display();
        cpu.bus.voodoo.as_mut().unwrap().frame_buffer();
    }
    let mut counts: Vec<_> = capture.finish().into_iter().collect();
    assert!(!counts.is_empty(), "the scene submitted no triangles");
    counts.sort_by(|(a, ca), (b, cb)| cb.triangles.cmp(&ca.triangles).then_with(|| a.cmp(b)));
    let triangles: u64 = counts.iter().map(|(_, count)| count.triangles).sum();
    let generic: u64 = counts
        .iter()
        .filter(|(_, c)| c.variant.is_none())
        .map(|(_, c)| c.triangles)
        .sum();
    let states: Vec<_> = counts
        .into_iter()
        .map(|(key, count)| {
            let kind = match count.variant {
                None => "generic",
                Some(index) if index < super::FIRST_COMBINE => "full",
                Some(_) => "combine",
            };
            serde_json::json!({
                "tmus": key.tmus,
                "registers": key.registers.map(|value| format!("0x{value:08X}")),
                "triangles": count.triangles,
                "projected_area": count.projected_area,
                "variant": count.variant,
                "kind": kind,
            })
        })
        .collect();
    let report = serde_json::json!({
        "state": path,
        "batches": batches,
        "instructions_per_batch": instructions,
        "triangles": triangles,
        "generic_triangles": generic,
        "area_unit": "unclipped projected triangle area, not shaded pixels",
        "states": states,
    });
    fs::write(
        dir.join("voodoo-states.json"),
        serde_json::to_vec_pretty(&report).unwrap(),
    )
    .unwrap();
    let card = cpu.bus.voodoo.as_mut().unwrap();
    card.prepare_display();
    let (width, height) = card.picture_size();
    let mut picture = crate::video::Frame::new(width, height);
    card.render(&mut picture.rgba, width as usize);
    crate::capture::png::save(&picture, &dir.join("last-frame.png")).unwrap();
    println!(
        "{triangles} triangles in {} states; {generic} use generic",
        states.len()
    );
}
