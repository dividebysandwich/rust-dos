//! The readers against archives the test writer builds, and against data
//! an independent decoder expanded.

use crate::write::{self, ArchiveFile, CabinetFile, CabinetGroup, CabinetOptions};
use crate::*;
use std::collections::HashMap;
use std::io::Cursor;

/// Files in memory, found by path in any case.
#[derive(Clone, Default)]
struct MemFolder(HashMap<String, Vec<u8>>);

impl MemFolder {
    fn of(set: Vec<(String, Vec<u8>)>) -> MemFolder {
        MemFolder(set.into_iter().collect())
    }
}

impl Folder for MemFolder {
    fn lookup(&self, path: &str) -> std::io::Result<Lookup> {
        let found: Vec<&Vec<u8>> = self
            .0
            .iter()
            .filter(|(k, _)| fold(k) == fold(path))
            .map(|(_, v)| v)
            .collect();
        Ok(match found[..] {
            [] => Lookup::Missing,
            [data] => Lookup::Found(Box::new(Cursor::new(data.clone())), data.len() as u64),
            _ => Lookup::Several,
        })
    }
}

fn open(set: Vec<(String, Vec<u8>)>, header: &str, compression: Compression) -> Result<Cabinet> {
    Cabinet::open(
        Box::new(MemFolder::of(set)),
        header,
        compression,
        &Limits::default(),
    )
}

/// Bytes that don't compress to nothing: a pattern with some noise.
fn sample(len: usize, seed: u32) -> Vec<u8> {
    let mut x = seed.wrapping_mul(2_654_435_761).wrapping_add(1);
    (0..len)
        .map(|i| {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            if i % 7 < 4 { (i % 251) as u8 } else { x as u8 }
        })
        .collect()
}

fn message(e: Error) -> String {
    e.to_string()
}

#[test]
fn explodes_the_format_descriptions_example() {
    // The example of the format description, as blast corrects it.
    let data = [0x00, 0x04, 0x82, 0x24, 0x25, 0x8f, 0x80, 0x7f];
    assert_eq!(explode(&data, 100).unwrap(), b"AIAIAIAIAIAIA");
}

#[test]
fn explodes_coded_literals_checked_against_an_independent_decoder() {
    // From the refurbished-dinosaurs toolkit's tests: written with coded
    // literals and a 1 KB dictionary, and expanded to the same text by
    // pwexplode, which shares no code with this reader or the writer.
    let data = [
        0x01, 0x04, 0x50, 0x6c, 0xd3, 0xd4, 0xf1, 0x3d, 0x25, 0xb9, 0x81, 0xd2, 0x8b, 0xbd, 0x4a,
        0x76, 0xb6, 0xf2, 0x2c, 0x97, 0xcd, 0xf2, 0x3a, 0xcd, 0x33, 0x74, 0xba, 0x4d, 0xf7, 0xa6,
        0xa5, 0xd8, 0xda, 0x46, 0x55, 0xbc, 0x80, 0x7f,
    ];
    assert_eq!(
        explode(&data, 1000).unwrap(),
        b"Hello, hello, hello! A small TEST of coded literals."
    );
}

#[test]
fn the_code_tables_are_complete() {
    let codes = crate::explode::codes();
    for (code, symbols) in [
        (&codes.literal, 256),
        (&codes.length, 16),
        (&codes.distance, 64),
    ] {
        assert_eq!(
            code.counts[1..].iter().map(|&c| c as usize).sum::<usize>(),
            symbols
        );
        assert!(code.complete());
    }
}

#[test]
fn explodes_what_the_writer_implodes() {
    let data = [
        sample(20_000, 1),
        b"abcabcabcabcabc".repeat(400),
        Vec::new(),
        vec![7],
    ]
    .concat();
    for (coded, bits) in [(true, 4), (true, 5), (true, 6), (false, 4), (false, 6)] {
        let imploded = write::implode(&data, coded, bits, true);
        assert_eq!(
            explode(&imploded, data.len()).unwrap(),
            data,
            "coded {} bits {}",
            coded,
            bits
        );
    }
    let plain = write::implode(b"no matches here", true, 6, false);
    assert_eq!(explode(&plain, 100).unwrap(), b"no matches here");
}

