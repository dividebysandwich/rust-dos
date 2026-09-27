//! The NE2000: Novell's ISA network card, a National DP8390 Ethernet
//! controller with 16 KB of buffer memory the driver reaches through the
//! card's data port (remote DMA), and the card's address in a PROM. Every
//! DOS packet driver, Windows for Workgroups, Windows 95 and most network
//! games' drivers know it. As in DOSBox Staging's (after Bochs's), its
//! frames go out as soon as the driver sends them, and it reports them sent
//! after the time 10 Mbit/s takes; frames come into its receive ring as the
//! ring has room for them, and wait while it hasn't.
//!
//! Its ports, from its base (300h unless set): 00h the command register,
//! 01h-0Fh the DP8390's registers in the page the command register selects,
//! 10h the data port (a byte or a word at a time, as the data configuration
//! register says), 1Fh the reset port.

use super::frame::{self, Mac};
use std::collections::VecDeque;

/// The card's buffer memory: 32 KB from 4000h, as the NE2000's 16-bit
/// mode has it (pages 40h-7Fh).
const MEM_START: usize = 16 * 1024;
const MEM_SIZE: usize = 32 * 1024;
const MEM_END: usize = MEM_START + MEM_SIZE;
const PAGE_FIRST: u8 = (MEM_START / 256) as u8;
const PAGE_LAST: u8 = (MEM_END / 256 - 1) as u8;
/// Frames waiting for room in the receive ring; more are dropped.
const WAITING: usize = 64;

/// Interrupt status (and mask) bits.
const ISR_PRX: u8 = 0x01;
const ISR_PTX: u8 = 0x02;
const ISR_RDC: u8 = 0x40;
const ISR_RST: u8 = 0x80;
/// Data configuration: word transfers, normal operation (not loopback).
const DCR_WTS: u8 = 0x01;
const DCR_LS: u8 = 0x08;
/// Receive configuration: runts, broadcasts, multicasts, everything.
const RCR_AR: u8 = 0x02;
const RCR_AB: u8 = 0x04;
const RCR_AM: u8 = 0x08;
const RCR_PRO: u8 = 0x10;

/// The card's ports, from its base.
pub const PORTS: u16 = 0x20;
pub const DATA: u16 = 0x10;
const RESET: u16 = 0x1F;

#[derive(Clone, Debug)]
pub struct Ne2000 {
    pub base: u16,
    pub irq: u8,
    pub mac: Mac,
    // The command register: stopped, started, the remote DMA command and
    // the register page.
    stop: bool,
    start: bool,
    rdma_cmd: u8,
    page: u8,
    isr: u8,
    imr: u8,
    dcr: u8,
    tcr: u8,
    tsr: u8,
    rcr: u8,
    rsr: u8,
    local_dma: u16,
    page_start: u8,
    page_stop: u8,
    boundary: u8,
    tx_page: u8,
    tx_bytes: u16,
    remote_dma: u16,
    remote_start: u16,
    remote_bytes: u16,
    /// The station address the receive filter takes (PAR0-5), which the
    /// driver copies from the PROM.
    phys: [u8; 6],
    curr: u8,
    mchash: [u8; 8],
    rempkt: u8,
    localpkt: u8,
    address_cnt: u16,
    mem: Vec<u8>,
    /// When the frame being sent is on the wire, in PIT ticks.
    tx_due: Option<u64>,
    /// Frames for the receive ring, waiting for room in it.
    waiting: VecDeque<Vec<u8>>,
    /// Frames the driver sent, for the bus to hand to the network.
    pub outgoing: Vec<Vec<u8>>,
    /// Whether the card holds its IRQ line up, as the bus last saw it.
    pub pic_line: bool,
    pub log: Vec<String>,
}

impl Default for Ne2000 {
    fn default() -> Self {
        Self::new(0x300, 10, Mac::default())
    }
}

crate::state_fields!(Ne2000 {
    base, irq, mac, stop, start, rdma_cmd, page, isr, imr, dcr, tcr, tsr, rcr, rsr, local_dma, page_start,
    page_stop, boundary, tx_page, tx_bytes, remote_dma, remote_start, remote_bytes, phys, curr, mchash, rempkt,
    localpkt, address_cnt, mem, tx_due, pic_line
} skip { waiting, outgoing, log });

