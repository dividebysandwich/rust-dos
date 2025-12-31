use rust_dos::cpu::Cpu;
use std::fs;
use std::path::PathBuf;

#[test]
fn test_load_executable_in_subdirectory() {
    // Setup temporary test directory structure
    let root_path = PathBuf::from("target/test_load_exe_root");
    let sub_path = root_path.join("SUB");

    // Clean up previous run
    if root_path.exists() {
        fs::remove_dir_all(&root_path).unwrap();
    }

    fs::create_dir_all(&sub_path).unwrap();

    // Create a dummy COM file in the subdirectory
    let com_path = sub_path.join("TEST.COM");
    // Simple infinite loop: EB FE
    fs::write(&com_path, vec![0xEB, 0xFE]).unwrap();

    // Initialize CPU with the root path
    let mut cpu = Cpu::new(root_path.clone());

    // Verify file is NOT found initially (since we are in root)
    let loaded = cpu.load_executable("TEST.COM");
    assert!(!loaded, "Should not find TEST.COM in root");

    // Change directory to "SUB"
    let cd_success = cpu.bus.disk.set_current_directory("SUB");
    assert!(cd_success, "Failed to change directory to SUB");

    // Verify CWD is correct
    assert_eq!(cpu.bus.disk.get_current_directory(), "SUB");

    // Try to load "TEST.COM" again - should succeed now
    let loaded_now = cpu.load_executable("TEST.COM");
    assert!(loaded_now, "Should find TEST.COM in SUB");

    // Verify it loaded as a COM file
    // CS should be 0x1000, IP should be 0x100
    assert_eq!(cpu.cs, 0x1000);
    assert_eq!(cpu.ip, 0x100);

    // Cleanup
    fs::remove_dir_all(&root_path).unwrap();
}
