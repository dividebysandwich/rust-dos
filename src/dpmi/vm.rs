//! Virtual memory for the host's clients in a DOS machine of the built-in
//! Windows: what Windows 98's VMM gives a DOS box whose PIF sets its
//! protected-mode (DPMI) memory. There a DPMI client's memory blocks are
//! virtual: they may add up to the PIF's limit, more than the machine
//! has, and the VMM pages them to the swap file. Redguard's launcher
//! starts the game through such a PIF (FFFFh KB), and the game asks for
//! about 45 MB on a machine that has 32.
//!
//! On plain DOS the host has no virtual memory, as a real machine's DOS
//! extenders have none without a swap file of their own: its clients'
//! memory is extended memory, and a linear address is a physical one.
//! In a DOS machine with a limit (`dosvm::dpmi_limit`) the host turns
//! paging on while its clients run in protected mode, and off in real
//! mode, which can't page:
//!
//! - The page tables map the machine's memory where it is, up to `base`
//!   (the memory rounded up to 4 MB), so that conventional memory, the
//!   host's tables and every address a client knows from plain DOS stay
//!   where they are. Physical addresses a client maps (INT 31h
//!   AX=0800h) are mapped where they are too.
//! - Memory blocks (INT 31h AX=0501h and the like) come from the linear
//!   space from `base` up, as much as the limit. Their pages have no
//!   physical page until they are touched: a page fault there gives the
//!   page one (`bring_in`), zeroed, or with its contents read back from
//!   the page file, and the client goes on at the instruction that
//!   faulted. The page file is the host's own, kept with the machine's
//!   state.
//! - Physical pages come from extended memory, a block at a time
//!   (`CHUNK`), while there is free memory left besides `RESERVE`. Then a
//!   page no client touched since the last look (the accessed bit of its
//!   page table entry, cleared on each look) goes to the page file to
//!   free one, unless it is locked (INT 31h AX=0600h).
//! - The host's own reads and writes of a client's memory (DOS calls'
//!   buffers, call structures) go through `phys`, which brings a page in
//!   as a fault would.
//!
//! A Windows 98 DOS box has one swap file with the system's other memory,
//! and the VMM may page the Windows programs out for it. Here the DOS
//! program gets the memory the Windows programs lend it
//! (`memory::Pool::lend`) as physical pages, and the rest in the page
//! file: what the program can have is the same.

use std::collections::BTreeMap;

use crate::cpu::{CR0_PG, Cpu};

const PAGE: u32 = 0x1000;
/// The bytes a page table maps.
const TABLE_SPAN: u32 = 0x40_0000;
/// Page table entry bits.
const PRESENT: u32 = 0x001;
const ACCESSED: u32 = 0x020;
/// Present, writable, and reachable from level 3: every entry has these.
const USER_PAGE: u32 = 0x007;
/// The extended memory taken at a time for physical pages.
const CHUNK: u32 = 0x1_0000;
/// The extended memory the pages leave free, for what wants it besides
/// them: a nested client's LDT, a DOS call's block (INT 21h AH=48h), XMS.
const RESERVE: u32 = 0x8_0000;
/// The physical pages that locks (INT 31h AX=0600h) leave unlocked, as
/// the VMM limits what may be locked: there is always a page to send out
/// for one that comes in, and for a page table.
const UNLOCKED: u32 = 16;
/// The highest the linear space may reach: below the addresses where
/// cards map their memory.
const SPACE_END: u32 = 0xC000_0000;

/// The host's virtual memory: the page tables, the linear space of the
/// memory blocks and where each of its pages is.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Vm {
    /// The bytes of the linear space for memory blocks: the PIF's limit,
    /// in whole pages.
    limit: u32,
    /// Where that space starts.
    base: u32,
    /// The page directory's physical address.
    dir: u32,
    /// The extended memory taken, as (address, bytes): the directory,
    /// the page tables and the physical pages.
    chunks: Vec<(u32, u32)>,
    /// Physical pages taken and not in use.
    free: Vec<u32>,
    /// The memory blocks: address to bytes.
    blocks: BTreeMap<u32, u32>,
    /// The pages of the blocks that have a physical page: linear address
    /// to physical.
    present: BTreeMap<u32, u32>,
    /// The pages of the blocks that have none: linear address to their
    /// place in the page file plus 1, or 0 for a page of zeros.
    out: BTreeMap<u32, u32>,
    /// The page file: the contents of pages, by place; an empty one is
    /// free, and in `free_slots`.
    slots: Vec<Vec<u8>>,
    free_slots: Vec<u32>,
    /// Locked pages (INT 31h AX=0600h), which stay in memory: linear
    /// address to how many locks.
    locks: BTreeMap<u32, u32>,
    /// The page the last search for one to page out stopped at.
    hand: u32,
    /// The pages read back from the page file and written to it.
    ins: u64,
    outs: u64,
}