/// The multicast hash filter's bit for `destination`: the top 6 bits of
/// the Ethernet CRC of the address, as the DP8390 computes it.
fn multicast_index(destination: &[u8]) -> usize {
    const POLYNOMIAL: u32 = 0x04C1_1DB6;
    let mut crc: u32 = 0xFFFF_FFFF;
    for &byte in &destination[..6] {
        let mut b = byte;
        for _ in 0..8 {
            let carry = ((crc >> 31) as u8 ^ (b & 1)) as u32;
            crc <<= 1;
            b >>= 1;
            if carry != 0 {
                crc = (crc ^ POLYNOMIAL) | carry;
            }
        }
    }
    (crc >> 26) as usize
}

impl Ne2000 {
    pub fn new(base: u16, irq: u8, mac: Mac) -> Self {
        let mut card = Self {
            base,
            irq,
            mac,
            stop: true,
            start: false,
            rdma_cmd: 4,
            page: 0,
            isr: ISR_RST,
            imr: 0,
            dcr: 0x04,
            tcr: 0,
            tsr: 0,
            rcr: 0,
            rsr: 0,
            local_dma: 0,
            page_start: 0,
            page_stop: 0,
            boundary: 0,
            tx_page: 0,
            tx_bytes: 0,
            remote_dma: 0,
            remote_start: 0,
            remote_bytes: 0,
            phys: [0; 6],
            curr: 0,
            mchash: [0; 8],
            rempkt: 0,
            localpkt: 0,
            address_cnt: 0,
            mem: vec![0; MEM_SIZE],
            tx_due: None,
            waiting: VecDeque::new(),
            outgoing: Vec::new(),
            pic_line: false,
            log: Vec::new(),
        };
        card.reset();
        card
    }

    /// A reset, through the reset port or at power on: the controller
    /// stopped, its registers cleared.
    pub fn reset(&mut self) {
        let (base, irq, mac, pic_line) = (self.base, self.irq, self.mac, self.pic_line);
        *self = Self {
            base,
            irq,
            mac,
            stop: true,
            start: false,
            rdma_cmd: 4,
            page: 0,
            isr: ISR_RST,
            imr: 0,
            dcr: 0x04,
            tcr: 0,
            tsr: 0,
            rcr: 0,
            rsr: 0,
            local_dma: 0,
            page_start: 0,
            page_stop: 0,
            boundary: 0,
            tx_page: 0,
            tx_bytes: 0,
            remote_dma: 0,
            remote_start: 0,
            remote_bytes: 0,
            phys: self.phys,
            curr: 0,
            mchash: self.mchash,
            rempkt: 0,
            localpkt: 0,
            address_cnt: 0,
            mem: std::mem::take(&mut self.mem),
            tx_due: None,
            waiting: std::mem::take(&mut self.waiting),
            outgoing: std::mem::take(&mut self.outgoing),
            pic_line,
            log: std::mem::take(&mut self.log),
        };
        self.mem.fill(0);
    }

    /// Whether the card's interrupt line is up.
    pub fn irq_line(&self) -> bool {
        self.isr & self.imr & 0x7F != 0
    }

    pub fn next_event(&self) -> Option<u64> {
        self.tx_due
    }

    /// The PROM: the address with each byte twice, then `WW`, which is how
    /// drivers tell an NE2000 from an NE1000.
    fn prom(&self, at: usize) -> u8 {
        match at {
            0..12 => self.mac.0[at / 2],
            12..32 => 0x57,
            _ => 0xFF,
        }
    }

    fn page_in_range(page: u8) -> bool {
        (PAGE_FIRST..=PAGE_LAST).contains(&page)
    }

    fn command(&self) -> u8 {
        (self.page << 6) | (self.rdma_cmd << 3) | ((self.tx_due.is_some() as u8) << 2) | ((self.start as u8) << 1) | self.stop as u8
    }

    /// A byte from port `offset` of the card.
    pub fn read(&mut self, offset: u16) -> u8 {
        match offset {
            0x00 => self.command(),
            0x01..=0x0F => match self.page {
                0 => self.page0_read(offset),
                1 => self.page1_read(offset),
                2 => self.page2_read(offset),
                _ => 0,
            },
            DATA => self.data_read(false) as u8,
            RESET => {
                self.reset();
                0
            }
            _ => 0,
        }
    }