#[test]
fn refuses_malformed_imploded_data() {
    for (data, problem) in [
        (vec![2, 4], "literal mode 2"),
        (vec![0, 7], "dictionary of 7 bits"),
        (vec![0, 4, 0x82], "ends before its end code"),
    ] {
        let e = message(explode(&data, 100).unwrap_err());
        assert!(e.contains(problem), "{:?}: {}", data, e);
    }
    let e = message(explode(&write::implode(&sample(500, 3), true, 6, true), 100).unwrap_err());
    assert!(e.contains("expands past 100"), "{}", e);
}

fn nested_files() -> Vec<CabinetFile> {
    vec![
        CabinetFile::new("", "README.TXT", b"Read me first."),
        CabinetFile::new("DATA", "LEVEL1.DAT", &sample(150_000, 7)),
        CabinetFile {
            compressed: false,
            ..CabinetFile::new("DATA\\SUB", "RAW.BIN", &sample(3000, 9))
        },
        CabinetFile {
            obfuscated: true,
            ..CabinetFile::new("DATA\\SUB", "SECRET.DAT", &sample(70_000, 11))
        },
        CabinetFile {
            obfuscated: true,
            compressed: false,
            ..CabinetFile::new("", "PLAIN.OBF", &sample(999, 13))
        },
        CabinetFile::new("", "EMPTY.TXT", b""),
    ]
}

#[test]
fn lists_and_expands_members_of_each_version() {
    for major in [0, 5, 6] {
        for header_in_cabinet in [false, true] {
            let files = nested_files();
            let options = CabinetOptions {
                header_in_cabinet,
                ..Default::default()
            };
            let set = write::cabinet(major, &files, &options);
            let header = if header_in_cabinet {
                "DATA1.CAB"
            } else {
                "data1.hdr"
            };
            let cab = open(set, header, Compression::Chunks).unwrap();
            assert_eq!(cab.major_version(), major);
            let paths: Vec<&str> = cab.members().iter().map(|m| m.path.as_str()).collect();
            assert_eq!(
                paths,
                [
                    "DATA/LEVEL1.DAT",
                    "DATA/SUB/RAW.BIN",
                    "DATA/SUB/SECRET.DAT",
                    "EMPTY.TXT",
                    "PLAIN.OBF",
                    "README.TXT"
                ],
                "version {}",
                major
            );
            for f in &files {
                let path = if f.directory.is_empty() {
                    f.name.clone()
                } else {
                    format!("{}/{}", f.directory, f.name)
                };
                assert_eq!(
                    cab.read(&path.to_lowercase()).unwrap(),
                    f.data,
                    "{} in version {}",
                    path,
                    major
                );
            }
            let level = cab.find("data\\level1.dat").unwrap();
            assert_eq!(level.entry.index, 1);
            assert_eq!(level.entry.directory, "DATA");
            assert_eq!(cab.read_entry(1).unwrap(), files[1].data);
            assert!(cab.skipped().is_empty());
        }
    }
}

#[test]
fn reads_members_split_across_volumes() {
    for major in [0, 5, 6] {
        for compressed in [true, false] {
            let mut files: Vec<CabinetFile> = (0..4)
                .map(|i| CabinetFile {
                    compressed,
                    ..CabinetFile::new("", &format!("F{}.BIN", i), &sample(40_000, i))
                })
                .collect();
            // Version 5 tells a split file from the volume headers, which
            // can't show the set's last file split (nor can Unshield).
            files.push(CabinetFile {
                compressed,
                ..CabinetFile::new("", "LAST.TXT", b"last")
            });
            let options = CabinetOptions {
                volume_capacity: 25_000,
                ..Default::default()
            };
            let set = write::cabinet(major, &files, &options);
            assert!(set.len() > 3, "several volumes");
            let cab = open(set, "data1.hdr", Compression::Chunks)
                .map_err(|e| format!("{} {} {}", major, compressed, e))
                .unwrap();
            for (i, f) in files.iter().enumerate() {
                assert_eq!(
                    cab.read(&f.name).unwrap(),
                    f.data,
                    "file {} version {} compressed {}",
                    i,
                    major,
                    compressed
                );
            }
        }
    }
}