impl Vm {
    /// Virtual memory of `limit` bytes for a DOS machine, with its page
    /// tables in extended memory: None if there isn't room for them.
    fn new(cpu: &mut Cpu, limit: u32) -> Option<Vm> {
        let ram = cpu.bus.ram().len() as u32;
        let base = ram.checked_add(TABLE_SPAN - 1)? & !(TABLE_SPAN - 1);
        let limit = limit.min(SPACE_END.saturating_sub(base)) & !(PAGE - 1);
        if limit == 0 {
            return None;
        }
        let mut vm = Vm {
            limit,
            base,
            ..Vm::default()
        };
        vm.dir = vm.frame(cpu)?;
        cpu.bus
            .fill_ram(vm.dir as usize..(vm.dir + PAGE) as usize, 0);
        for table in 0..base / TABLE_SPAN {
            let at = vm.table(cpu, table * TABLE_SPAN)?;
            let entries: Vec<u8> = (0..1024u32)
                .flat_map(|i| ((table * TABLE_SPAN + i * PAGE) | USER_PAGE).to_le_bytes())
                .collect();
            cpu.bus.load_bytes(at as usize & !0xFFF, &entries);
        }
        Some(vm)
    }

    /// The end of the linear space of memory blocks.
    fn end(&self) -> u32 {
        self.base + self.limit
    }

    /// Whether `lin` is in the linear space of memory blocks.
    fn has(&self, lin: u32) -> bool {
        (self.base..self.end()).contains(&lin)
    }

    /// A physical page: one taken before, a block of extended memory more,
    /// or one a page of a block gives up for it (`page_out`). Its contents
    /// are what they were.
    fn frame(&mut self, cpu: &mut Cpu) -> Option<u32> {
        if let Some(page) = self.free.pop() {
            return Some(page);
        }
        let end = cpu.bus.ram().len() as u32;
        let (largest, _) = cpu.bus.xms.dpmi_free(end);
        let size = if largest >= CHUNK + RESERVE {
            CHUNK
        } else {
            PAGE
        };
        if largest >= size + RESERVE
            && let Some(chunk) = cpu.bus.xms.take_dpmi(size, end)
        {
            self.chunks.push((chunk, size));
            self.free
                .extend((PAGE..size).step_by(PAGE as usize).rev().map(|o| chunk + o));
            return Some(chunk);
        }
        self.page_out(cpu)
    }

    /// The physical address of the page table entry of `lin`, with a
    /// page table made for it if there is none.
    fn table(&mut self, cpu: &mut Cpu, lin: u32) -> Option<u32> {
        let pde_at = self.dir + 4 * (lin >> 22);
        let mut pde = cpu.bus.read_32(pde_at as usize);
        if pde & PRESENT == 0 {
            let table = self.frame(cpu)?;
            cpu.bus.fill_ram(table as usize..(table + PAGE) as usize, 0);
            pde = table | USER_PAGE;
            cpu.bus.write_32(pde_at as usize, pde);
        }
        Some((pde & !0xFFF) + 4 * ((lin >> 12) & 0x3FF))
    }

    /// The physical address of the page table entry of `lin`, if it has a
    /// page table.
    fn entry(&self, cpu: &Cpu, lin: u32) -> Option<u32> {
        let pde = cpu.bus.read_32((self.dir + 4 * (lin >> 22)) as usize);
        (pde & PRESENT != 0).then(|| (pde & !0xFFF) + 4 * ((lin >> 12) & 0x3FF))
    }

    /// Send a page of a block that no client touched since the last look
    /// to the page file, and give its physical page: the first from where
    /// the last search stopped, skipping locked ones and clearing the
    /// accessed bit of those touched (two rounds, so the first may only
    /// clear them). None if every page is locked.
    fn page_out(&mut self, cpu: &mut Cpu) -> Option<u32> {
        let mut left = 2 * self.present.len();
        let mut hand = self.hand;
        let victim = loop {
            if left == 0 {
                return None;
            }
            left -= 1;
            let next = self
                .present
                .range(hand + 1..)
                .next()
                .or_else(|| self.present.iter().next());
            let (&lin, &phys) = next?;
            hand = lin;
            if self.locks.contains_key(&lin) {
                continue;
            }
            let at = self.entry(cpu, lin)? as usize;
            let entry = cpu.bus.read_32(at);
            if entry & ACCESSED != 0 {
                cpu.bus.write_32(at, entry & !ACCESSED);
                cpu.tlb.flush_page(lin);
                continue;
            }
            break (lin, phys, at);
        };
        let (lin, phys, at) = victim;
        self.hand = hand;
        let mut data = vec![0; PAGE as usize];
        cpu.bus.read_into(phys as usize, &mut data);
        let slot = if data.iter().all(|&b| b == 0) {
            0
        } else {
            self.store(data)
        };
        cpu.bus.write_32(at, 0);
        cpu.tlb.flush_page(lin);
        self.present.remove(&lin);
        self.out.insert(lin, slot);
        self.outs += 1;
        Some(phys)
    }

