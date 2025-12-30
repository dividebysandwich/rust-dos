use rust_dos::bus::Bus;
use std::path::PathBuf;

#[test]
fn test_root_dir_search() {
    // Initialize Bus with current directory as root
    let mut bus = Bus::new(PathBuf::from("."));

    // Test finding any file with *.* pattern
    // We know there are files in the root (e.g. Cargo.toml)
    let result = bus.disk.find_directory_entry("*.*", 0, 0x3F); // 0x3F matches everything

    match result {
        Ok(entry) => {
            println!("Found: {}", entry.filename);
            assert!(!entry.filename.is_empty());
        }
        Err(e) => {
            panic!(
                "Failed to find any file in root directory using *.*: Error code {}",
                e
            );
        }
    }
}

#[test]
fn test_specific_file_search_in_root() {
    let bus = Bus::new(PathBuf::from("."));

    // Cargo.toml -> CARGO.TOM (8.3 truncation)
    // We must search for the 8.3 name if we act like DOS.
    // Or does DOS strictly match long names? This emulator seems to strictly use generated short names.
    let result = bus.disk.find_directory_entry("CARGO.TOM", 0, 0x3F);

    match result {
        Ok(_) => {}
        Err(e) => panic!(
            "Should find CARGO.TOM in root, but got error code: 0x{:02X}",
            e
        ),
    }
}

#[test]
fn test_wildcard_question_mark_pattern() {
    let mut bus = Bus::new(PathBuf::from("."));

    // Create a dummy file or rely on existing ones. D.COM is short.
    // matches_pattern("D.COM", "????????.???") should be true

    // We want to find D.COM specifically, which is short.
    // If we only find EVILMAZE (8 chars), the bug is still present.
    // We'll traverse index until we find D.COM or exhaust.

    let mut found_d_com = false;
    let mut index = 0;

    // Use Attr 0x10 to find files/dirs (avoiding 0x08 VolLabel trap)
    while let Ok(entry) = bus.disk.find_directory_entry("????????.???", index, 0x10) {
        println!("Found: {}", entry.filename);
        if entry.filename == "D.COM" {
            found_d_com = true;
            break;
        }
        index += 1;
        if index > 100 {
            break;
        } // safety break
    }

    assert!(found_d_com, "Should find D.COM using ????????.??? pattern");
}

#[test]
fn test_dos_path_handling() {
    let mut bus = Bus::new(PathBuf::from("."));

    // Test specific DOS path style: C:\*.*
    // This previously failed because Path::new usage on Linux treated "C:\*.*" as a filename.
    // We use Attribute 0x10 (Directory + Files) to avoid matching only Volume Label (0x08)
    // which the current DiskController logic handles exclusively.
    let result = bus.disk.find_directory_entry(r"C:\*.*", 0, 0x10);
    match result {
        Ok(entry) => {
            println!("Found with C:\\*.*: {}", entry.filename);
            // Should verify it is NOT RUSTDOS
            assert_ne!(entry.filename, "RUSTDOS");
        }
        Err(e) => {
            panic!("Failed to find files using C:\\*.*: Error {}", e);
        }
    }
}
