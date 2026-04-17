//! Sound Blaster DSP + DMA unit tests. These cover the parts games touch
//! during autodetect and the first PCM block — if any of these break, a
//! real SB driver will fail to bind to the card.

use rust_dos::sb::{Dma8237Ch1, SoundBlaster, DSP_VERSION_MAJOR, DSP_VERSION_MINOR};

#[test]
fn dsp_reset_handshake_pushes_0xaa() {
    let mut sb = SoundBlaster::new();
    sb.write_reset(1);
    sb.write_reset(0);
    // Driver reads 0x22E, sees bit 7, then reads 0x22A expecting 0xAA.
    assert_eq!(sb.read_buffer_status() & 0x80, 0x80);
    assert_eq!(sb.read_data(), 0xAA);
}

#[test]
fn dsp_version_returns_configured_sb2() {
    let mut sb = SoundBlaster::new();
    sb.write_command(0xE1);
    assert_eq!(sb.read_data(), DSP_VERSION_MAJOR);
    assert_eq!(sb.read_data(), DSP_VERSION_MINOR);
}

#[test]
fn dsp_e0_identifies_by_xor() {
    let mut sb = SoundBlaster::new();
    sb.write_command(0xE0);
    sb.write_command(0x55);
    // Echoes ~byte. Drivers compare the result to rule out bus noise.
    assert_eq!(sb.read_data(), !0x55);
}

#[test]
fn speaker_on_off_round_trips() {
    let mut sb = SoundBlaster::new();
    sb.write_command(0xD1);
    assert!(sb.speaker_on);
    sb.write_command(0xD8);
    assert_eq!(sb.read_data(), 0xFF);
    sb.write_command(0xD3);
    assert!(!sb.speaker_on);
}

#[test]
fn time_constant_sets_rate() {
    let mut sb = SoundBlaster::new();
    // TC 210 → 1e6 / (256-210) = ~21739 Hz, clamped.
    sb.write_command(0x40);
    sb.write_command(210);
    assert!(sb.sample_rate_hz >= 20_000 && sb.sample_rate_hz <= 22_500);
}

#[test]
fn test_register_round_trips() {
    let mut sb = SoundBlaster::new();
    sb.write_command(0xE4);
    sb.write_command(0x3C);
    sb.write_command(0xE8);
    assert_eq!(sb.read_data(), 0x3C);
}

#[test]
fn force_irq_sets_pending_cleared_by_status_read() {
    let mut sb = SoundBlaster::new();
    sb.write_command(0xF2);
    assert!(sb.irq_pending);
    let _ = sb.read_buffer_status();
    assert!(!sb.irq_pending);
}

#[test]
fn dma_addr_flipflop_lsb_then_msb() {
    let mut dma = Dma8237Ch1::default();
    dma.clear_flipflop();
    dma.write_addr(0x34); // LSB
    dma.write_addr(0x12); // MSB
    assert_eq!(dma.base_addr, 0x1234);
    assert_eq!(dma.cur_addr, 0x1234);
}

#[test]
fn single_cycle_dma_consumes_bytes_and_fires_irq() {
    // 4-byte 8-bit PCM transfer at 11025 Hz, host rate 44100.
    // After 4 pulls we should see IRQ pending and dma_active cleared.
    let mut ram = vec![0u8; 1024 * 1024];
    let base = 0x10000;
    ram[base..base + 4].copy_from_slice(&[0x80, 0xC0, 0x40, 0x00]);

    let mut sb = SoundBlaster::new();
    let mut dma = Dma8237Ch1::default();
    dma.page = 0x01; // page << 16 = 0x10000
    dma.base_addr = 0x0000;
    dma.cur_addr = 0x0000;
    dma.masked = false;

    // Set time constant → 11025 Hz (TC ≈ 165).
    sb.write_command(0x40);
    sb.write_command(165);
    sb.write_command(0xD1); // speaker on

    // 0x14 single-cycle, length = 3 (so 4 bytes get transferred).
    sb.write_command(0x14);
    sb.write_command(0x03);
    sb.write_command(0x00);

    // Pump enough host samples to exhaust the block. At ~11 kHz into
    // 44.1 kHz, 4 DMA pulls need ~16 host samples; a margin of 256 is
    // more than plenty without being slow.
    for _ in 0..256 {
        let _ = sb.advance_one(&ram, &mut dma, 44100);
    }

    assert!(sb.irq_pending, "DMA completion should raise IRQ 5");
    assert!(!sb.dma_active, "single-cycle DMA must clear dma_active at TC");
}

#[test]
fn auto_init_dma_reloads_on_terminal_count() {
    let mut ram = vec![0u8; 1024 * 1024];
    // 2-byte block at phys 0x00500.
    ram[0x00500] = 0xFF;
    ram[0x00501] = 0x00;

    let mut sb = SoundBlaster::new();
    let mut dma = Dma8237Ch1::default();
    dma.page = 0x00;
    dma.base_addr = 0x0500;
    dma.cur_addr = 0x0500;
    dma.masked = false;

    sb.write_command(0x40);
    sb.write_command(165);
    sb.write_command(0xD1);
    // Set block length = 2 via 0x48.
    sb.write_command(0x48);
    sb.write_command(0x01);
    sb.write_command(0x00);
    // Auto-init PCM out.
    sb.write_command(0x1C);

    // Drain enough samples to cross at least one block boundary.
    for _ in 0..512 {
        let _ = sb.advance_one(&ram, &mut dma, 44100);
    }

    assert!(sb.irq_pending, "auto-init DMA raises IRQ at each block end");
    assert!(sb.dma_active, "auto-init DMA must stay active across reload");
}

#[test]
fn direct_dac_sample_held_when_dma_inactive() {
    let ram = vec![0u8; 1024];
    let mut sb = SoundBlaster::new();
    let mut dma = Dma8237Ch1::default();
    sb.write_command(0x10);
    sb.write_command(0xC0); // ~0.5 scale, positive
    // advance_one with DMA inactive returns the latched sample.
    let s = sb.advance_one(&ram, &mut dma, 44100);
    assert!(s > 0, "direct-DAC write should produce a positive sample");
    // And should keep returning the same value.
    let s2 = sb.advance_one(&ram, &mut dma, 44100);
    assert_eq!(s, s2);
}