    /// Keep a page's contents in the page file: its place plus 1.
    fn store(&mut self, data: Vec<u8>) -> u32 {
        match self.free_slots.pop() {
            Some(slot) => {
                self.slots[slot as usize] = data;
                slot + 1
            }
            None => {
                self.slots.push(data);
                self.slots.len() as u32
            }
        }
    }

    /// Take the contents at `slot` (a place plus 1) out of the page file:
    /// None for a page of zeros.
    fn take(&mut self, slot: u32) -> Option<Vec<u8>> {
        let s = slot.checked_sub(1)?;
        let data = std::mem::take(self.slots.get_mut(s as usize)?);
        self.free_slots.push(s);
        Some(data)
    }

    /// Give the page at `lin` of a block a physical page, with its
    /// contents. False if it isn't a page of a block that is out, or no
    /// physical page can be had.
    fn bring_in(&mut self, cpu: &mut Cpu, lin: u32) -> bool {
        let lin = lin & !(PAGE - 1);
        let Some(slot) = self.out.remove(&lin) else {
            return false;
        };
        let room = self.table(cpu, lin).zip(self.frame(cpu));
        let Some((at, phys)) = room else {
            self.out.insert(lin, slot);
            return false;
        };
        match self.take(slot) {
            Some(data) => {
                cpu.bus.load_bytes(phys as usize, &data);
                self.ins += 1;
            }
            None => cpu.bus.fill_ram(phys as usize..(phys + PAGE) as usize, 0),
        }
        // Accessed, as the access that wants it is about to make it: the
        // next search for a page to send out passes it once.
        cpu.bus.write_32(at as usize, phys | USER_PAGE | ACCESSED);
        self.present.insert(lin, phys);
        true
    }

    /// Where a block of `len` bytes fits in the linear space, from `at`
    /// on (the first gap) or exactly at `at` (`exact`).
    fn place(&self, len: u32, at: Option<u32>) -> Option<u32> {
        if let Some(at) = at {
            let end = at.checked_add(len)?;
            let clear = self.blocks.iter().all(|(&b, &l)| end <= b || b + l <= at);
            return (self.has(at) && end <= self.end() && clear).then_some(at);
        }
        let mut start = self.base;
        for (&b, &l) in &self.blocks {
            if b - start >= len {
                return Some(start);
            }
            start = b + l;
        }
        (self.end() - start >= len).then_some(start)
    }

    /// Commit the pages of `[from, to)` as pages of zeros.
    fn commit(&mut self, from: u32, to: u32) {
        for page in (from..to).step_by(PAGE as usize) {
            self.out.insert(page, 0);
        }
    }

    /// Give up the pages of `[from, to)`: their physical pages and places
    /// in the page file.
    fn decommit(&mut self, cpu: &mut Cpu, from: u32, to: u32) {
        for page in (from..to).step_by(PAGE as usize) {
            if let Some(phys) = self.present.remove(&page) {
                if let Some(at) = self.entry(cpu, page) {
                    cpu.bus.write_32(at as usize, 0);
                }
                cpu.tlb.flush_page(page);
                self.free.push(phys);
            }
            if let Some(slot) = self.out.remove(&page) {
                self.take(slot);
            }
            self.locks.remove(&page);
        }
    }

    /// Take a lock off each of `pages` that has one.
    fn unlock(&mut self, pages: &[u32]) {
        for page in pages {
            if let Some(count) = self.locks.get_mut(page) {
                *count -= 1;
                if *count == 0 {
                    self.locks.remove(page);
                }
            }
        }
    }

    /// Move the page at `from` of a block to `to`, where no page is.
    fn move_page(&mut self, cpu: &mut Cpu, from: u32, to: u32) {
        if let Some(slot) = self.out.remove(&from) {
            self.out.insert(to, slot);
        } else if let Some(phys) = self.present.remove(&from) {
            if let Some(at) = self.entry(cpu, from) {
                cpu.bus.write_32(at as usize, 0);
            }
            cpu.tlb.flush_page(from);
            match self.entry(cpu, to) {
                Some(at) => {
                    cpu.bus.write_32(at as usize, phys | USER_PAGE);
                    self.present.insert(to, phys);
                }
                // No page table there (see `resize`): the page goes to the
                // page file.
                None => {
                    let mut data = vec![0; PAGE as usize];
                    cpu.bus.read_into(phys as usize, &mut data);
                    let slot = self.store(data);
                    self.out.insert(to, slot);
                    self.free.push(phys);
                }
            }
        }
        if let Some(locks) = self.locks.remove(&from) {
            self.locks.insert(to, locks);
        }
    }

