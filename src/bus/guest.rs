//! Memory as the BIOS's services reach it: by the linear address their
//! caller's segment and offset make (`Cpu::real_linear`), through the A20
//! gate, as a BIOS's own code would read and write it.

use super::Bus;

impl Bus {
    /// The physical address of the linear address `lin` of a BIOS service.
    fn guest_physical(&self, lin: u32) -> usize {
        (lin & self.a20_mask) as usize
    }

    pub fn guest_read_8(&mut self, lin: u32) -> u8 {
        self.read_8(self.guest_physical(lin))
    }

    pub fn guest_read_16(&mut self, lin: u32) -> u16 {
        u16::from_le_bytes([self.guest_read_8(lin), self.guest_read_8(lin.wrapping_add(1))])
    }

    pub fn guest_read_32(&mut self, lin: u32) -> u32 {
        self.guest_read_16(lin) as u32 | (self.guest_read_16(lin.wrapping_add(2)) as u32) << 16
    }

    pub fn guest_write_8(&mut self, lin: u32, value: u8) {
        let addr = self.guest_physical(lin);
        self.write_8(addr, value);
    }

    pub fn guest_write_16(&mut self, lin: u32, value: u16) {
        let [lo, hi] = value.to_le_bytes();
        self.guest_write_8(lin, lo);
        self.guest_write_8(lin.wrapping_add(1), hi);
    }

    pub fn guest_write_32(&mut self, lin: u32, value: u32) {
        self.guest_write_16(lin, value as u16);
        self.guest_write_16(lin.wrapping_add(2), (value >> 16) as u16);
    }

    /// Fill `data` from the memory at `lin`.
    pub fn guest_read_bytes(&mut self, lin: u32, data: &mut [u8]) {
        for (i, byte) in data.iter_mut().enumerate() {
            *byte = self.guest_read_8(lin.wrapping_add(i as u32));
        }
    }

    /// Write `data` to the memory at `lin`.
    pub fn guest_write_bytes(&mut self, lin: u32, data: &[u8]) {
        for (i, &byte) in data.iter().enumerate() {
            self.guest_write_8(lin.wrapping_add(i as u32), byte);
        }
    }
}
