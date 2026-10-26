use super::*;

#[test]
fn a_dos_program_is_no_pe_file() {
    assert!(!is_pe(b"MZ\x90\x00"));
    assert!(!is_pe(b"\xB4\x4C\xCD\x21"));
    let mut file = vec![0u8; 0x100];
    file[..2].copy_from_slice(b"MZ");
    file[0x3C] = 0x80;
    assert!(!is_pe(&file));
    file[0x80..0x84].copy_from_slice(b"PE\0\0");
    assert!(is_pe(&file));
    // e_lfanew past the end of the file.
    file[0x3C..0x40].copy_from_slice(&0x1_0000u32.to_le_bytes());
    assert!(!is_pe(&file));
}

#[test]
fn a_built_program_has_its_headers_sections_and_imports() {
    let mut b = Builder::exe(0x40_0000);
    let iat = b.imports(&[
        (
            "KERNEL32.dll",
            &[Imp::Name("GetStdHandle"), Imp::Name("ExitProcess")],
        ),
        ("WSOCK32.dll", &[Imp::Ordinal(23)]),
    ]);
    let code = b.next_rva();
    b.section(".text", &[0xC3; 16], SCN_CODE | SCN_EXECUTE | SCN_READ);
    b.section_sized(
        ".bss",
        &[],
        0x2345,
        SCN_UNINITIALIZED_DATA | SCN_READ | SCN_WRITE,
    );
    b.entry(code);
    let file = b.build();
    assert!(is_pe(&file));

    let image = Image::load(&file).unwrap();
    let h = &image.headers;
    assert_eq!(
        (h.machine, h.image_base, h.entry),
        (MACHINE_I386, 0x40_0000, code)
    );
    assert_eq!(h.subsystem, SUBSYSTEM_CONSOLE);
    assert!(!h.is_dll());
    assert!(!h.relocatable());
    let names: Vec<&str> = h.sections.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names, [".idata", ".text", ".bss"]);
    assert_eq!(h.size_of_image, 0x3000 + 0x3000);
    assert_eq!(h.section_at(code + 4).unwrap().name, ".text");
    assert_eq!(image.data[code as usize..code as usize + 16], [0xC3; 16]);

    let imports = image.imports().unwrap();
    assert_eq!(imports.len(), 2);
    assert_eq!(imports[0].name, "KERNEL32.dll");
    let functions: Vec<&Import> = imports[0].functions.iter().map(|(_, f)| f).collect();
    assert_eq!(
        functions,
        [
            &Import::Name {
                hint: 0,
                name: "GetStdHandle".into()
            },
            &Import::Name {
                hint: 0,
                name: "ExitProcess".into()
            }
        ]
    );
    assert_eq!(
        imports[1].functions,
        [(iat[1][0] - 0x40_0000, Import::Ordinal(23))]
    );
    // The slots are where the builder said the code finds them.
    for (dll, vas) in imports.iter().zip(&iat) {
        let slots: Vec<u32> = dll
            .functions
            .iter()
            .map(|(rva, _)| rva + 0x40_0000)
            .collect();
        assert_eq!(&slots, vas);
    }
}

#[test]
fn the_dos_stub_says_the_program_needs_windows() {
    let file = Builder::exe(0x40_0000).build();
    let header = u16_at(&file, 8).unwrap() as usize * 16;
    let stub = &file[header..];
    // MOV DX,000Eh points at the message in the load module.
    assert_eq!(&stub[..5], &[0x0E, 0x1F, 0xBA, 0x0E, 0x00]);
    assert!(stub[0x0E..].starts_with(b"This program cannot be run in DOS mode."));
}

#[test]
fn a_dll_exports_by_name_and_ordinal() {
    let mut b = Builder::dll(0x1000_0000, "TEST.DLL");
    let code = b.section(".text", &[0x90; 32], SCN_CODE | SCN_EXECUTE | SCN_READ);
    b.export(Some("Zeta"), 3, code)
        .export(Some("Alpha"), 1, code + 8)
        .export(None, 5, code + 16);
    let image = Image::load(&b.build()).unwrap();
    assert!(image.headers.is_dll());
    let exports = image.exports().unwrap().unwrap();
    assert_eq!(exports.name, "TEST.DLL");
    assert_eq!(exports.ordinal_base, 1);
    assert_eq!(exports.exports.len(), 3);
    assert_eq!(
        exports.by_name("Alpha").unwrap().target,
        ExportTarget::Rva(code + 8)
    );
    assert_eq!(exports.by_name("Zeta").unwrap().ordinal, 3);
    assert_eq!(exports.by_ordinal(5).unwrap().name, None);
    assert!(exports.by_ordinal(2).is_none());
}