#[test]
fn follows_links_and_skips_entries_it_cannot_read() {
    let mut files = nested_files();
    files.push(CabinetFile {
        link_to: Some(1),
        ..CabinetFile::new("DATA", "LEVEL1.DAT", b"")
    });
    files.push(CabinetFile {
        link_to: Some(0),
        ..CabinetFile::new("", "LINKED.TXT", b"")
    });
    files.push(CabinetFile {
        invalid: true,
        ..CabinetFile::new("", "GONE.TXT", b"x")
    });
    files.push(CabinetFile {
        data: files[3].data.clone(),
        ..CabinetFile::new("DATA\\SUB", "secret.dat", b"")
    });
    files[9].obfuscated = true;
    let cab = open(
        write::cabinet(6, &files, &CabinetOptions::default()),
        "data1.hdr",
        Compression::Chunks,
    )
    .unwrap();
    let kinds: Vec<(usize, SkipKind)> = cab.skipped().iter().map(|s| (s.index, s.kind)).collect();
    assert_eq!(
        kinds,
        [
            (6, SkipKind::SharesListedData),
            (8, SkipKind::MarkedInvalid),
            (9, SkipKind::DuplicatesListedMember)
        ]
    );
    // A link at another path is a member of its own.
    assert_eq!(cab.read("LINKED.TXT").unwrap(), b"Read me first.");
    let level = cab.find("DATA/LEVEL1.DAT").unwrap();
    assert_eq!(
        level.shared_by.iter().map(|e| e.index).collect::<Vec<_>>(),
        [6]
    );
    assert_eq!(cab.read_entry(6).unwrap(), files[1].data);
    assert_eq!(cab.read_entry(9).unwrap(), files[3].data);
    assert!(matches!(cab.read_entry(8), Err(Error::NotFound(_))));
}

#[test]
fn different_files_at_one_path_are_read_by_index() {
    for major in [5, 6] {
        let files = vec![
            CabinetFile::new("", "SAME.TXT", b"first"),
            CabinetFile::new("", "other.txt", b"other"),
            CabinetFile::new("", "same.txt", b"second"),
        ];
        let cab = open(
            write::cabinet(major, &files, &CabinetOptions::default()),
            "data1.hdr",
            Compression::Chunks,
        )
        .unwrap();
        assert_eq!(cab.members().len(), 1);
        assert!(
            matches!(cab.read("same.txt"), Err(Error::NotFound(m)) if m.contains("different files"))
        );
        let conflict = &cab.conflicts()[0];
        assert_eq!(conflict.path, "SAME.TXT");
        assert_eq!(
            conflict
                .files
                .iter()
                .map(|f| f.entry.index)
                .collect::<Vec<_>>(),
            [0, 2]
        );
        assert_eq!(cab.read_entry(0).unwrap(), b"first");
        assert_eq!(cab.read_entry(2).unwrap(), b"second");
        assert!(
            cab.skipped()
                .iter()
                .all(|s| s.kind == SkipKind::PathHeldByDifferentFiles)
        );
        assert!(
            cab.skipped()[0].reason.contains("files 0 and 2"),
            "{}",
            cab.skipped()[0].reason
        );
    }
}

#[test]
fn checks_the_md5_and_the_size() {
    let files = vec![CabinetFile {
        stored: Some(write::chunks(b"not what the MD5 says")),
        ..CabinetFile::new("", "A.TXT", b"what the MD5 says!!!!")
    }];
    let cab = open(
        write::cabinet(6, &files, &CabinetOptions::default()),
        "data1.hdr",
        Compression::Chunks,
    )
    .unwrap();
    let e = message(cab.read("A.TXT").unwrap_err());
    assert!(e.contains("MD5"), "{}", e);
    for major in [0, 5] {
        for (stored, problem) in [
            (b"shorter".to_vec(), "ends after"),
            (vec![b'x'; 40], "past its declared size"),
        ] {
            let files = vec![CabinetFile {
                stored: Some(write::chunks(&stored)),
                ..CabinetFile::new("", "A.TXT", &[b'y'; 20])
            }];
            let cab = open(
                write::cabinet(major, &files, &CabinetOptions::default()),
                "data1.hdr",
                Compression::Chunks,
            )
            .unwrap();
            let e = message(cab.read("A.TXT").unwrap_err());
            assert!(e.contains(problem), "{}", e);
        }
    }
}

