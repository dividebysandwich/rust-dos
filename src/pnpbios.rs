//! The Plug and Play BIOS (PnP BIOS specification 1.0A): the `$PnP`
//! installation structure in the ROM, and the entry point that operating
//! systems call, in real mode and in 16-bit protected mode, to list the
//! system board's devices and their resources.
//!
//! The devices are DOSBox-X's (bios.cpp, `ISAPNP_sysdev_*`), with the same
//! handles, for the Windows 95 installed on it: Windows knows a device by
//! its handle (Enum\BIOS\*PNP0303\00), and one it finds under another
//! handle, or a `$PnP` at another address, it installs again. The PCI bus,
//! serial ports and IDE controllers DOSBox-X had are left out, as rust-dos
//! has none, so the handles skip theirs.

use crate::bus::Bus;
use crate::cpu::{Cpu, Seg};

/// Where the installation structure is: F000:FEB0, where DOSBox-X had it
/// and Windows remembers it.
const HEADER: usize = 0xFFEB0;
/// The entry point, real and protected mode: a far-call service, then
/// RETF (`bios::install`).
pub const ENTRY: u16 = 0x1300;

/// Return codes.
const SUCCESS: u16 = 0x00;
const FUNCTION_NOT_SUPPORTED: u16 = 0x82;
const BAD_PARAMETER: u16 = 0x84;

/// A compressed EISA ID ("PNP0303").
fn eisa_id(id: &[u8; 7]) -> [u8; 4] {
    let letter = |c: u8| (c - b'@') as u32 & 0x1F;
    let hex = |c: u8| (if c <= b'9' { c - b'0' } else { c - b'A' + 10 }) as u32;
    let value = letter(id[0]) << 2
        | letter(id[1]) >> 3
        | (letter(id[1]) & 7) << 13
        | letter(id[2]) << 8
        | hex(id[3]) << 20
        | hex(id[4]) << 16
        | hex(id[5]) << 28
        | hex(id[6]) << 24;
    value.to_le_bytes()
}

/// A device node's resources as the ISA PnP resource data has them.
enum Resource {
    /// I/O ports: 16-bit decode, first port, alignment and length.
    Io(u16, u8, u8),
    Irq(u8),
    Dma(u8),
    /// Memory: base and length.
    Memory(u32, u32),
}

/// A system device node's data after its size and handle: the EISA ID,
/// type code, attributes (can't be disabled or configured) and resource
/// lists, allocated, possible (none) and compatible (none).
fn node(id: &[u8; 7], typ: [u8; 3], resources: &[Resource]) -> Vec<u8> {
    let mut data = eisa_id(id).to_vec();
    data.extend(typ);
    data.extend(0x0003u16.to_le_bytes());
    for resource in resources {
        match *resource {
            Resource::Io(port, align, len) => {
                let [lo, hi] = port.to_le_bytes();
                data.extend([0x47, 0x01, lo, hi, lo, hi, align, len]);
            }
            Resource::Irq(irq) => {
                let [lo, hi] = (1u16 << irq).to_le_bytes();
                data.extend([0x23, lo, hi, 0x09]);
            }
            Resource::Dma(dma) => data.extend([0x2A, 1 << dma, 0x01]),
            Resource::Memory(base, len) => {
                data.extend([0x86, 9, 0, 0x01]);
                data.extend(base.to_le_bytes());
                data.extend(len.to_le_bytes());
            }
        }
    }
    // End tags of the allocated, possible and compatible lists.
    data.extend([0x79, 0x00, 0x79, 0x00, 0x79, 0x00]);
    data
}

/// The system devices, by handle.
fn nodes(bus: &Bus) -> Vec<(u8, Vec<u8>)> {
    use Resource::*;
    let memory = bus.ram().len() as u32;
    let mut ram = vec![Memory(0, 0xA0000)];
    if memory > 0x10_0000 {
        ram.push(Memory(0x10_0000, memory - 0x10_0000));
    }
    vec![
        (0x00, node(b"PNP0303", [0x09, 0x00, 0x00], &[Io(0x60, 1, 1), Io(0x64, 1, 1), Irq(1)])),
        (0x01, node(b"PNP0F0E", [0x09, 0x02, 0x00], &[Irq(12)])),
        (0x02, node(b"PNP0200", [0x08, 0x01, 0x00], &[Io(0x00, 0x10, 0x10), Io(0x81, 1, 0x0F), Io(0xC0, 0x20, 0x20), Dma(4)])),
        (0x03, node(b"PNP0000", [0x08, 0x00, 0x01], &[Io(0x20, 1, 2), Io(0xA0, 1, 2), Irq(2)])),
        (0x04, node(b"PNP0100", [0x08, 0x02, 0x01], &[Io(0x40, 4, 4), Irq(0)])),
        (0x05, node(b"PNP0B00", [0x08, 0x03, 0x01], &[Io(0x70, 1, 2), Irq(8)])),
        (0x06, node(b"PNP0800", [0x04, 0x01, 0x00], &[Io(0x61, 1, 1)])),
        (0x07, node(b"PNP0C01", [0x08, 0x80, 0x00], &[Io(0x24, 4, 4)])),
        (0x08, node(b"PNP0C02", [0x08, 0x80, 0x00], &[Io(0x208, 4, 4)])),
        (0x09, node(b"PNP0A00", [0x06, 0x04, 0x00], &[])),
        (0x0B, node(b"PNP0C04", [0x0B, 0x80, 0x00], &[Io(0xF0, 0x10, 0x10), Irq(13)])),
        (0x0C, node(b"PNP0C01", [0x05, 0x00, 0x00], &ram)),
    ]
}