    /// Allocate a block of `bytes` (whole pages), at `at` or anywhere:
    /// its address.
    fn allocate(&mut self, bytes: u32, at: Option<u32>) -> Option<u32> {
        let len = bytes.checked_add(PAGE - 1)? & !(PAGE - 1);
        if len == 0 || at.is_some_and(|a| a & (PAGE - 1) != 0) {
            return None;
        }
        let base = self.place(len, at)?;
        self.blocks.insert(base, len);
        self.commit(base, base + len);
        Some(base)
    }

    /// Free the block at `base`. False if there is none.
    fn release(&mut self, cpu: &mut Cpu, base: u32) -> bool {
        let Some(len) = self.blocks.remove(&base) else {
            return false;
        };
        self.decommit(cpu, base, base + len);
        true
    }

    /// Make the block at `base` `bytes` long (whole pages), where it is or
    /// elsewhere, with what it held: its address.
    fn resize(&mut self, cpu: &mut Cpu, base: u32, bytes: u32) -> Option<u32> {
        let old = *self.blocks.get(&base)?;
        let len = bytes.checked_add(PAGE - 1)? & !(PAGE - 1);
        if len == 0 {
            return None;
        }
        self.blocks.remove(&base);
        if self.place(len, Some(base)).is_some() {
            self.blocks.insert(base, len);
            if len > old {
                self.commit(base + old, base + len);
            } else {
                self.decommit(cpu, base + len, base + old);
            }
            return Some(base);
        }
        let Some(new) = self.place(len, None) else {
            self.blocks.insert(base, old);
            return None;
        };
        self.blocks.insert(new, len);
        let kept = old.min(len);
        // The page tables of the new place first: making one may send a
        // page out, which then moves as a page in the page file. They
        // start at 4 MB boundaries, and the new place need not.
        let first = new & !(TABLE_SPAN - 1);
        for table in (first..new + kept).step_by(TABLE_SPAN as usize) {
            self.table(cpu, table);
        }
        for offset in (0..kept).step_by(PAGE as usize) {
            self.move_page(cpu, base + offset, new + offset);
        }
        self.decommit(cpu, base + kept, base + old);
        self.commit(new + kept, new + len);
        Some(new)
    }

    /// The largest block that fits in the linear space, and its free
    /// bytes.
    fn free_space(&self) -> (u32, u32) {
        let mut largest = 0;
        let mut start = self.base;
        for (&b, &l) in &self.blocks {
            largest = largest.max(b - start);
            start = b + l;
        }
        largest = largest.max(self.end() - start);
        let used: u32 = self.blocks.values().sum();
        (largest, self.limit - used)
    }

    /// The physical pages it may still have: those taken and free, the
    /// extended memory it may take, and the pages of blocks that can go
    /// to the page file.
    fn physical_pages(&self, cpu: &Cpu) -> u32 {
        let end = cpu.bus.ram().len() as u32;
        let (_, free) = cpu.bus.xms.dpmi_free(end);
        let unlocked = self
            .present
            .keys()
            .filter(|lin| !self.locks.contains_key(lin))
            .count();
        self.free.len() as u32 + free.saturating_sub(RESERVE) / PAGE + unlocked as u32
    }
}

/// Start virtual memory for the host's first client, in a DOS machine
/// whose PIF gives it a limit; none on plain DOS or for Auto.
pub(super) fn start(cpu: &mut Cpu) {
    let Some(limit) = crate::win32::dosvm::dpmi_limit(&cpu.bus.win32) else {
        return;
    };
    match Vm::new(cpu, limit) {
        Some(vm) => {
            cpu.bus.log_string(&format!(
                "[DPMI] Virtual memory: {} KB of memory blocks from {:08X}, as the DOS machine's PIF \
                 allows",
                vm.limit / 1024,
                vm.base
            ));
            cpu.bus.dpmi.vm = Some(vm);
        }
        None => cpu
            .bus
            .log_string("[DPMI] No memory for the page tables: no virtual memory"),
    }
}

/// The last client ended: the page tables and pages go back to the XMS
/// pool, and the page file is emptied.
pub(super) fn stop(cpu: &mut Cpu) {
    let Some(vm) = cpu.bus.dpmi.vm.take() else {
        return;
    };
    leave(cpu);
    for (chunk, _) in vm.chunks {
        cpu.bus.xms.release_dpmi(chunk);
    }
}

/// Run `f` on the virtual memory, if there is any.
fn with<R>(cpu: &mut Cpu, f: impl FnOnce(&mut Vm, &mut Cpu) -> R) -> Option<R> {
    let mut vm = cpu.bus.dpmi.vm.take()?;
    let result = f(&mut vm, cpu);
    cpu.bus.dpmi.vm = Some(vm);
    Some(result)
}

/// Whether the host has virtual memory.
pub(super) fn active(cpu: &Cpu) -> bool {
    cpu.bus.dpmi.vm.is_some()
}