#[test]
fn a_forwarded_export_names_its_target() {
    // A forwarder is an export whose RVA lies in the export directory: a
    // string there. Build one by hand inside the directory's section.
    let mut b = Builder::dll(0x1000_0000, "FWD.DLL");
    let code = b.section(".text", &[0x90; 4], SCN_CODE | SCN_EXECUTE | SCN_READ);
    b.export(Some("Here"), 1, code);
    let mut image = Image::load(&b.build()).unwrap();
    let dir = image.headers.dirs[DIR_EXPORT];
    // Put "KERNEL32.Sleep" into the directory's slack and point at it.
    let text = b"KERNEL32.Sleep\0";
    let at = dir.rva + dir.size;
    image.data[at as usize..at as usize + text.len()].copy_from_slice(text);
    image.headers.dirs[DIR_EXPORT].size += text.len() as u32;
    let functions = image.u32_at(dir.rva + 28).unwrap();
    image.set_u32(functions, at).unwrap();
    let exports = image.exports().unwrap().unwrap();
    assert_eq!(
        exports.by_name("Here").unwrap().target,
        ExportTarget::Forwarder("KERNEL32.Sleep".into())
    );
}

#[test]
fn relocations_move_the_image() {
    let mut b = Builder::exe(0x40_0000);
    let data = b.next_rva();
    let mut bytes = Vec::new();
    // Two pointers into the image, on two pages.
    bytes.extend_from_slice(&(0x40_0000 + data + 8).to_le_bytes());
    bytes.resize(0x1004, 0);
    bytes.extend_from_slice(&0x40_1234u32.to_le_bytes());
    b.section(".data", &bytes, SCN_INITIALIZED_DATA | SCN_READ | SCN_WRITE);
    b.reloc(data).reloc(data + 0x1004);
    let mut image = Image::load(&b.build()).unwrap();
    assert!(image.headers.relocatable());
    let relocs = image.relocations().unwrap();
    assert_eq!(
        relocs,
        [
            Relocation {
                rva: data,
                kind: REL_HIGHLOW
            },
            Relocation {
                rva: data + 0x1004,
                kind: REL_HIGHLOW
            }
        ]
    );
    image.relocate(0x0080_0000).unwrap();
    assert_eq!(image.u32_at(data).unwrap(), 0x80_0000 + data + 8);
    assert_eq!(image.u32_at(data + 0x1004).unwrap(), 0x80_1234);
    assert_eq!(image.headers.image_base, 0x80_0000);
    assert_eq!(Headers::parse(&image.data).unwrap().image_base, 0x80_0000);
}

#[test]
fn the_tls_directory_has_the_template_and_callbacks() {
    let mut b = Builder::exe(0x40_0000);
    let rva = b.next_rva();
    let va = |offset: u32| 0x40_0000 + rva + offset;
    // The directory at 0, the template at 0x40, the index at 0x60, the
    // callbacks at 0x70.
    let mut data = vec![0u8; 0x80];
    for (at, value) in [
        (0, va(0x40)),
        (4, va(0x48)),
        (8, va(0x60)),
        (12, va(0x70)),
        (16, 8),
        (0x70, 0x40_1000),
    ] {
        data[at..at + 4].copy_from_slice(&value.to_le_bytes());
    }
    data[0x40..0x48].copy_from_slice(b"template");
    b.section(".tls", &data, SCN_INITIALIZED_DATA | SCN_READ | SCN_WRITE);
    b.directory(DIR_TLS, rva, 24);
    let image = Image::load(&b.build()).unwrap();
    let tls = image.tls().unwrap().unwrap();
    assert_eq!(tls.index_address, va(0x60));
    assert_eq!(tls.callbacks, [0x40_1000]);
    let template = image.tls_template(&tls).unwrap();
    assert_eq!(&template[..8], b"template");
    assert_eq!(template.len(), 16);
}

#[test]
fn resources_are_found_by_type_name_and_language() {
    let mut b = Builder::exe(0x40_0000);
    b.resource(6, 1, 0x409, b"strings")
        .resource(6, 2, 0x409, b"more")
        .resource(3, 1, 0, b"icon!");
    let image = Image::load(&b.build()).unwrap();
    let resources = image.resources().unwrap();
    assert_eq!(resources.len(), 3);
    let find = |kind, name| {
        resources
            .iter()
            .find(|r| r.kind == ResId::Id(kind) && r.name == ResId::Id(name))
            .unwrap()
    };
    let strings = find(6, 2);
    assert_eq!(strings.language, ResId::Id(0x409));
    let data = &image.data[strings.rva as usize..(strings.rva + strings.size) as usize];
    assert_eq!(data, b"more");
    let icon = find(3, 1);
    assert_eq!(
        &image.data[icon.rva as usize..(icon.rva + icon.size) as usize],
        b"icon!"
    );
}