#[test]
fn reads_marker_delimited_members_only_when_asked() {
    for major in [0, 5, 6] {
        let files = vec![
            CabinetFile::new("", "BIG.BIN", &sample(200_000, 5)),
            CabinetFile::new("", "SMALL.TXT", b"small"),
            CabinetFile::new("", "NONE.TXT", b""),
        ];
        let options = CabinetOptions {
            markers: true,
            volume_capacity: 60_000,
            ..Default::default()
        };
        let set = write::cabinet(major, &files, &options);
        let cab = open(set.clone(), "data1.hdr", Compression::Markers).unwrap();
        for f in &files {
            assert_eq!(
                cab.read(&f.name).unwrap(),
                f.data,
                "{} version {}",
                f.name,
                major
            );
        }
        let chunks = open(set, "data1.hdr", Compression::Chunks).unwrap();
        let e = message(chunks.read("BIG.BIN").unwrap_err());
        assert!(e.contains("marker-delimited") || e.contains("-O"), "{}", e);
        // The chunked set read as markers fails too.
        let chunked = write::cabinet(major, &files, &CabinetOptions::default());
        assert!(
            open(chunked, "data1.hdr", Compression::Markers)
                .unwrap()
                .read("BIG.BIN")
                .is_err()
        );
    }
    // One stream whose chunks refer back, and chunk ends inside.
    let data = sample(100_000, 17);
    let stored = write::marker_chunks(&data, 1000, true);
    let files = vec![CabinetFile {
        stored: Some(stored),
        ..CabinetFile::new("", "C.BIN", &data)
    }];
    let cab = open(
        write::cabinet(6, &files, &CabinetOptions::default()),
        "data1.hdr",
        Compression::Markers,
    )
    .unwrap();
    assert_eq!(cab.read("C.BIN").unwrap(), data);
}

#[test]
fn reports_malformed_marker_delimited_data() {
    let data = b"marker data marker data".to_vec();
    let mut no_marker = write::marker_chunks(&data, 8, false);
    no_marker.truncate(no_marker.len() - 4);
    let mut finished = Vec::with_capacity(200);
    let mut c = flate2::Compress::new(flate2::Compression::best(), false);
    c.compress_vec(&data, &mut finished, flate2::FlushCompress::Finish)
        .unwrap();
    for (stored, problem) in [
        (no_marker, "marker"),
        (finished, "final deflate block"),
        (vec![0xff, 0xff, 0xff], "inflate"),
    ] {
        let files = vec![CabinetFile {
            stored: Some(stored),
            ..CabinetFile::new("", "M.TXT", &data)
        }];
        let cab = open(
            write::cabinet(5, &files, &CabinetOptions::default()),
            "data1.hdr",
            Compression::Markers,
        )
        .unwrap();
        let e = message(cab.read("M.TXT").unwrap_err());
        assert!(e.contains(problem), "{}: {}", problem, e);
    }
}

#[test]
fn refuses_damaged_headers_and_other_versions() {
    let files = nested_files();
    let set = write::cabinet(5, &files, &CabinetOptions::default());
    let mut truncated = set.clone();
    truncated[0].1.truncate(0x300);
    assert!(matches!(
        open(truncated, "data1.hdr", Compression::Chunks),
        Err(Error::Invalid(_))
    ));
    let mut volume = set.clone();
    volume[1].1.truncate(1000);
    let e = message(
        open(volume, "data1.hdr", Compression::Chunks)
            .err()
            .unwrap(),
    );
    assert!(e.contains("past the end of volume"), "{}", e);
    let missing: Vec<_> = set
        .iter()
        .filter(|(n, _)| n != "data1.cab")
        .cloned()
        .collect();
    assert!(matches!(
        open(missing, "data1.hdr", Compression::Chunks),
        Err(Error::NotFound(_))
    ));
    for word in [0x0200_0000 | 700, 0x0300_0000] {
        let options = CabinetOptions {
            version_word: Some(word),
            ..Default::default()
        };
        let set = write::cabinet(6, &files, &options);
        assert!(
            matches!(
                open(set, "data1.hdr", Compression::Chunks),
                Err(Error::Unsupported(_))
            ),
            "{:#x}",
            word
        );
    }
    let mut ms = set.clone();
    ms[0].1[..4].copy_from_slice(b"MSCF");
    assert!(
        message(open(ms, "data1.hdr", Compression::Chunks).err().unwrap()).contains("Microsoft")
    );
}