/// Put the installation structure in the ROM: signature, version 1.0,
/// length, no event notification, the entry point for real mode and for
/// protected mode (at F0000h), an OEM ID, the data segment, the checksum.
pub fn install(bus: &mut Bus) {
    let mut header = [0u8; 0x21];
    header[0..4].copy_from_slice(b"$PnP");
    header[4] = 0x10;
    header[5] = 0x21;
    header[0x0D..0x11].copy_from_slice(&(0xF000u32 << 16 | ENTRY as u32).to_le_bytes());
    header[0x11..0x13].copy_from_slice(&ENTRY.to_le_bytes());
    header[0x13..0x17].copy_from_slice(&0xF0000u32.to_le_bytes());
    header[0x17..0x1B].copy_from_slice(&eisa_id(b"RDS0100"));
    header[0x1B..0x1D].copy_from_slice(&0xF000u16.to_le_bytes());
    header[0x1D..0x21].copy_from_slice(&0xF0000u32.to_le_bytes());
    let sum = header.iter().fold(0u8, |sum, &b| sum.wrapping_add(b));
    header[8] = sum.wrapping_neg();
    bus.write_rom(HEADER, &header);
}

/// The far address of the installation structure, for ES:DI at boot.
pub fn header_pointer() -> (u16, u16) {
    (0xF000, (HEADER - 0xF0000) as u16)
}

/// The linear address a far pointer (segment:offset in real and V86
/// mode, a GDT or LDT selector in protected mode) points to.
fn far_pointer(cpu: &mut Cpu, pointer: u32) -> u32 {
    let (selector, offset) = ((pointer >> 16) as u16, pointer & 0xFFFF);
    if !cpu.pm() {
        return cpu.real_linear(selector, offset as u16);
    }
    cpu.fetch_descriptor(selector, 0).map_or(0, |desc| desc.base().wrapping_add(offset))
}