    /// A byte to port `offset` of the card, at PIT tick `now`.
    pub fn write(&mut self, offset: u16, value: u8, now: u64) {
        match offset {
            0x00 => self.write_command(value, now),
            0x01..=0x0F => match self.page {
                0 => self.page0_write(offset, value),
                1 => self.page1_write(offset, value),
                2 => self.page2_write(offset, value),
                _ => {}
            },
            DATA => self.data_write(value as u16, false),
            RESET => self.reset(),
            _ => {}
        }
    }

    /// A word from the data port.
    pub fn read_word(&mut self) -> u16 {
        self.data_read(true)
    }

    /// A word to the data port.
    pub fn write_word(&mut self, value: u16) {
        self.data_write(value, true);
    }

    fn write_command(&mut self, mut value: u8, now: u64) {
        // No remote DMA command given is "abort", the safe one.
        if value & 0x38 == 0 {
            value |= 0x20;
        }
        if value & 0x01 != 0 {
            self.isr |= ISR_RST;
            self.stop = true;
        } else {
            self.stop = false;
        }
        self.rdma_cmd = (value & 0x38) >> 3;
        if value & 0x02 != 0 && !self.start {
            self.isr &= !ISR_RST;
        }
        self.start = value & 0x02 != 0;
        self.page = value >> 6;
        // Send packet: the remote DMA reads the frame at the boundary.
        if self.rdma_cmd == 3 {
            self.remote_start = self.boundary as u16 * 256;
            self.remote_dma = self.remote_start;
            let at = (self.boundary as usize * 256 + 2).wrapping_sub(MEM_START);
            if at + 1 < MEM_SIZE {
                self.remote_bytes = u16::from_le_bytes([self.mem[at], self.mem[at + 1]]);
            }
        }
        if value & 0x04 != 0 {
            self.transmit(now);
        }
        if self.rdma_cmd == 1 && self.start && self.remote_bytes == 0 {
            self.isr |= ISR_RDC;
        }
    }

    /// The frame in the transmit buffer.
    fn tx_frame(&self) -> Option<Vec<u8>> {
        if !Self::page_in_range(self.tx_page) {
            return None;
        }
        let at = self.tx_page as usize * 256 - MEM_START;
        (at + self.tx_bytes as usize <= MEM_SIZE).then(|| self.mem[at..at + self.tx_bytes as usize].to_vec())
    }

    fn transmit(&mut self, now: u64) {
        let loopback = (self.tcr >> 1) & 3;
        let Some(frame) = self.tx_frame() else {
            self.log.push(format!("[NE2000] A frame outside the buffer can't be sent (page {:02X})", self.tx_page));
            return;
        };
        if loopback != 0 {
            // Internal loopback, which drivers test the card with.
            if loopback == 1 {
                self.receive(&frame);
            }
            self.isr |= ISR_PTX;
            return;
        }
        // A frame still on its way goes first, to keep them in order.
        if self.tx_due.is_some() {
            self.tx_done();
        }
        if !frame.is_empty() {
            self.outgoing.push(frame);
        }
        // Preamble, gap and CRC, and the frame's bits, at 10 Mbit/s.
        let bits = 64 + 96 + 32 + self.tx_bytes as u64 * 8;
        let ticks = (bits * crate::timer::PIT_HZ).div_ceil(10_000_000);
        self.tx_due = Some(now + ticks.max(1));
    }

    fn tx_done(&mut self) {
        self.tx_due = None;
        self.tsr |= 0x01;
        self.isr |= ISR_PTX;
    }

    /// The frame being sent is on the wire at PIT tick `now`.
    pub fn advance(&mut self, now: u64) {
        if self.tx_due.is_some_and(|due| due <= now) {
            self.tx_done();
        }
    }

    fn page0_read(&self, offset: u16) -> u8 {
        match offset {
            0x01 => self.local_dma as u8,
            0x02 => (self.local_dma >> 8) as u8,
            0x03 => self.boundary,
            0x04 => self.tsr,
            0x05 => 0, // no collisions
            0x06 => 0, // the FIFO
            0x07 => self.isr,
            0x08 => self.remote_dma as u8,
            0x09 => (self.remote_dma >> 8) as u8,
            0x0A | 0x0B => 0xFF,
            0x0C => self.rsr,
            _ => 0, // the tally counters
        }
    }