#[test]
fn refuses_paths_that_are_not_portable_and_sets_over_their_limits() {
    for (dir, name) in [
        ("..", "X"),
        ("", "A:B"),
        ("C:\\WIN", "X"),
        ("", "CON.TXT"),
        ("A\\\\B", "X"),
    ] {
        let files = vec![CabinetFile::new(dir, name, b"x")];
        let e = open(
            write::cabinet(6, &files, &CabinetOptions::default()),
            "data1.hdr",
            Compression::Chunks,
        );
        assert!(
            matches!(e, Err(Error::Invalid(m)) if m.contains("portable")),
            "{:?} {:?}",
            dir,
            name
        );
    }
    let files = nested_files();
    let set = write::cabinet(6, &files, &CabinetOptions::default());
    for limits in [
        Limits {
            max_files: 3,
            ..Limits::default()
        },
        Limits {
            max_expanded_bytes: 1000,
            ..Limits::default()
        },
        Limits {
            max_header_bytes: 0x300,
            ..Limits::default()
        },
        Limits {
            max_name_bytes: 4,
            ..Limits::default()
        },
    ] {
        let e = Cabinet::open(
            Box::new(MemFolder::of(set.clone())),
            "data1.hdr",
            Compression::Chunks,
            &limits,
        );
        assert!(matches!(e, Err(Error::Invalid(_))), "{:?}", limits);
    }
}

#[test]
fn opens_a_cab_holding_its_header_past_the_header_limit() {
    let files = vec![CabinetFile::new("", "BIG.BIN", &sample(300_000, 2))];
    let options = CabinetOptions {
        header_in_cabinet: true,
        ..Default::default()
    };
    let set = write::cabinet(6, &files, &options);
    let limits = Limits {
        max_header_bytes: 8192,
        ..Limits::default()
    };
    let cab = Cabinet::open(
        Box::new(MemFolder::of(set)),
        "data1.cab",
        Compression::Chunks,
        &limits,
    )
    .unwrap();
    assert_eq!(cab.read("big.bin").unwrap(), files[0].data);
}

#[test]
fn reports_files_stored_outside_and_reads_one_found_beside_the_header() {
    for major in [0, 5, 6] {
        for markers in [false, true] {
            let mut files = nested_files();
            files.push(CabinetFile {
                outside: true,
                ..CabinetFile::new("EXTRA", "OUT.BIN", &sample(5000, 21))
            });
            files.push(CabinetFile {
                outside: true,
                ..CabinetFile::new("", "MISSING.BIN", b"missing")
            });
            files.push(CabinetFile {
                outside: true,
                ..CabinetFile::new("", "SHORT.BIN", b"short one")
            });
            let options = CabinetOptions {
                markers,
                ..Default::default()
            };
            let mut set = write::cabinet(major, &files, &options);
            set.push((
                "extra/out.bin".to_string(),
                write::stored_bytes(&files[6], markers),
            ));
            set.push(("SHORT.BIN".to_string(), b"short".to_vec()));
            let compression = if markers {
                Compression::Markers
            } else {
                Compression::Chunks
            };
            let cab = open(set, "data1.hdr", compression).unwrap();
            let outside: Vec<(usize, OutsideStatus)> = cab
                .outside_files()
                .iter()
                .map(|o| (o.member.entry.index, o.status))
                .collect();
            assert_eq!(
                outside,
                [
                    (6, OutsideStatus::Available),
                    (7, OutsideStatus::Missing),
                    (8, OutsideStatus::LengthDiffers)
                ],
                "version {}",
                major
            );
            assert!(cab.find("EXTRA/OUT.BIN").is_none());
            assert_eq!(cab.read_entry(6).unwrap(), files[6].data);
            assert!(cab.read_entry(7).is_err());
            assert_eq!(
                cab.skipped()
                    .iter()
                    .filter(|s| s.kind == SkipKind::StoredOutside)
                    .count(),
                3
            );
        }
    }
}

