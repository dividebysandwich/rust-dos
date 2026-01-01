use rust_dos::bus::Bus;
use rust_dos::video::{ADDR_VGA_GRAPHICS, ADDR_VGA_TEXT};

#[test]
fn test_ram_access() {
    let mut bus = Bus::new(std::path::PathBuf::from("."));

    // Write to generic RAM (e.g., address 0x1000)
    bus.write_8(0x1000, 0xAA);
    assert_eq!(bus.read_8(0x1000), 0xAA);

    // Test persistence
    bus.write_8(0x1000, 0xBB);
    assert_eq!(bus.read_8(0x1000), 0xBB);
}

#[test]
fn test_vram_mapping() {
    let mut bus = Bus::new(std::path::PathBuf::from("."));

    // Test Text Mode VRAM (0xB8000)
    let text_addr = ADDR_VGA_TEXT; // 0xB8000
    bus.write_8(text_addr, 0x41); // 'A'

    // Verify it landed in the dedicated vram_text vector
    assert_eq!(bus.vga.vram_text[0], 0x41);
    // Verify read_8 maps correctly
    assert_eq!(bus.read_8(text_addr), 0x41);
    // Verify it DID NOT go to RAM or Graphics VRAM
    assert_eq!(bus.ram[text_addr], 0x00);
    assert_eq!(bus.vga.read_graphics(0), 0x00);

    // Test Graphics Mode VRAM (0xA0000)
    let graph_addr = ADDR_VGA_GRAPHICS; // 0xA0000
    // Set Mode 13h so write_8 works. We must use the helper to set Registers (Chain 4, etc.)
    bus.vga
        .set_video_mode(rust_dos::video::VideoMode::Graphics320x200);
    bus.video_mode = rust_dos::video::VideoMode::Graphics320x200;

    // Bus::write_8 checks vga.ports() to decide where to route IO, but here we do MEM write.
    // MEM write goes to vga.write_graphics.
    // write_graphics checks Chain4 bit. set_video_mode sets it.

    bus.write_8(graph_addr, 0xFF);

    assert_eq!(bus.vga.read_graphics(0), 0xFF);
    assert_eq!(bus.read_8(graph_addr), 0xFF);
    assert_eq!(bus.ram[graph_addr], 0x00);
}

#[test]
fn test_little_endian_read_write() {
    let mut bus = Bus::new(std::path::PathBuf::from("."));
    let addr = 0x2000;

    // Write 32-bit value: 0x12345678
    // Memory layout should be: 78 56 34 12
    bus.write_32(addr, 0x12345678);

    assert_eq!(bus.read_8(addr), 0x78);
    assert_eq!(bus.read_8(addr + 1), 0x56);
    assert_eq!(bus.read_8(addr + 2), 0x34);
    assert_eq!(bus.read_8(addr + 3), 0x12);

    assert_eq!(bus.read_16(addr), 0x5678);
    assert_eq!(bus.read_32(addr), 0x12345678);
}

#[test]
fn test_pit_channel_2_latch_logic() {
    let mut bus = Bus::new(std::path::PathBuf::from("."));

    // PIT Channel 2 (Speaker) uses port 0x42
    // It has a MSB/LSB flip-flop.

    // 1. Initialize Divisor to known state (0xFFFF)
    bus.pit_divisor = 0xFFFF;
    bus.pit_write_msb = false; // Expecting LSB next

    // 2. Write LSB (0x12)
    bus.io_write(0x42, 0x12);
    // Divisor should now be 0xFF12 (LSB changed, MSB kept from init)
    assert_eq!(bus.pit_divisor, 0xFF12);
    // Toggle should have flipped
    assert_eq!(bus.pit_write_msb, true);

    // 3. Write MSB (0x34)
    bus.io_write(0x42, 0x34);
    // Divisor should now be 0x3412
    assert_eq!(bus.pit_divisor, 0x3412);
    // Toggle should have flipped back
    assert_eq!(bus.pit_write_msb, false);
}

#[test]
fn test_pit_channel_0_reset_bug() {
    let mut bus = Bus::new(std::path::PathBuf::from("."));

    // --------------------------------------------------------
    // This test targets the bug in IO Port 0x43 (Command Reg).
    // Failing to reset Channel 0's latch causes timing issues.
    // --------------------------------------------------------

    // 1. Put Channel 0 into "MSB expected" state
    bus.pit0_divisor = 0xFFFF;
    bus.pit0_write_msb = false;

    bus.io_write(0x40, 0xAA); // Write LSB
    assert!(
        bus.pit0_write_msb,
        "PIT0 toggle should be TRUE (expecting MSB)"
    );

    // 2. Send Command 0x36 to Port 0x43
    // Binary: 00 11 01 10
    //         ^^ Channel 0
    //            ^^ Access Mode 11 (Lo/Hi Byte)
    //               ^^ Mode 3
    //                  ^^ Binary
    // THIS MUST RESET THE LATCH TO EXPECT LSB (False)
    bus.io_write(0x43, 0x36);

    // 3. Verify Latch Reset
    // If bug exists: This remains TRUE
    // If fixed: This becomes FALSE
    assert_eq!(
        bus.pit0_write_msb, false,
        "PIT Channel 0 latch failed to reset after Command 0x36!"
    );

    // 4. Verify functionality matches expectation
    // We write 0xBB.
    // If reset worked (state=LSB), result is 0xFFBB.
    // If reset failed (state=MSB), result is 0xBBAA.
    bus.io_write(0x40, 0xBB);

    if bus.pit0_divisor == 0xBBAA {
        panic!("PIT Channel 0 Bug confirmed: Wrote MSB (0xBBAA) instead of LSB (0xFFBB)");
    }

    assert_eq!(bus.pit0_divisor, 0xFFBB);
}

#[test]
fn test_speaker_io_port_61() {
    let mut bus = Bus::new(std::path::PathBuf::from("."));

    // Port 0x61 controls speaker.
    // Bit 0: Gate 2
    // Bit 1: Data
    // Both must be 1 for speaker_on to be true.

    // 1. Write 0x00 (Both off)
    bus.io_write(0x61, 0x00);
    assert_eq!(bus.speaker_on, false);
    assert_eq!(bus.io_read(0x61), 0x00);

    // 2. Write 0x03 (Both on)
    bus.io_write(0x61, 0x03);
    assert_eq!(bus.speaker_on, true);
    // Reading 0x61 should reflect the state (masked)
    assert_eq!(bus.io_read(0x61) & 0x03, 0x03);

    // 3. Write 0x02 (Bit 0 off)
    bus.io_write(0x61, 0x02);
    assert_eq!(bus.speaker_on, false);
}