    fn page0_write(&mut self, offset: u16, value: u8) {
        match offset {
            0x01 if Self::page_in_range(value) => self.page_start = value,
            0x02 if (PAGE_FIRST..=PAGE_LAST + 1).contains(&value) => self.page_stop = value,
            0x03 if Self::page_in_range(value) => self.boundary = value,
            0x04 if Self::page_in_range(value) => self.tx_page = value,
            0x05 => self.tx_bytes = (self.tx_bytes & 0xFF00) | value as u16,
            0x06 => self.tx_bytes = (self.tx_bytes & 0x00FF) | (value as u16) << 8,
            // Writing 1 clears a status bit; RST isn't one to clear.
            0x07 => self.isr &= !(value & 0x7F),
            0x08 => {
                self.remote_start = (self.remote_start & 0xFF00) | value as u16;
                self.remote_dma = self.remote_start;
            }
            0x09 => {
                self.remote_start = (self.remote_start & 0x00FF) | (value as u16) << 8;
                self.remote_dma = self.remote_start;
            }
            0x0A => self.remote_bytes = (self.remote_bytes & 0xFF00) | value as u16,
            0x0B => self.remote_bytes = (self.remote_bytes & 0x00FF) | (value as u16) << 8,
            0x0C => self.rcr = value & 0x3F,
            0x0D => self.tcr = value & 0x1F,
            0x0E => self.dcr = value & 0x7F,
            0x0F => self.imr = value & 0x7F,
            _ => {}
        }
    }

    fn page1_read(&self, offset: u16) -> u8 {
        match offset {
            0x01..=0x06 => self.phys[offset as usize - 1],
            0x07 => self.curr,
            _ => self.mchash[offset as usize - 8],
        }
    }

    fn page1_write(&mut self, offset: u16, value: u8) {
        match offset {
            0x01..=0x06 => self.phys[offset as usize - 1] = value,
            0x07 if Self::page_in_range(value) => self.curr = value,
            0x08..=0x0F => self.mchash[offset as usize - 8] = value,
            _ => {}
        }
    }

    fn page2_read(&self, offset: u16) -> u8 {
        match offset {
            0x01 => self.page_start,
            0x02 => self.page_stop,
            0x03 => self.rempkt,
            0x04 => self.tx_page,
            0x05 => self.localpkt,
            0x06 => (self.address_cnt >> 8) as u8,
            0x07 => self.address_cnt as u8,
            0x08..=0x0B => 0xFF,
            0x0C => self.rcr,
            0x0D => self.tcr,
            0x0E => self.dcr,
            _ => self.imr,
        }
    }

    fn page2_write(&mut self, offset: u16, value: u8) {
        match offset {
            0x01 => self.local_dma = (self.local_dma & 0xFF00) | value as u16,
            0x02 => self.local_dma = (self.local_dma & 0x00FF) | (value as u16) << 8,
            0x03 => self.rempkt = value,
            0x05 => self.localpkt = value,
            0x06 => self.address_cnt = (self.address_cnt & 0x00FF) | (value as u16) << 8,
            0x07 => self.address_cnt = (self.address_cnt & 0xFF00) | value as u16,
            _ => {}
        }
    }

    /// The card's memory at `at` as the remote DMA reads it: the PROM
    /// below the buffer.
    fn memory(&self, at: u16) -> u8 {
        let at = at as usize;
        match at {
            0..32 => self.prom(at),
            MEM_START..MEM_END => self.mem[at - MEM_START],
            _ => 0xFF,
        }
    }

    /// The next byte or word of the remote DMA from the card.
    fn data_read(&mut self, word: bool) -> u16 {
        if self.remote_bytes == 0 {
            return 0;
        }
        let word = word && self.remote_bytes > 1;
        let low = self.memory(self.remote_dma);
        let high = if word { self.memory(self.remote_dma.wrapping_add(1)) } else { 0 };
        self.advance_remote_dma();
        u16::from_le_bytes([low, high])
    }

    /// The next byte or word of the remote DMA to the card.
    fn data_write(&mut self, value: u16, word: bool) {
        if word && self.dcr & DCR_WTS == 0 {
            return;
        }
        let at = self.remote_dma as usize;
        let len = if word { 2 } else { 1 };
        for i in 0..len {
            let at = at + i;
            if (MEM_START..MEM_END).contains(&at) {
                self.mem[at - MEM_START] = (value >> (8 * i)) as u8;
            }
        }
        self.remote_dma = self.remote_dma.wrapping_add(len as u16);
        self.wrap_remote_dma();
        self.remote_bytes = self.remote_bytes.wrapping_sub(len as u16);
        if self.remote_bytes as usize > MEM_SIZE {
            self.remote_bytes = 0;
        }
        if self.remote_bytes == 0 {
            self.isr |= ISR_RDC;
        }
    }