#[test]
fn reports_file_groups_without_choosing_one() {
    for major in [0, 5, 6] {
        let files: Vec<CabinetFile> = (0..6)
            .map(|i| CabinetFile::new("", &format!("F{}", i), &[i as u8; 10]))
            .collect();
        let groups = vec![
            CabinetGroup::new("Program Files", 0, 2),
            CabinetGroup {
                list: 3,
                ..CabinetGroup::new("Data", 2, 3)
            },
            CabinetGroup::new("Help", 5, 5),
        ];
        let options = CabinetOptions {
            groups,
            ..Default::default()
        };
        let cab = open(
            write::cabinet(major, &files, &options),
            "data1.hdr",
            Compression::Chunks,
        )
        .unwrap();
        let names: Vec<_> = cab
            .file_groups()
            .iter()
            .map(|g| g.name.clone().unwrap())
            .collect();
        assert_eq!(names, ["Program Files", "Help", "Data"]);
        let kind = |i: usize| cab.members()[i].entry.groups.kind;
        assert_eq!(
            (0..6).map(kind).collect::<Vec<_>>(),
            [
                MembershipKind::One,
                MembershipKind::One,
                MembershipKind::Several,
                MembershipKind::One,
                MembershipKind::None,
                MembershipKind::One
            ]
        );
        assert_eq!(cab.members()[2].entry.groups.groups, [0, 2]);
        assert!(cab.file_group_problem().is_none());

        // A reversed range and one past the table leave what they may hold
        // undetermined.
        let groups = vec![
            CabinetGroup::new("Back", 3, 1),
            CabinetGroup::new("Past", 4, 99),
        ];
        let options = CabinetOptions {
            groups,
            ..Default::default()
        };
        let cab = open(
            write::cabinet(major, &files, &options),
            "data1.hdr",
            Compression::Chunks,
        )
        .unwrap();
        assert!(cab.file_groups().iter().all(|g| g.problem.is_some()));
        assert!(
            cab.members()
                .iter()
                .all(|m| m.entry.groups.kind == MembershipKind::Undetermined)
        );
    }
    let files = vec![CabinetFile::new("", "A", b"a")];
    let cab = open(
        write::cabinet(6, &files, &CabinetOptions::default()),
        "data1.hdr",
        Compression::Chunks,
    )
    .unwrap();
    assert_eq!(
        cab.members()[0].entry.groups.kind,
        MembershipKind::NoFileGroups
    );
}

#[test]
fn version_words_of_major_version_zero() {
    for word in [0x0100_0004, 0x0100_0000, 0x0200_0000 | 50] {
        let options = CabinetOptions {
            version_word: Some(word),
            ..Default::default()
        };
        let cab = open(
            write::cabinet(0, &nested_files(), &options),
            "data1.hdr",
            Compression::Chunks,
        )
        .unwrap();
        assert_eq!(cab.major_version(), 0);
    }
}

#[test]
fn nothing_in_a_damaged_header_makes_it_panic() {
    let files = nested_files();
    let options = CabinetOptions {
        groups: vec![CabinetGroup::new("G", 0, 3)],
        ..Default::default()
    };
    for major in [0, 5, 6] {
        let set = write::cabinet(major, &files, &options);
        let header = set[0].1.clone();
        let mut x = 0x1234_5678u32;
        for round in 0..400 {
            let mut damaged = header.clone();
            for _ in 0..1 + round % 8 {
                x ^= x << 13;
                x ^= x >> 17;
                x ^= x << 5;
                let at = x as usize % damaged.len();
                damaged[at] = (x >> 8) as u8;
            }
            let mut set = set.clone();
            set[0].1 = damaged;
            if let Ok(cab) = open(set, "data1.hdr", Compression::Chunks) {
                for m in cab.members() {
                    let _ = cab.read(&m.path);
                }
            }
        }
    }
}

fn archive_files() -> Vec<ArchiveFile> {
    vec![
        ArchiveFile::new("", "SETUP.INI", b"[Setup]\r\nName=Game\r\n"),
        ArchiveFile {
            stored: true,
            ..ArchiveFile::new("", "RAW.BIN", &sample(2000, 4))
        },
        ArchiveFile {
            dictionary_bits: 4,
            coded_literals: false,
            ..ArchiveFile::new("DATA", "LEVEL.DAT", &sample(30_000, 6))
        },
        ArchiveFile::new("DATA\\SUB", "TEXT.TXT", &b"text and more text ".repeat(300)),
    ]
}

fn open_archive(bytes: Vec<u8>) -> Result<Archive> {
    Archive::open(Box::new(Cursor::new(bytes)), &Limits::default())
}

#[test]
fn lists_and_expands_an_installshield_3_archive() {
    let files = archive_files();
    let archive = open_archive(write::archive(&files, 0, 1, 1)).unwrap();
    let paths: Vec<&str> = archive.members().iter().map(|m| m.path.as_str()).collect();
    assert_eq!(
        paths,
        [
            "DATA/LEVEL.DAT",
            "DATA/SUB/TEXT.TXT",
            "RAW.BIN",
            "SETUP.INI"
        ]
    );
    for f in &files {
        let path = if f.directory.is_empty() {
            f.name.clone()
        } else {
            format!("{}\\{}", f.directory, f.name)
        };
        assert_eq!(archive.read(&path).unwrap(), f.data);
    }
    assert_eq!(
        archive.members()[0].entry.groups.kind,
        MembershipKind::NoFileGroups
    );
    // A split archive of one part reads; other parts don't.
    assert!(open_archive(write::archive(&files, 3, 1, 1)).is_ok());
    for (flags, parts, part) in [(3, 2, 1), (3, 1, 2), (0, 2, 1)] {
        assert!(matches!(
            open_archive(write::archive(&files, flags, parts, part)),
            Err(Error::Unsupported(_))
        ));
    }
}