/// Paging on, with the host's tables, as the processor goes to protected
/// mode for a client.
pub(super) fn enter(cpu: &mut Cpu) {
    let Some(dir) = cpu.bus.dpmi.vm.as_ref().map(|vm| vm.dir) else {
        return;
    };
    if cpu.cr0 & CR0_PG == 0 || cpu.cr3 != dir {
        cpu.cr3 = dir;
        cpu.cr0 |= CR0_PG;
        cpu.tlb.flush();
    }
}

/// Paging off, as the processor goes to real mode.
pub(super) fn leave(cpu: &mut Cpu) {
    if cpu.cr0 & CR0_PG != 0 {
        cpu.cr0 &= !CR0_PG;
        cpu.tlb.flush();
    }
}

/// A page fault at `lin`, error code `error`: whether it was a page of a
/// memory block that wasn't there, which is now, so that the instruction
/// can run again.
pub(super) fn page_fault(cpu: &mut Cpu, lin: u32, error: u32) -> bool {
    error & PRESENT == 0 && with(cpu, |vm, cpu| vm.bring_in(cpu, lin)).unwrap_or(false)
}

/// The physical address of the client's linear address `lin`, for the
/// host's own reads and writes: through the page tables, with the page
/// brought in if it is out, when there is virtual memory; `lin` itself
/// otherwise.
pub(super) fn phys(cpu: &mut Cpu, lin: u32) -> u32 {
    let page = lin & !(PAGE - 1);
    let found = with(cpu, |vm, cpu| {
        if !vm.has(lin) {
            return None;
        }
        if !vm.present.contains_key(&page) {
            vm.bring_in(cpu, page);
        }
        vm.present.get(&page).map(|&phys| {
            // The host's access counts as a touch. The entry is written
            // only when that changes it: a write to RAM is what a loop
            // that waits (`idle`) can't be proven idle with.
            if let Some(at) = vm.entry(cpu, page) {
                let entry = cpu.bus.read_32(at as usize);
                if entry & ACCESSED == 0 {
                    cpu.bus.write_32(at as usize, entry | ACCESSED);
                }
            }
            phys | (lin & (PAGE - 1))
        })
    });
    found.flatten().unwrap_or(lin)
}

/// Bring the pages of `len` bytes at `lin` in, before the host reaches
/// them through the processor (its segments), which would fault.
pub(super) fn touch(cpu: &mut Cpu, lin: u32, len: u32) {
    if !active(cpu) || len == 0 {
        return;
    }
    let last = lin.wrapping_add(len - 1) & !(PAGE - 1);
    let mut page = lin & !(PAGE - 1);
    loop {
        phys(cpu, page);
        if page == last {
            break;
        }
        page = page.wrapping_add(PAGE);
    }
}

pub(super) fn read_8(cpu: &mut Cpu, lin: u32) -> u8 {
    let at = phys(cpu, lin);
    cpu.bus.read_8(at as usize)
}

pub(super) fn write_8(cpu: &mut Cpu, lin: u32, value: u8) {
    let at = phys(cpu, lin);
    cpu.bus.write_8(at as usize, value);
}

pub(super) fn read_16(cpu: &mut Cpu, lin: u32) -> u16 {
    u16::from_le_bytes([read_8(cpu, lin), read_8(cpu, lin.wrapping_add(1))])
}

pub(super) fn write_16(cpu: &mut Cpu, lin: u32, value: u16) {
    for (i, byte) in value.to_le_bytes().into_iter().enumerate() {
        write_8(cpu, lin.wrapping_add(i as u32), byte);
    }
}

pub(super) fn read_32(cpu: &mut Cpu, lin: u32) -> u32 {
    let mut bytes = [0; 4];
    for (i, byte) in bytes.iter_mut().enumerate() {
        *byte = read_8(cpu, lin.wrapping_add(i as u32));
    }
    u32::from_le_bytes(bytes)
}

/// A memory block of `bytes` for a client, at the linear address `at` or
/// anywhere: its address, its pages zeros. Extended memory with no
/// virtual memory.
pub(super) fn allocate(cpu: &mut Cpu, bytes: u32, at: Option<u32>) -> Option<u32> {
    if let Some(base) = with(cpu, |vm, _| vm.allocate(bytes, at)) {
        return base;
    }
    let end = cpu.bus.ram().len() as u32;
    let base = match at {
        Some(at) => cpu.bus.xms.take_dpmi_at(at, bytes, end).then_some(at)?,
        None => cpu.bus.xms.take_dpmi(bytes, end)?,
    };
    let len = bytes.div_ceil(PAGE) * PAGE;
    cpu.bus.fill_ram(base as usize..(base + len) as usize, 0);
    Some(base)
}

/// Free the memory block at `base`.
pub(super) fn release(cpu: &mut Cpu, base: u32) {
    if with(cpu, |vm, cpu| vm.release(cpu, base)) != Some(true) {
        cpu.bus.xms.release_dpmi(base);
    }
}

