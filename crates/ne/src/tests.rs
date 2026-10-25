use super::*;

#[test]
fn a_dos_or_pe_program_is_no_ne_file() {
    assert!(!is_ne(b"MZ\x90\x00"));
    let mut file = vec![0u8; 0x100];
    file[..2].copy_from_slice(b"MZ");
    file[0x3C] = 0x80;
    assert!(!is_ne(&file));
    file[0x80..0x84].copy_from_slice(b"PE\0\0");
    assert!(!is_ne(&file));
    file[0x80..0x82].copy_from_slice(b"NE");
    assert!(is_ne(&file));
    file[0x3C..0x40].copy_from_slice(&0x1_0000u32.to_le_bytes());
    assert!(!is_ne(&file));
}

/// A program with a code and a data segment, imports of USER by ordinal
/// and of KERNEL by name, two exports and a resource.
fn sample() -> Vec<u8> {
    let mut b = Builder::new("sample", MULTIPLEDATA);
    let user = b.import_module("USER");
    let kernel = b.import_module("KERNEL");
    let name = b.imported_name("GETVERSION");
    let code = b.segment(SegmentSpec {
        data: vec![0x90; 0x40],
        flags: SEG_PRELOAD,
        alloc: 0,
        relocations: vec![
            Relocation {
                source: ADDR_FARPTR,
                additive: false,
                offset: 0x10,
                target: Target::Ordinal {
                    module: user,
                    ordinal: 1,
                },
            },
            Relocation {
                source: ADDR_FARPTR,
                additive: false,
                offset: 0x20,
                target: Target::Name {
                    module: kernel,
                    name_offset: name,
                },
            },
            Relocation {
                source: ADDR_SELECTOR,
                additive: true,
                offset: 0x30,
                target: Target::Internal {
                    segment: 2,
                    offset: 0,
                },
            },
        ],
    });
    let data = b.segment(SegmentSpec {
        data: vec![1, 2, 3, 4],
        flags: SEG_DATA | SEG_PRELOAD,
        alloc: 0x100,
        relocations: Vec::new(),
    });
    b.auto_data(data, 0x400, 0x2000);
    b.entry_point(code, 0x08);
    b.initial_stack(data, 0);
    b.entry(
        1,
        Entry::Code {
            segment: code as u8,
            offset: 0x20,
            flags: ENTRY_EXPORTED,
            moveable: false,
        },
    );
    b.entry(
        3,
        Entry::Code {
            segment: code as u8,
            offset: 0x30,
            flags: ENTRY_EXPORTED,
            moveable: true,
        },
    );
    b.entry(
        4,
        Entry::Constant {
            value: 0x1234,
            flags: ENTRY_EXPORTED,
        },
    );
    b.export("WNDPROC", 1, true);
    b.export("OTHERPROC", 3, false);
    b.resource(ResId::Int(5), ResId::Name("ABOUT".into()), 0x30, b"dialog");
    b.resource(ResId::Int(6), ResId::Int(1), 0x30, b"strings!");
    b.build()
}

#[test]
fn a_built_module_reads_back() {
    let file = sample();
    assert!(is_ne(&file));
    let m = Module::parse(&file).unwrap();
    assert_eq!(m.name(), "SAMPLE");
    assert_eq!(m.flags, MULTIPLEDATA);
    assert!(!m.is_dll());
    assert_eq!((m.auto_data, m.heap, m.stack), (2, 0x400, 0x2000));
    assert_eq!(m.cs_ip, (1, 8));
    assert_eq!(m.ss_sp, (2, 0));
    assert_eq!(m.target_os, OS_WINDOWS);
    assert_eq!(m.module_refs, vec!["USER", "KERNEL"]);
    assert_eq!(m.segments.len(), 2);
    let code = m.segments[0];
    assert_eq!(code.data(&file).unwrap(), &[0x90; 0x40][..]);
    assert!(code.has_relocations());
    let data = m.segments[1];
    assert!(data.is_data());
    assert_eq!(data.size(), 0x100);
    assert_eq!(data.data(&file).unwrap(), &[1, 2, 3, 4]);

    let relocs = code.relocations(&file).unwrap();
    assert_eq!(relocs.len(), 3);
    assert_eq!(
        relocs[0].target,
        Target::Ordinal {
            module: 1,
            ordinal: 1
        }
    );
    match &relocs[1].target {
        Target::Name {
            module,
            name_offset,
        } => {
            assert_eq!(*module, 2);
            assert_eq!(m.imported_name(*name_offset).as_deref(), Some("GETVERSION"));
        }
        other => panic!("{:?}", other),
    }
    assert!(relocs[2].additive);
    assert_eq!(relocs[2].source, ADDR_SELECTOR);

    assert_eq!(
        m.entry(1),
        Some(Entry::Code {
            segment: 1,
            offset: 0x20,
            flags: ENTRY_EXPORTED,
            moveable: false
        })
    );
    assert_eq!(m.entry(2), None);
    assert!(matches!(
        m.entry(3),
        Some(Entry::Code {
            moveable: true,
            offset: 0x30,
            ..
        })
    ));
    assert_eq!(
        m.entry(4),
        Some(Entry::Constant {
            value: 0x1234,
            flags: ENTRY_EXPORTED
        })
    );
    assert_eq!(m.entry(5), None);
    assert_eq!(m.ordinal_of("wndproc"), Some(1));
    assert_eq!(m.ordinal_of("OtherProc"), Some(3));
    assert_eq!(m.ordinal_of("NOPE"), None);

    let dialog = m
        .find_resource(&ResId::Int(5), &ResId::Name("about".into()))
        .unwrap();
    assert_eq!(&m.resource_data(&file, dialog).unwrap()[..6], b"dialog");
    let strings = m.find_resource(&ResId::Int(6), &ResId::Int(1)).unwrap();
    assert_eq!(&m.resource_data(&file, strings).unwrap()[..8], b"strings!");
    assert!(m.find_resource(&ResId::Int(6), &ResId::Int(2)).is_none());
}

#[test]
fn damaged_tables_are_errors_not_panics() {
    let file = sample();
    for len in (0..file.len()).step_by(7) {
        let _ = Module::parse(&file[..len]);
    }
    let ne = ne_offset(&file).unwrap();
    for at in ne..ne + 0x40 {
        let mut bad = file.clone();
        bad[at] = 0xFF;
        if let Ok(m) = Module::parse(&bad) {
            for s in &m.segments {
                let _ = s.relocations(&bad);
                let _ = s.data(&bad);
            }
        }
    }
}

#[test]
fn many_entry_points_split_into_bundles() {
    let mut b = Builder::new("big", LIBMODULE | SINGLEDATA);
    let code = b.segment(SegmentSpec {
        data: vec![0xCB; 0x1000],
        flags: 0,
        alloc: 0,
        relocations: Vec::new(),
    });
    for ordinal in (1..600).filter(|o| o % 50 != 0) {
        b.entry(
            ordinal,
            Entry::Code {
                segment: code as u8,
                offset: ordinal * 4,
                flags: ENTRY_EXPORTED,
                moveable: false,
            },
        );
    }
    let file = b.build();
    let m = Module::parse(&file).unwrap();
    assert!(m.is_dll());
    for ordinal in 1..600u16 {
        let e = m.entry(ordinal);
        if ordinal % 50 == 0 {
            assert_eq!(e, None, "{}", ordinal);
        } else {
            assert_eq!(
                e,
                Some(Entry::Code {
                    segment: 1,
                    offset: ordinal * 4,
                    flags: ENTRY_EXPORTED,
                    moveable: false
                })
            );
        }
    }
}