#[test]
fn bad_images_are_refused() {
    let file = Builder::exe(0x40_0000).build();
    let pe = 0x80;
    let mut other = file.clone();
    other[pe + 4..pe + 6].copy_from_slice(&0x8664u16.to_le_bytes());
    assert!(matches!(Headers::parse(&other), Err(Error::Unsupported(_))));
    let mut unaligned = file.clone();
    unaligned[pe + 24 + 28..pe + 24 + 32].copy_from_slice(&0x40_1000u32.to_le_bytes());
    assert!(matches!(Headers::parse(&unaligned), Err(Error::Invalid(_))));
    assert!(matches!(
        Headers::parse(&file[..0x90]),
        Err(Error::Truncated(_))
    ));
    assert_eq!(Headers::parse(b"MZ"), Err(Error::NotPe));
}

#[test]
fn a_section_without_a_file_offset_is_zeros() {
    // Watcom's linker gives .bss a raw size with no data in the file:
    // offset 0. Windows' loader maps zeros there, not the file's start.
    let mut b = Builder::exe(0x40_0000);
    let code = b.next_rva();
    b.section(".text", &[0xC3; 16], SCN_CODE | SCN_EXECUTE | SCN_READ);
    let bss = b.section_sized(
        ".bss",
        &[],
        0x2000,
        SCN_UNINITIALIZED_DATA | SCN_READ | SCN_WRITE,
    );
    b.entry(code);
    let mut file = b.build();
    let header = section_header(&file, 1);
    assert_eq!(&file[header..header + 4], b".bss");
    file[header + 16..header + 20].copy_from_slice(&0x2000u32.to_le_bytes());
    file[header + 20..header + 24].copy_from_slice(&0u32.to_le_bytes());

    let image = Image::load(&file).unwrap();
    let at = bss as usize;
    assert!(image.data[at..at + 0x2000].iter().all(|&b| b == 0));
}

#[test]
fn raw_data_past_the_virtual_size_is_mapped_to_the_page_end() {
    // A patch can put code after the end the linker gave a section: in
    // its raw data, in the last page. Windows' loader maps it.
    let mut b = Builder::exe(0x40_0000);
    let code = b.next_rva();
    let mut text = vec![0xC3; 0x1000];
    text[0x800] = 0x90;
    b.section(".text", &text, SCN_CODE | SCN_EXECUTE | SCN_READ);
    b.entry(code);
    let mut file = b.build();
    let header = section_header(&file, 0);
    assert_eq!(&file[header..header + 5], b".text");
    file[header + 8..header + 12].copy_from_slice(&0x10u32.to_le_bytes());

    let image = Image::load(&file).unwrap();
    let at = code as usize;
    assert_eq!(image.data[at + 0x800], 0x90);
    assert_eq!(image.data[at + 0xFFF], 0xC3);
}

#[test]
fn raw_data_past_the_image_end_is_cut_short() {
    // An image size that ends in the last section's last page: the raw
    // data past it isn't mapped.
    let mut b = Builder::exe(0x40_0000);
    let code = b.next_rva();
    b.section(".text", &[0xC3; 0x1000], SCN_CODE | SCN_EXECUTE | SCN_READ);
    b.entry(code);
    let mut file = b.build();
    let header = section_header(&file, 0);
    file[header + 8..header + 12].copy_from_slice(&0x10u32.to_le_bytes());
    let size_of_image = pe_offset(&file).unwrap() + 24 + 56;
    file[size_of_image..size_of_image + 4].copy_from_slice(&(code + 0x10).to_le_bytes());

    let image = Image::load(&file).unwrap();
    assert_eq!(image.data.len(), code as usize + 0x10);
    assert_eq!(image.data[code as usize + 0xF], 0xC3);
}

/// Where the section header `index` of the PE file `file` is.
fn section_header(file: &[u8], index: usize) -> usize {
    let pe = pe_offset(file).unwrap();
    let optional = u16::from_le_bytes([file[pe + 20], file[pe + 21]]) as usize;
    pe + 24 + optional + 40 * index
}