/// Make the memory block at `base`, `old` bytes long, `bytes` long, where
/// it is or elsewhere, with what it held and zeros after: its address.
pub(super) fn resize(cpu: &mut Cpu, base: u32, old: u32, bytes: u32) -> Option<u32> {
    if is_virtual(cpu.bus.dpmi.vm.as_ref(), base) {
        return with(cpu, |vm, cpu| vm.resize(cpu, base, bytes)).flatten();
    }
    let end = cpu.bus.ram().len() as u32;
    let len = bytes.div_ceil(PAGE) * PAGE;
    if cpu.bus.xms.resize_dpmi(base, len, end) {
        if len > old {
            cpu.bus
                .fill_ram((base + old) as usize..(base + len) as usize, 0);
        }
        return Some(base);
    }
    let new = cpu.bus.xms.take_dpmi(len, end)?;
    cpu.bus.fill_ram(new as usize..(new + len) as usize, 0);
    cpu.bus
        .copy_ram(base as usize, new as usize, old.min(len) as usize);
    cpu.bus.xms.release_dpmi(base);
    Some(new)
}

/// The largest memory block a client can have and all it can have, in
/// bytes.
pub(super) fn free(cpu: &Cpu) -> (u32, u32) {
    match &cpu.bus.dpmi.vm {
        Some(vm) => vm.free_space(),
        None => cpu.bus.xms.dpmi_free(cpu.bus.ram().len() as u32),
    }
}

/// INT 31h AX=0500h's information, twelve dwords: the largest block, the
/// pages of the largest block that may be unlocked and locked, the pages
/// of the linear space, the free pages that may be unlocked, the free
/// pages, the machine's pages, the free pages of the linear space and the
/// page file's, and three reserved (FFFFFFFFh, as for "not known").
pub(super) fn info(cpu: &Cpu) -> [u32; 12] {
    let end = cpu.bus.ram().len() as u32;
    let pages = end / PAGE;
    let (largest, total) = free(cpu);
    match &cpu.bus.dpmi.vm {
        Some(vm) => {
            let locked = vm
                .physical_pages(cpu)
                .saturating_sub(UNLOCKED)
                .min(largest / PAGE);
            [
                largest,
                largest / PAGE,
                locked,
                vm.limit / PAGE,
                total / PAGE,
                total / PAGE,
                pages,
                total / PAGE,
                vm.limit / PAGE,
                u32::MAX,
                u32::MAX,
                u32::MAX,
            ]
        }
        None => [
            largest,
            largest / PAGE,
            largest / PAGE,
            pages,
            total / PAGE,
            total / PAGE,
            pages,
            total / PAGE,
            u32::MAX,
            u32::MAX,
            u32::MAX,
            u32::MAX,
        ],
    }
}

/// Lock (or unlock) the pages of `len` bytes at `lin`: locked pages of a
/// block are brought in and stay in memory. False, with no page locked,
/// if they can't all be in memory at once with `UNLOCKED` pages left
/// over (INT 31h AX=0600h's error 8013h). With no virtual memory every
/// page is always there.
pub(super) fn lock(cpu: &mut Cpu, lin: u32, len: u32, on: bool) -> bool {
    if len == 0 {
        return true;
    }
    let first = lin & !(PAGE - 1);
    let last = lin.saturating_add(len - 1) & !(PAGE - 1);
    let locked = with(cpu, |vm, cpu| {
        let pages: Vec<u32> = vm
            .present
            .range(first..=last)
            .chain(vm.out.range(first..=last))
            .map(|(&page, _)| page)
            .collect();
        if !on {
            vm.unlock(&pages);
            return true;
        }
        // No more than leaves `UNLOCKED` pages to page through.
        let new = pages
            .iter()
            .filter(|page| !vm.locks.contains_key(page))
            .count() as u32;
        if new + UNLOCKED > vm.physical_pages(cpu) {
            return false;
        }
        // Locked before they come in, so that bringing in one doesn't
        // send another of them out.
        for &page in &pages {
            *vm.locks.entry(page).or_default() += 1;
        }
        let all_in = pages
            .iter()
            .all(|page| vm.present.contains_key(page) || vm.bring_in(cpu, *page));
        if !all_in {
            vm.unlock(&pages);
        }
        all_in
    });
    locked.unwrap_or(true)
}

/// Map `len` bytes of physical memory at `phys` (INT 31h AX=0800h): its
/// linear address, the same. With virtual memory the page tables get
/// entries for it where it lies past the machine's memory; None where it
/// would cover the linear space of memory blocks, runs past 4 GB, or
/// there is no room for the tables. Without, any range is its own linear
/// address, as before there was virtual memory.
pub(super) fn map_physical(cpu: &mut Cpu, phys: u32, len: u32) -> Option<u32> {
    let end = phys as u64 + len as u64;
    let mapped = with(cpu, |vm, cpu| {
        if end > 1 << 32 || (end > vm.base as u64 && phys < vm.end()) {
            return None;
        }
        let first = (phys & !(PAGE - 1)) as u64;
        for page in (first..end).step_by(PAGE as usize) {
            let page = page as u32;
            if page < vm.base {
                continue;
            }
            let at = vm.table(cpu, page)?;
            cpu.bus.write_32(at as usize, page | USER_PAGE);
            cpu.tlb.flush_page(page);
        }
        Some(phys)
    });
    mapped.unwrap_or(Some(phys))
}