#[test]
fn skips_invalid_archive_entries_and_reports_conflicts() {
    let mut files = archive_files();
    files.insert(
        2,
        ArchiveFile {
            invalid: true,
            ..ArchiveFile::new("", "BAD:NAME", b"bad")
        },
    );
    files.insert(3, ArchiveFile::new("", "setup.ini", b"another"));
    let archive = open_archive(write::archive(&files, 0, 1, 1)).unwrap();
    let kinds: Vec<(usize, SkipKind)> = archive
        .skipped()
        .iter()
        .map(|s| (s.index, s.kind))
        .collect();
    assert_eq!(
        kinds,
        [
            (0, SkipKind::PathHeldByDifferentFiles),
            (2, SkipKind::MarkedInvalid),
            (3, SkipKind::PathHeldByDifferentFiles)
        ]
    );
    assert_eq!(archive.skipped()[1].path, None);
    assert_eq!(archive.read_entry(3).unwrap(), b"another");
    assert_eq!(archive.read_entry(0).unwrap(), files[0].data);
    assert!(archive.read("SETUP.INI").is_err());
}

#[test]
fn checks_an_archive_members_size_and_end() {
    let files = archive_files();
    let bytes = write::archive(&files, 0, 1, 1);
    for (change, problem) in [(1i64, "ends after"), (-1, "expands past")] {
        let mut damaged = bytes.clone();
        let entry = write::archive_file_entry(&damaged, 3);
        let size =
            u32::from_le_bytes(damaged[entry + 3..entry + 7].try_into().unwrap()) as i64 + change;
        damaged[entry + 3..entry + 7].copy_from_slice(&(size as u32).to_le_bytes());
        let archive = open_archive(damaged).unwrap();
        let e = message(archive.read_entry(3).unwrap_err());
        assert!(e.contains(problem), "{}", e);
    }
    // A byte left after the end code.
    let mut data = write::implode(b"trailing", true, 6, true);
    data.push(0);
    let mut file = ArchiveFile::new("", "T.TXT", b"trailing");
    file.stored = true;
    let mut bytes = write::archive(&[file], 0, 1, 1);
    let entry = write::archive_file_entry(&bytes, 0);
    let offset = write::ARCHIVE_DATA_START;
    // Make the member the imploded bytes with one more after them, which
    // still fit before the tables? Rebuild instead: store the data whole.
    let mut rebuilt = bytes[..offset].to_vec();
    rebuilt.extend_from_slice(&data);
    let tables = bytes.split_off(offset + 8);
    let shift = data.len() as u32 - 8;
    rebuilt.extend_from_slice(&tables);
    let entry = entry + shift as usize;
    rebuilt[entry + 7..entry + 11].copy_from_slice(&(data.len() as u32).to_le_bytes());
    rebuilt[entry + 25] = 0;
    for at in [41, 51] {
        let v = u32::from_le_bytes(rebuilt[at..at + 4].try_into().unwrap()) + shift;
        rebuilt[at..at + 4].copy_from_slice(&v.to_le_bytes());
    }
    let archive = open_archive(rebuilt).unwrap();
    let e = message(archive.read("T.TXT").unwrap_err());
    assert!(e.contains("left after its end code"), "{}", e);
}

#[test]
fn refuses_archive_tables_that_disagree_and_does_not_panic() {
    let files = archive_files();
    let bytes = write::archive(&files, 0, 1, 1);
    let mut wrong_directory = bytes.clone();
    let entry = write::archive_file_entry(&wrong_directory, 0);
    wrong_directory[entry + 1] = 1;
    let e = message(open_archive(wrong_directory).err().unwrap());
    assert!(e.contains("place it in directory"), "{}", e);
    let mut outside = bytes.clone();
    let entry = write::archive_file_entry(&outside, 1);
    outside[entry + 11..entry + 15].copy_from_slice(&0x00ff_ffffu32.to_le_bytes());
    assert!(message(open_archive(outside).err().unwrap()).contains("outside"));
    let mut x = 0x2468_ace1u32;
    for round in 0..600 {
        let mut damaged = bytes.clone();
        for _ in 0..1 + round % 6 {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            let at = x as usize % damaged.len();
            damaged[at] = (x >> 8) as u8;
        }
        if let Ok(archive) = open_archive(damaged) {
            for i in 0..files.len() {
                let _ = archive.read_entry(i);
            }
        }
    }
}

