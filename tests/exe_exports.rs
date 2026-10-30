//! rust-dos.exe's exports: NVIDIA's and AMD's drivers put a program on a
//! laptop's discrete graphics chip when its .exe exports
//! `NvOptimusEnablement` or `AmdPowerXpressRequestHighPerformance` as 1.
#![cfg(all(windows, target_env = "msvc"))]

fn u16_at(b: &[u8], at: usize) -> usize {
    u16::from_le_bytes([b[at], b[at + 1]]) as usize
}

fn u32_at(b: &[u8], at: usize) -> usize {
    u32::from_le_bytes(b[at..at + 4].try_into().unwrap()) as usize
}

/// The names the PE image `b` exports, each with the file offset of what
/// it names.
fn exports(b: &[u8]) -> Vec<(String, usize)> {
    let pe = u32_at(b, 0x3C);
    assert_eq!(&b[pe..pe + 4], b"PE\0\0");
    let sections = u16_at(b, pe + 6);
    let optional = pe + 24;
    let optional_size = u16_at(b, pe + 20);
    // PE32+: the data directories start at 112, the export table first.
    assert_eq!(u16_at(b, optional), 0x20B);
    let (dir, dir_size) = (u32_at(b, optional + 112), u32_at(b, optional + 116));
    assert!(dir_size > 0, "no export table");
    let table = optional + optional_size;
    let offset = |rva: usize| {
        (0..sections)
            .map(|i| table + i * 40)
            .find_map(|s| {
                let (size, at, raw) = (u32_at(b, s + 8), u32_at(b, s + 12), u32_at(b, s + 20));
                (at..at + size).contains(&rva).then(|| rva - at + raw)
            })
            .unwrap()
    };
    let dir = offset(dir);
    let (names, functions) = (offset(u32_at(b, dir + 32)), offset(u32_at(b, dir + 28)));
    let ordinals = offset(u32_at(b, dir + 36));
    (0..u32_at(b, dir + 24))
        .map(|i| {
            let name = offset(u32_at(b, names + i * 4));
            let end = b[name..].iter().position(|&c| c == 0).unwrap();
            let function = u32_at(b, functions + u16_at(b, ordinals + i * 2) * 4);
            (
                String::from_utf8_lossy(&b[name..name + end]).into_owned(),
                offset(function),
            )
        })
        .collect()
}

#[test]
fn the_exe_asks_for_the_discrete_graphics_chip() {
    let exe = std::fs::read(env!("CARGO_BIN_EXE_rust-dos")).unwrap();
    let exports = exports(&exe);
    for name in [
        "NvOptimusEnablement",
        "AmdPowerXpressRequestHighPerformance",
    ] {
        let &(_, at) = exports
            .iter()
            .find(|(n, _)| n == name)
            .unwrap_or_else(|| panic!("{} not in {:?}", name, exports));
        assert_eq!(u32_at(&exe, at), 1, "{}", name);
    }
}