/// The extended memory the virtual memory holds, as (address, bytes), for
/// the XMS pool after a state is loaded.
pub(super) fn reservations(vm: &Vm) -> Vec<(u32, u32)> {
    vm.chunks.clone()
}

/// Whether `base` is the address of a block in the linear space: memory
/// the XMS pool has no part in.
pub(super) fn is_virtual(vm: Option<&Vm>, base: u32) -> bool {
    vm.is_some_and(|vm| vm.blocks.contains_key(&base))
}

crate::state_fields!(Vm {
    limit,
    base,
    dir,
    chunks,
    free,
    blocks,
    present,
    out,
    slots,
    free_slots,
    locks,
    hand,
    ins,
    outs,
});

#[cfg(test)]
mod tests {
    use super::*;

    /// A machine whose extended memory has `free` bytes left for the
    /// pages besides the reserve, with a host's virtual memory of
    /// `limit` bytes, as a client in a DOS machine with such a PIF gets.
    fn machine(free: u32, limit: u32) -> Box<Cpu> {
        let mut cpu = Cpu::new(std::path::PathBuf::from("."));
        // As the host has it.
        cpu.bus.set_a20(true);
        let vm = Vm::new(&mut cpu, limit).expect("room for the page tables");
        let end = cpu.bus.ram().len() as u32;
        let (largest, _) = cpu.bus.xms.dpmi_free(end);
        cpu.bus
            .xms
            .take_dpmi(largest - free - RESERVE, end)
            .unwrap();
        cpu.bus.dpmi.vm = Some(vm);
        enter(&mut cpu);
        cpu
    }

    fn vm(cpu: &Cpu) -> &Vm {
        cpu.bus.dpmi.vm.as_ref().unwrap()
    }

    /// Write `value(page)` into every page of `len` bytes at `lin`
    /// through the processor, faulting pages in as the host's handler
    /// does.
    fn fill(cpu: &mut Cpu, lin: u32, len: u32, value: impl Fn(u32) -> u32) {
        for page in (lin..lin + len).step_by(PAGE as usize) {
            let at = match cpu.lin_to_phys(page, true, true) {
                Ok(at) => at,
                Err(_) => {
                    assert!(page_fault(cpu, page, 6), "page {page:08X} came in");
                    cpu.lin_to_phys(page, true, true).unwrap()
                }
            };
            cpu.bus.write_32(at as usize, value(page));
        }
    }

    /// 1 MB of pages, 12 MB of blocks: what doesn't fit goes to the page
    /// file and comes back.
    #[test]
    fn blocks_hold_more_than_the_machine_has() {
        let mut cpu = machine(1 << 20, 12 << 20);
        assert_eq!(free(&cpu), (12 << 20, 12 << 20));
        let a = allocate(&mut cpu, 8 << 20, None).unwrap();
        let b = allocate(&mut cpu, 4 << 20, None).unwrap();
        let base = vm(&cpu).base;
        assert_eq!((a, b), (base, base + (8 << 20)));
        assert_eq!(free(&cpu), (0, 0));
        assert_eq!(allocate(&mut cpu, PAGE, None), None);
        fill(&mut cpu, a, 8 << 20, |p| p ^ 0x5A5A_0000);
        fill(&mut cpu, b, 4 << 20, |p| !p);
        assert!(vm(&cpu).outs > 0, "pages went to the page file");
        for page in (a..a + (8 << 20)).step_by(PAGE as usize) {
            assert_eq!(read_32(&mut cpu, page), page ^ 0x5A5A_0000, "{page:08X}");
        }
        for page in (b..b + (4 << 20)).step_by(PAGE as usize) {
            assert_eq!(read_32(&mut cpu, page + 8), 0);
            assert_eq!(read_32(&mut cpu, page), !page, "{page:08X}");
        }
        assert!(vm(&cpu).ins > 0, "pages came back from the page file");
    }

    #[test]
    fn memory_below_the_blocks_is_where_it_is() {
        let mut cpu = machine(1 << 20, 8 << 20);
        cpu.bus.write_32(0x12_3450, 0xDEAD_BEEF);
        assert_eq!(cpu.lin_to_phys(0x12_3450, false, true), Ok(0x12_3450));
        assert_eq!(read_32(&mut cpu, 0x12_3450), 0xDEAD_BEEF);
        // A card's memory past the machine's.
        assert_eq!(
            map_physical(&mut cpu, 0xE000_0000, 0x10_0000),
            Some(0xE000_0000)
        );
        assert_eq!(cpu.lin_to_phys(0xE00F_F000, true, true), Ok(0xE00F_F000));
        // Not over the blocks' space.
        let base = vm(&cpu).base;
        assert_eq!(map_physical(&mut cpu, base, PAGE), None);
    }