    /// Past a byte or a word, as the data configuration says, as the
    /// DP8390 counts.
    fn advance_remote_dma(&mut self) {
        let step = if self.dcr & DCR_WTS != 0 { 2 } else { 1 };
        self.remote_dma = self.remote_dma.wrapping_add(step);
        self.wrap_remote_dma();
        self.remote_bytes = self.remote_bytes.saturating_sub(step);
        if self.remote_bytes == 0 {
            self.isr |= ISR_RDC;
        }
    }

    fn wrap_remote_dma(&mut self) {
        if self.page_stop != 0 && self.remote_dma == (self.page_stop as u16) << 8 {
            self.remote_dma = (self.page_start as u16) << 8;
        }
    }

    /// A frame from the network: into the ring now, or to wait for room.
    pub fn deliver(&mut self, frame: Vec<u8>) {
        if !self.accepts(&frame) {
            return;
        }
        if self.waiting.is_empty() && self.receive(&frame) {
            return;
        }
        if self.waiting.len() >= WAITING {
            self.waiting.pop_front();
        }
        self.waiting.push_back(frame);
    }

    /// Put the waiting frames into the ring as it has room.
    pub fn drain_waiting(&mut self) {
        while let Some(frame) = self.waiting.front() {
            if !self.accepts(frame) {
                self.waiting.pop_front();
                continue;
            }
            let frame = frame.clone();
            if !self.receive(&frame) {
                break;
            }
            self.waiting.pop_front();
        }
    }

    /// Frames are waiting for room in the ring.
    pub fn has_waiting(&self) -> bool {
        !self.waiting.is_empty()
    }

    /// Whether the card takes `frame`: it runs, is not looping back, and
    /// the frame passes its address filter.
    fn accepts(&self, frame: &[u8]) -> bool {
        if self.stop || self.dcr & DCR_LS == 0 || (self.tcr >> 1) & 3 != 0 || frame.len() < frame::HEADER {
            return false;
        }
        if frame.len() < 40 && self.rcr & RCR_AR == 0 {
            return false;
        }
        if self.rcr & RCR_PRO != 0 {
            return true;
        }
        let destination = &frame[..6];
        if destination == [0xFF; 6] {
            self.rcr & RCR_AB != 0
        } else if destination[0] & 1 != 0 {
            let bit = multicast_index(destination);
            self.rcr & RCR_AM != 0 && self.mchash[bit >> 3] & (1 << (bit & 7)) != 0
        } else {
            destination == self.phys
        }
    }