/// The entry point (`ENTRY`), called with a function number and its
/// parameters on the stack as a C function's, returning a code in AX.
pub fn entry(cpu: &mut Cpu) {
    // The parameters after the far return address, on the stack, which
    // Windows 95 OSR2's has anywhere in its linear memory.
    let ss = cpu.seg_cache(Seg::SS);
    let sp = if ss.attr & 0x4000 != 0 { cpu.esp() } else { cpu.esp() & 0xFFFF };
    let args = ss.base.wrapping_add(sp).wrapping_add(4);
    let word = |cpu: &mut Cpu, at: u32| cpu.bus.guest_read_16(args + at);
    let dword = |cpu: &mut Cpu, at: u32| cpu.bus.guest_read_32(args + at);
    let function = word(cpu, 0);
    let nodes = nodes(&cpu.bus);
    let result = match function {
        // Get Number of System Device Nodes: the count and the largest
        // node's size.
        0x00 => {
            let (count, size) = (dword(cpu, 2), dword(cpu, 6));
            let largest = nodes.iter().map(|(_, data)| data.len() + 3).max().unwrap_or(0);
            if count != 0 {
                let at = far_pointer(cpu, count);
                cpu.bus.guest_write_8(at, nodes.len() as u8);
            }
            if size != 0 {
                let at = far_pointer(cpu, size);
                cpu.bus.guest_write_16(at, largest as u16);
            }
            SUCCESS
        }
        // Get System Device Node: the node whose handle the byte at Node
        // has, into the buffer, and the next one's handle (FFh after the
        // last) into Node. Control asks for the current (1) or the next
        // boot's (2) configuration, which are the same.
        0x01 => {
            let (node_ptr, buffer, control) = (dword(cpu, 2), dword(cpu, 6), word(cpu, 10));
            let (node_at, buffer_at) = (far_pointer(cpu, node_ptr), far_pointer(cpu, buffer));
            let handle = cpu.bus.guest_read_8(node_at);
            match nodes.iter().position(|(h, _)| *h == handle) {
                Some(i) if control & 3 == 1 || control & 3 == 2 => {
                    let data = &nodes[i].1;
                    cpu.bus.guest_write_16(buffer_at, (data.len() + 3) as u16);
                    cpu.bus.guest_write_8(buffer_at + 2, handle);
                    cpu.bus.guest_write_bytes(buffer_at + 3, data);
                    let next = nodes.get(i + 1).map_or(0xFF, |(h, _)| *h);
                    cpu.bus.guest_write_8(node_at, next);
                    SUCCESS
                }
                _ => BAD_PARAMETER,
            }
        }
        // Send Message: power off, and the system saying it runs or no
        // longer does.
        0x04 => match word(cpu, 2) {
            0x41 => {
                crate::boot::power_off(cpu, "the system asked the Plug and Play BIOS");
                SUCCESS
            }
            0x42 | 0x43 => SUCCESS,
            _ => FUNCTION_NOT_SUPPORTED,
        },
        // Get Plug and Play ISA Configuration Structure: revision 1, no
        // ISA PnP cards and no read port.
        0x40 => {
            let pointer = dword(cpu, 2);
            if pointer != 0 {
                let at = far_pointer(cpu, pointer);
                cpu.bus.guest_write_bytes(at, &[0x01, 0x00, 0x00, 0x00, 0x00, 0x00]);
            }
            SUCCESS
        }
        _ => FUNCTION_NOT_SUPPORTED,
    };
    if cpu.bus.guest_faulted() {
        return;
    }
    cpu.set_ax(result);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Call the entry point in real mode with `args` (words, the function
    /// first) on the stack at 0:8000 behind a far return address.
    fn call(cpu: &mut Cpu, args: &[u16]) -> u16 {
        cpu.set_ss(0);
        cpu.set_esp(0x8000);
        cpu.bus.write_32(0x8000, 0);
        for (i, &arg) in args.iter().enumerate() {
            cpu.bus.write_16(0x8004 + 2 * i, arg);
        }
        entry(cpu);
        cpu.ax()
    }

    #[test]
    fn the_installation_structure_checks_out() {
        let cpu = Cpu::new(std::path::PathBuf::from("."));
        let header: Vec<u8> = (0..0x21).map(|i| cpu.bus.read_8(HEADER + i)).collect();
        assert_eq!(&header[0..4], b"$PnP");
        assert_eq!(header.iter().fold(0u8, |sum, &b| sum.wrapping_add(b)), 0);
        assert_eq!(u32::from_le_bytes(header[0x0D..0x11].try_into().unwrap()), 0xF000_1300);
        assert_eq!(cpu.bus.read_32(0xF1300), 0xCB01_3AFE, "the service and RETF");
    }

    #[test]
    fn the_nodes_come_in_a_chain_of_handles() {
        let mut cpu = Cpu::new(std::path::PathBuf::from("."));
        // Get Number of System Device Nodes into 0:0600 and 0:0602.
        assert_eq!(call(&mut cpu, &[0x00, 0x0600, 0, 0x0602, 0, 0xF000]), 0);
        assert_eq!(cpu.bus.read_8(0x0600), 12);
        let largest = cpu.bus.read_16(0x0602);
        // Walk the chain from handle 0: each node into 0:1000.
        let mut handles = Vec::new();
        cpu.bus.write_8(0x0604, 0);
        while cpu.bus.read_8(0x0604) != 0xFF {
            let handle = cpu.bus.read_8(0x0604);
            assert_eq!(call(&mut cpu, &[0x01, 0x0604, 0, 0x1000, 0, 1, 0xF000]), 0);
            assert_eq!(cpu.bus.read_8(0x1002), handle);
            assert!(cpu.bus.read_16(0x1000) <= largest);
            handles.push(handle);
        }
        assert_eq!(handles, [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 0x0B, 0x0C]);
        // The keyboard: PNP0303 with ports 60h and 64h and IRQ 1.
        cpu.bus.write_8(0x0604, 0);
        call(&mut cpu, &[0x01, 0x0604, 0, 0x1000, 0, 1, 0xF000]);
        let node: Vec<u8> = (0..cpu.bus.read_16(0x1000) as usize).map(|i| cpu.bus.read_8(0x1000 + i)).collect();
        assert_eq!(&node[3..7], &[0x41, 0xD0, 0x03, 0x03]);
        assert_eq!(&node[12..20], &[0x47, 0x01, 0x60, 0x00, 0x60, 0x00, 0x01, 0x01]);
        assert_eq!(&node[28..32], &[0x23, 0x02, 0x00, 0x09]);
        // A handle there is none of, and a bad control value.
        cpu.bus.write_8(0x0604, 0x0A);
        assert_eq!(call(&mut cpu, &[0x01, 0x0604, 0, 0x1000, 0, 1, 0xF000]), BAD_PARAMETER);
        cpu.bus.write_8(0x0604, 0);
        assert_eq!(call(&mut cpu, &[0x01, 0x0604, 0, 0x1000, 0, 3, 0xF000]), BAD_PARAMETER);
        assert_eq!(call(&mut cpu, &[0x03]), FUNCTION_NOT_SUPPORTED);
    }

    #[test]
    fn eisa_ids_are_compressed_as_the_specification_has_them() {
        // "PNP0303" is 41D00303h, stored little-endian.
        assert_eq!(eisa_id(b"PNP0303"), [0x41, 0xD0, 0x03, 0x03]);
        assert_eq!(eisa_id(b"PNP0F0E"), [0x41, 0xD0, 0x0F, 0x0E]);
    }
}