    #[test]
    fn a_block_grows_and_moves_with_what_it_held() {
        let mut cpu = machine(1 << 20, 8 << 20);
        let a = allocate(&mut cpu, 2 * PAGE, None).unwrap();
        let b = allocate(&mut cpu, PAGE, None).unwrap();
        write_16(&mut cpu, a + PAGE, 0x5678);
        write_16(&mut cpu, a + PAGE + 2, 0x1234);
        // b is in the way: a moves.
        let moved = resize(&mut cpu, a, 2 * PAGE, 3 << 20).unwrap();
        assert!(moved > b);
        assert_eq!(read_32(&mut cpu, moved + PAGE), 0x1234_5678);
        assert_eq!(read_32(&mut cpu, moved + 2 * PAGE), 0);
        // Its old place is free again, and the page there gone.
        assert!(cpu.lin_to_phys(a + PAGE, false, true).is_err());
        assert_eq!(allocate(&mut cpu, 2 * PAGE, None), Some(a));
        // Shrinking keeps it where it is.
        assert_eq!(resize(&mut cpu, moved, 3 << 20, PAGE), Some(moved));
        assert_eq!(read_32(&mut cpu, moved), 0);
        assert_eq!(free(&cpu).1, (8 << 20) - 4 * PAGE);
        release(&mut cpu, moved);
        assert_eq!(free(&cpu).1, (8 << 20) - 3 * PAGE);
    }

    #[test]
    fn locked_pages_stay_in_memory() {
        let mut cpu = machine(1 << 20, 8 << 20);
        let a = allocate(&mut cpu, PAGE, None).unwrap();
        assert!(lock(&mut cpu, a, PAGE, true));
        let b = allocate(&mut cpu, 4 << 20, None).unwrap();
        fill(&mut cpu, b, 4 << 20, |p| p);
        fill(&mut cpu, b, 4 << 20, |p| p + 1);
        assert!(vm(&cpu).present.contains_key(&a));
        assert!(lock(&mut cpu, a, PAGE, false));
        fill(&mut cpu, b, 4 << 20, |p| p + 2);
        fill(&mut cpu, b, 4 << 20, |p| p + 3);
        assert!(!vm(&cpu).present.contains_key(&a));
    }

    /// A lock of more than fits in memory at once fails and locks
    /// nothing; one of less brings every page of it in, though some were
    /// in the page file, and keeps them there.
    #[test]
    fn a_lock_takes_all_its_pages_or_none() {
        let mut cpu = machine(1 << 20, 8 << 20);
        let a = allocate(&mut cpu, 2 << 20, None).unwrap();
        fill(&mut cpu, a, 2 << 20, |p| p);
        assert!(!lock(&mut cpu, a, 2 << 20, true));
        assert!(vm(&cpu).locks.is_empty());
        let half = 512 << 10;
        assert!(lock(&mut cpu, a, half, true));
        let held = vm(&cpu);
        assert!(
            (a..a + half)
                .step_by(PAGE as usize)
                .all(|p| held.present.contains_key(&p))
        );
        fill(&mut cpu, a + half, (2 << 20) - half, |p| p + 1);
        for page in (a..a + half).step_by(PAGE as usize) {
            assert!(
                cpu.lin_to_phys(page, false, true).is_ok(),
                "{page:08X} stayed"
            );
            assert_eq!(read_32(&mut cpu, page), page);
        }
    }

    /// Without virtual memory any physical range maps to itself, up to the
    /// end of the 4 GB; with it, one past 4 GB doesn't.
    #[test]
    fn physical_memory_maps_up_to_4_gb() {
        let mut cpu = Cpu::new(std::path::PathBuf::from("."));
        assert_eq!(
            map_physical(&mut cpu, 0xFFF0_0000, 0x10_0000),
            Some(0xFFF0_0000)
        );
        let mut cpu = machine(1 << 20, 8 << 20);
        assert_eq!(
            map_physical(&mut cpu, 0xFFF0_0000, 0x10_0000),
            Some(0xFFF0_0000)
        );
        assert_eq!(cpu.lin_to_phys(0xFFFF_F000, false, true), Ok(0xFFFF_F000));
        assert_eq!(map_physical(&mut cpu, 0xFFF0_0000, 0x10_1000), None);
    }

    #[test]
    fn without_virtual_memory_blocks_are_extended_memory() {
        let mut cpu = Cpu::new(std::path::PathBuf::from("."));
        let end = cpu.bus.ram().len() as u32;
        let a = allocate(&mut cpu, 0x2000, None).unwrap();
        assert!((0x10_0000..end).contains(&a));
        assert_eq!(phys(&mut cpu, a + 5), a + 5);
        assert!(free(&cpu).1 < end);
    }
}