#[test]
fn relative_paths() {
    assert_eq!(relative_path("A\\B\\C.TXT").unwrap(), "A/B/C.TXT");
    assert_eq!(relative_path("caf\u{e9}.txt").unwrap(), "caf\u{e9}.txt");
    for bad in [
        "", "/A", "A/../B", "A//B", "A:", "NUL", "com1.txt", "A.", "A ", "a\u{1}",
    ] {
        assert!(relative_path(bad).is_err(), "{:?}", bad);
    }
}

#[test]
fn reads_a_set_from_a_host_folder() {
    let dir = std::env::temp_dir().join(format!("rust-dos-installshield-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let files = nested_files();
    for (name, bytes) in write::cabinet(5, &files, &CabinetOptions::default()) {
        std::fs::write(dir.join(name.to_uppercase()), bytes).unwrap();
    }
    assert_eq!(sniff(&dir.join("DATA1.HDR")).unwrap(), Some(Kind::Cabinet));
    let cab = Cabinet::open_path(
        &dir.join("DATA1.HDR"),
        Compression::Chunks,
        &Limits::default(),
    )
    .unwrap();
    assert_eq!(cab.read("README.TXT").unwrap(), b"Read me first.");
    std::fs::write(
        dir.join("_SETUP.1"),
        write::archive(&archive_files(), 0, 1, 1),
    )
    .unwrap();
    assert_eq!(sniff(&dir.join("_SETUP.1")).unwrap(), Some(Kind::Archive));
    let archive = Archive::open_path(&dir.join("_setup.1"), &Limits::default());
    // The host's names may be case-sensitive: open by the name it has.
    let archive = archive
        .or_else(|_| Archive::open_path(&dir.join("_SETUP.1"), &Limits::default()))
        .unwrap();
    assert_eq!(archive.members().len(), 4);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn lists_and_expands_an_engine_archive() {
    let engine = sample(5000, 7);
    let helper = sample(300, 9);
    let bytes = write::engine_archive(&[
        ("INSTALL.EXE", "_INS0432._MP", &engine),
        ("zdata.dll", "zdata.dll", &helper),
        ("BOOT16.EXE", "_INJ0432._MP", b"MZ"),
    ]);
    assert!(is_engine_archive(&bytes));
    let archive = EngineArchive::parse(&bytes).unwrap();
    let names: Vec<(&str, &str)> = archive
        .files()
        .iter()
        .map(|f| (f.name.as_str(), f.extract_as.as_str()))
        .collect();
    assert_eq!(
        names,
        [
            ("INSTALL.EXE", "_INS0432._MP"),
            ("zdata.dll", "zdata.dll"),
            ("BOOT16.EXE", "_INJ0432._MP")
        ]
    );
    let files = archive.files();
    assert_eq!(archive.read(&bytes, &files[0]).unwrap(), engine);
    assert_eq!(archive.read(&bytes, &files[1]).unwrap(), helper);
    assert_eq!(files[2].size(), 2);
}

#[test]
fn refuses_damaged_engine_archives() {
    let bytes = write::engine_archive(&[("INSTALL.EXE", "_INS0432._MP", b"engine")]);
    // Cut short anywhere: it doesn't open, or its file doesn't read.
    for len in 0..bytes.len() {
        let cut = &bytes[..len];
        if let Ok(archive) = EngineArchive::parse(cut) {
            assert!(archive.read(cut, &archive.files()[0]).is_err());
        }
    }
    // A name with a folder in it.
    let bad = write::engine_archive(&[("INSTALL.EXE", r"..\_INS0432._MP", b"engine")]);
    assert!(EngineArchive::parse(&bad).is_err());
    // Too many files.
    let mut many = bytes.clone();
    many[0x4E] = 200;
    assert!(EngineArchive::parse(&many).is_err());
    assert!(!is_engine_archive(b"MZ\x90\x00"));
}
