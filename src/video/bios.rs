//! What the video card and its BIOS show programs looking for them: the
//! BIOS data area's fields about the adapter and the monitor, and the video
//! BIOS ROM at C000h.

use super::adapter::VideoSetup;
use crate::bus::Bus;

/// Put the adapter of `setup` in place: the card, and what the BIOS data
/// area and the ROM say about it. The video mode stays as it is; the
/// caller sets the one the adapter starts in.
pub fn install(bus: &mut Bus, setup: VideoSetup) {
    bus.vga.adapter = setup.adapter;

    // Equipment word bits 4-5: the initial video mode, 10 for 80x25 in
    // colour. The other bits are the floppies' and the coprocessor's.
    let equipment = bus.read_16(0x0410) & !0x0030;
    bus.write_16(0x0410, equipment | 0x0020);
    // The CRTC's address.
    bus.write_16(0x0463, 0x03D4);

    // 0487h: bits 5-6 the memory (11: 256 KB).
    bus.write_8(0x0487, 0x60);
    // 0488h: the EGA's switch settings and feature bits; 9 is an enhanced
    // colour display.
    bus.write_8(0x0488, 0x09);
    // 0489h: bit 0 the VGA is active; bits 7 and 4 the text modes'
    // scanlines (01: 400); bit 1 gray-scale summing, bit 2 a monochrome
    // monitor, bit 3 no palette loading.
    bus.write_8(0x0489, 0x11);
    // 048Ah: the index of the VGA's entry in the display combination code
    // table (INT 10h AH=1Ah returns the code itself).
    bus.write_8(0x048A, 0x0B);

    // The video BIOS ROM: its signature and size (64 blocks of 512 bytes),
    // and the name that programs look for.
    bus.load_bytes(0xC0000, &[0x55, 0xAA, 0x40]);
    bus.load_bytes(0xC001E, b"IBM VGA");
}