    /// Put `frame` into the receive ring with its 4-byte header (status,
    /// next page, length); false if the ring has no room for it.
    fn receive(&mut self, frame: &[u8]) -> bool {
        let (start, stop, curr) = (self.page_start, self.page_stop, self.curr);
        if self.stop
            || !Self::page_in_range(start)
            || !(PAGE_FIRST..=PAGE_LAST + 1).contains(&stop)
            || start >= stop
            || !(start..stop).contains(&curr)
        {
            return false;
        }
        let len = frame.len().max(frame::MIN_FRAME);
        let pages = (len + 4 + 4 + 255) / 256;
        let avail =
            if curr < self.boundary { (self.boundary - curr) as usize } else { (stop - start) as usize - (curr - self.boundary) as usize };
        // A full ring would look empty, as CURR would meet BNRY.
        if avail <= pages {
            return false;
        }
        let mut next = curr as usize + pages;
        if next >= stop as usize {
            next -= (stop - start) as usize;
        }
        let mut bytes = Vec::with_capacity(len + 4);
        let status = 0x01 | if frame[0] & 1 != 0 { 0x20 } else { 0 };
        bytes.extend_from_slice(&[status, next as u8]);
        bytes.extend_from_slice(&((len + 4) as u16).to_le_bytes());
        bytes.extend_from_slice(frame);
        bytes.resize(len + 4, 0);
        // Into the ring, from the current page on, wrapping at its end.
        let ring = (start as usize * 256 - MEM_START)..(stop as usize * 256 - MEM_START);
        let mut at = curr as usize * 256 - MEM_START;
        for byte in bytes {
            self.mem[at] = byte;
            at += 1;
            if at == ring.end {
                at = ring.start;
            }
        }
        self.curr = next as u8;
        self.rsr = 0x01 | if frame[0] & 1 != 0 { 0x20 } else { 0 };
        self.isr |= ISR_PRX;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MAC: Mac = Mac([0x02, 0x00, 0x5E, 0x10, 0x20, 0x30]);

    /// A card set up as a packet driver sets it up: word mode, receive
    /// ring from page 4Ch to 80h, transmit buffer at 40h, broadcasts
    /// taken, its address in PAR0-5, started.
    fn started() -> Ne2000 {
        let mut c = Ne2000::new(0x300, 10, MAC);
        c.write(0, 0x21, 0); // page 0, stop, abort DMA
        c.write(0x0E, 0x49, 0); // DCR: word, normal, FIFO 8
        c.write(0x01, 0x4C, 0); // PSTART
        c.write(0x02, 0x80, 0); // PSTOP
        c.write(0x03, 0x4C, 0); // BNRY
        c.write(0x04, 0x40, 0); // TPSR
        c.write(0x0C, 0x04, 0); // RCR: broadcasts
        c.write(0x0D, 0x00, 0); // TCR: normal
        c.write(0x07, 0xFF, 0); // clear ISR
        c.write(0x0F, 0x1F, 0); // IMR
        c.write(0, 0x61, 0); // page 1
        for i in 0..6 {
            c.write(1 + i, MAC.0[i as usize], 0);
        }
        c.write(0x07, 0x4D, 0); // CURR
        c.write(0, 0x22, 0); // page 0, start
        c
    }

    fn remote_dma(c: &mut Ne2000, address: u16, count: u16, command: u8) {
        c.write(0x08, address as u8, 0);
        c.write(0x09, (address >> 8) as u8, 0);
        c.write(0x0A, count as u8, 0);
        c.write(0x0B, (count >> 8) as u8, 0);
        c.write(0, command, 0);
    }

    #[test]
    fn resets_and_shows_its_prom() {
        let mut c = Ne2000::new(0x300, 10, MAC);
        assert_eq!(c.read(0) & 0x3F, 0x21, "stopped, DMA aborted");
        assert_eq!(c.read(0x07) & ISR_RST, ISR_RST);
        c.write(0x0E, 0x49, 0);
        // The PROM, a word at a time in word mode: each address byte twice.
        remote_dma(&mut c, 0, 32, 0x0A);
        let prom: Vec<u16> = (0..16).map(|_| c.read_word()).collect();
        for i in 0..6 {
            assert_eq!(prom[i], u16::from_le_bytes([MAC.0[i], MAC.0[i]]));
        }
        assert_eq!(prom[7], 0x5757, "WW: an NE2000");
        assert_eq!(c.read(0x07) & ISR_RDC, ISR_RDC, "the remote DMA completed");
        // The reset port.
        c.write(0, 0x22, 0);
        assert_eq!(c.read(0x07) & ISR_RST, 0);
        c.read(RESET);
        assert_eq!(c.read(0x07) & ISR_RST, ISR_RST);
    }

    #[test]
    fn remote_dma_writes_and_reads_the_buffer() {
        let mut c = started();
        remote_dma(&mut c, 0x4000, 6, 0x12); // write
        for w in [0x1234u16, 0x5678, 0x9ABC] {
            c.write_word(w);
        }
        assert_eq!(c.read(0x07) & ISR_RDC, ISR_RDC);
        c.write(0x07, ISR_RDC, 0);
        remote_dma(&mut c, 0x4000, 6, 0x0A); // read
        assert_eq!([c.read_word(), c.read_word(), c.read_word()], [0x1234, 0x5678, 0x9ABC]);
        // Byte mode.
        c.write(0x0E, 0x48, 0);
        remote_dma(&mut c, 0x4001, 2, 0x0A);
        assert_eq!([c.read(DATA), c.read(DATA)], [0x12, 0x78]);
    }

    #[test]
    fn sends_frames_and_says_so_after_the_wire_time() {
        let mut c = started();
        let frame = frame::build(Mac::BROADCAST, MAC, 0x0800, &[7; 100]);
        remote_dma(&mut c, 0x4000, frame.len() as u16, 0x12);
        for pair in frame.chunks(2) {
            c.write_word(u16::from_le_bytes([pair[0], *pair.get(1).unwrap_or(&0)]));
        }
        c.write(0x05, frame.len() as u8, 0);
        c.write(0x06, (frame.len() >> 8) as u8, 0);
        c.write(0, 0x26, 1000); // transmit
        assert_eq!(c.outgoing, vec![frame.clone()]);
        assert_eq!(c.read(0) & 0x04, 0x04, "sending");
        let due = c.next_event().unwrap();
        assert!(due > 1000 && due < 1000 + 200, "{}", due);
        c.advance(due);
        assert_eq!(c.read(0x07) & ISR_PTX, ISR_PTX);
        assert_eq!(c.read(0x04) & 1, 1, "TSR: sent");
        assert!(c.irq_line());
        c.write(0x07, ISR_PTX, 0);
        assert!(!c.irq_line());
    }

    #[test]
    fn frames_come_into_the_ring_and_wrap() {
        let mut c = started();
        let mine = frame::build(MAC, Mac([2, 0, 0, 0, 0, 9]), 0x0800, &[1; 300]);
        c.deliver(mine.clone());
        assert_eq!(c.read(0x07) & ISR_PRX, ISR_PRX);
        assert!(c.irq_line());
        // Its header: status, next page, length.
        remote_dma(&mut c, 0x4D00, 4, 0x0A);
        let (h0, h1) = (c.read_word(), c.read_word());
        assert_eq!(h0, 0x4F01, "received, next packet at page 4Fh");
        assert_eq!(h1 as usize, mine.len() + 4);
        remote_dma(&mut c, 0x4D04, 2, 0x0A);
        assert_eq!(c.read_word().to_le_bytes(), [MAC.0[0], MAC.0[1]]);
        // Someone else's frame is filtered out; a broadcast isn't.
        c.deliver(frame::build(Mac([2, 0, 0, 0, 0, 8]), MAC, 0x0800, &[0; 50]));
        c.write(0, 0x62, 0);
        assert_eq!(c.read(0x07), 0x4F);
        c.write(0, 0x22, 0);
        c.deliver(frame::build(Mac::BROADCAST, MAC, 0x0800, &[0; 50]));
        c.write(0, 0x62, 0);
        assert_eq!(c.read(0x07), 0x50);
        c.write(0, 0x22, 0);
        // Fill the ring: the frames that don't fit wait for the driver to
        // move the boundary on.
        for _ in 0..40 {
            c.deliver(mine.clone());
        }
        assert!(c.has_waiting());
        c.write(0x03, 0x7E, 0); // BNRY: taken up to near the end
        c.drain_waiting();
        c.write(0, 0x62, 0);
        let curr = c.read(0x07);
        c.write(0, 0x22, 0);
        assert!((0x4C..0x80).contains(&curr), "CURR wrapped into the ring: {:02X}", curr);
    }

    #[test]
    fn loops_back_in_internal_loopback() {
        let mut c = started();
        c.write(0x0D, 0x02, 0); // TCR: internal loopback
        c.write(0x0C, 0x1F, 0);
        let frame = frame::build(MAC, MAC, 0x0800, &[3; 50]);
        remote_dma(&mut c, 0x4000, frame.len() as u16, 0x12);
        for pair in frame.chunks(2) {
            c.write_word(u16::from_le_bytes([pair[0], pair[1]]));
        }
        c.write(0x05, frame.len() as u8, 0);
        c.write(0x06, 0, 0);
        c.write(0, 0x26, 0);
        assert!(c.outgoing.is_empty());
        assert_eq!(c.read(0x07) & (ISR_PTX | ISR_PRX), ISR_PTX | ISR_PRX);
    }

    #[test]
    fn takes_the_multicasts_its_hash_filter_has() {
        let mut c = started();
        let group = Mac([0x01, 0x00, 0x5E, 0x00, 0x00, 0x01]);
        let frame = frame::build(group, Mac([2, 0, 0, 0, 0, 9]), 0x0800, &[0; 50]);
        c.write(0x0C, RCR_AB | RCR_AM, 0);
        c.deliver(frame.clone());
        assert_eq!(c.read(0x07) & ISR_PRX, 0, "not in the filter");
        let bit = multicast_index(&group.0);
        assert!(bit < 64);
        c.write(0, 0x62, 0);
        c.write(0x08 + (bit >> 3) as u16, 1 << (bit & 7), 0);
        c.write(0, 0x22, 0);
        c.deliver(frame);
        assert_eq!(c.read(0x07) & ISR_PRX, ISR_PRX);
    }
}
