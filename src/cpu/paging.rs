//! Paging: linear to physical translation through the two-level page
//! tables at CR3, or the page directory alone for the Pentium's 4 MB
//! pages, the TLB that caches it, and page faults.

use super::fault::{CpuResult, Fault};
use super::{CR0_PG, CR0_WP, CR4_PSE, Cpu, CpuModel};

/// Page table entry bits.
const PTE_P: u32 = 0x001;
const PTE_RW: u32 = 0x002;
const PTE_US: u32 = 0x004;
const PTE_A: u32 = 0x020;
const PTE_D: u32 = 0x040;
/// A page directory entry that maps a 4 MB page itself (with CR4.PSE).
const PDE_PS: u32 = 0x080;

/// #PF error code bits.
const PF_PROTECTION: u32 = 0x1;
const PF_WRITE: u32 = 0x2;
const PF_USER: u32 = 0x4;

const TLB_ENTRIES: usize = 1024;

/// A translation, valid for reads when `read_tag` is the linear page number
/// + 1, and for writes when `write_tag` is: a page that may not be written,
/// or whose dirty bit isn't set yet, has a write tag of 0, so writes to it
/// walk the page tables. `repr(C)`, and 32 bytes: the dynamic recompiler's
/// code looks translations up itself (see `layout`), at the linear address
/// shifted right by 7 and masked.
#[derive(Clone, Copy)]
#[repr(C, align(32))]
pub(crate) struct TlbEntry {
    read_tag: u32,
    write_tag: u32,
    /// Physical address of the page.
    phys: u32,
    /// For the x86-64 recompiler's code, where the page is plain RAM (see
    /// `Bus::is_plain_ram`): the linear address of the page, for reads and
    /// for writes as the tags allow them, and what to add to a linear
    /// address in it for the physical one. Elsewhere the tags are 1, which
    /// no page's address is, so the code takes its slow path.
    jit_read: u32,
    jit_write: u32,
    jit_delta: u32,
}

const EMPTY: TlbEntry = TlbEntry { read_tag: 0, write_tag: 0, phys: 0, jit_read: 1, jit_write: 1, jit_delta: 0 };
const _: () = assert!(std::mem::size_of::<TlbEntry>() == 32);

/// Where an entry's fields are, its size, and the entries in each set,
/// for `layout`.
pub(crate) const TLB_READ_TAG: usize = std::mem::offset_of!(TlbEntry, read_tag);
pub(crate) const TLB_WRITE_TAG: usize = std::mem::offset_of!(TlbEntry, write_tag);
pub(crate) const TLB_PHYS: usize = std::mem::offset_of!(TlbEntry, phys);
pub(crate) const TLB_JIT_READ: usize = std::mem::offset_of!(TlbEntry, jit_read);
pub(crate) const TLB_JIT_WRITE: usize = std::mem::offset_of!(TlbEntry, jit_write);
pub(crate) const TLB_JIT_DELTA: usize = std::mem::offset_of!(TlbEntry, jit_delta);
pub(crate) const TLB_ENTRY_SIZE: usize = std::mem::size_of::<TlbEntry>();
pub(crate) const TLB_SET: usize = TLB_ENTRIES;
/// Where the entries are in the TLB.
pub(crate) const TLB_ENTRIES_AT: usize = std::mem::offset_of!(Tlb, entries);

/// Translations the CPU has walked the page tables for, direct-mapped by
/// linear page number, in two sets: for accesses at privilege level 3,
/// which the pages' user bits restrict, and for the others. Like the
/// hardware's, it is only flushed by a CR3 load, a change of CR0.PG or WP,
/// INVLPG and task switches, so a program must flush it after changing
/// page tables, as on a real 386.
pub struct Tlb {
    /// In the CPU itself: the x86-64 recompiler's code reaches them from
    /// its address (`layout::TLB`).
    entries: [TlbEntry; 2 * TLB_ENTRIES],
    /// Counts flushes, full or of one page: a translation kept elsewhere
    /// (the execution loop's code window) holds while it doesn't change.
    pub epoch: u32,
    /// Whether an entry has come from a 4 MB page since the last flush.
    /// The entries are of 4 KB pieces of it.
    large: bool,
}

impl Default for Tlb {
    fn default() -> Self {
        Self { entries: [EMPTY; 2 * TLB_ENTRIES], epoch: 0, large: false }
    }
}

impl Tlb {
    pub fn flush(&mut self) {
        self.entries.fill(EMPTY);
        self.epoch = self.epoch.wrapping_add(1);
        self.large = false;
    }

    /// Drop the translations of the page holding `lin` (INVLPG). Where 4 MB
    /// pages may be cached, that page may be one, so the pieces of all of
    /// the 4 MB around `lin` go.
    pub fn flush_page(&mut self, lin: u32) {
        self.epoch = self.epoch.wrapping_add(1);
        let page = lin >> 12;
        if self.large {
            for e in self.entries.iter_mut() {
                if e.read_tag != 0 && (e.read_tag - 1) >> 10 == page >> 10 {
                    *e = EMPTY;
                }
            }
            return;
        }
        for set in 0..2 {
            let e = &mut self.entries[set * TLB_ENTRIES + page as usize % TLB_ENTRIES];
            if e.read_tag == page + 1 {
                *e = EMPTY;
            }
        }
    }

    #[inline(always)]
    fn slot(page: u32, user: bool) -> usize {
        (user as usize) * TLB_ENTRIES + page as usize % TLB_ENTRIES
    }

    /// The translation of linear page `page` for privilege level 3 or the
    /// others (`user`): its physical page, if the TLB has it.
    #[cfg_attr(not(dynrec), allow(dead_code))]
    pub(crate) fn lookup(&self, page: u32, user: bool) -> Option<u32> {
        let e = self.entries[Self::slot(page, user)];
        (e.read_tag == page + 1).then_some(e.phys)
    }

    /// The entries, supervisor then user, `TLB_ENTRIES` each, for the
    /// dynamic recompiler's code, while the CPU doesn't move.
    #[cfg_attr(not(dynrec), allow(dead_code))]
    pub(crate) fn entries_ptr(&self) -> *const TlbEntry {
        self.entries.as_ptr()
    }
}

/// How an access walks the page tables: those at `cr3`, at privilege
/// level 3 (`user`) or not, with CR0.WP (486) protecting read-only pages
/// from supervisor writes, and with CR4.PSE (Pentium) 4 MB pages. The
/// BIOS's services reach memory so too while paging is on (`Bus::guest_*`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GuestPaging {
    pub cr3: u32,
    pub user: bool,
    pub write_protect: bool,
    pub pse: bool,
}

/// What a walk of the page tables found for an access that they allow.
pub struct Walked {
    /// The physical address of the 4 KB page, of a 4 MB one the piece of
    /// it the access is in.
    pub page: u32,
    user_ok: bool,
    write_ok: bool,
    /// The page's dirty bit, after the access.
    dirty: bool,
    /// Whether it is a 4 MB page.
    large: bool,
}

/// Whether the page tables allow an access. Supervisor code may write
/// read-only pages, unless CR0.WP (486) says otherwise.
#[inline(always)]
fn allows(user_ok: bool, write_ok: bool, write: bool, paging: GuestPaging) -> bool {
    if paging.user && !user_ok {
        return false;
    }
    !write || write_ok || (!paging.user && !paging.write_protect)
}

/// Walk the page tables for an access to `lin` as the processor does,
/// setting the accessed bits, and the dirty bit for a write: the page it
/// reaches, or the page fault's error code.
pub fn walk_tables(bus: &mut crate::bus::Bus, paging: GuestPaging, lin: u32, write: bool) -> Result<Walked, u32> {
    let mut error = if write { PF_WRITE } else { 0 } | if paging.user { PF_USER } else { 0 };
    let a20 = bus.a20_mask();
    let pde_addr = (((paging.cr3 & 0xFFFF_F000) | ((lin >> 20) & 0xFFC)) & a20) as usize;
    let pde = bus.read_32(pde_addr);
    if pde & PTE_P == 0 {
        return Err(error);
    }
    if paging.pse && pde & PDE_PS != 0 {
        // A 4 MB page: the directory entry has its address, protection
        // and accessed and dirty bits.
        let (user_ok, write_ok) = (pde & PTE_US != 0, pde & PTE_RW != 0);
        if !allows(user_ok, write_ok, write, paging) {
            return Err(error | PF_PROTECTION);
        }
        let new_pde = pde | PTE_A | if write { PTE_D } else { 0 };
        if new_pde != pde {
            bus.write_32(pde_addr, new_pde);
        }
        let page = (pde & 0xFFC0_0000) | (lin & 0x003F_F000);
        return Ok(Walked { page, user_ok, write_ok, dirty: new_pde & PTE_D != 0, large: true });
    }
    let pte_addr = (((pde & 0xFFFF_F000) | ((lin >> 10) & 0xFFC)) & a20) as usize;
    let pte = bus.read_32(pte_addr);
    if pte & PTE_P == 0 {
        return Err(error);
    }
    let user_ok = pde & pte & PTE_US != 0;
    let write_ok = pde & pte & PTE_RW != 0;
    if !allows(user_ok, write_ok, write, paging) {
        error |= PF_PROTECTION;
        return Err(error);
    }
    if pde & PTE_A == 0 {
        bus.write_32(pde_addr, pde | PTE_A);
    }
    let new_pte = pte | PTE_A | if write { PTE_D } else { 0 };
    if new_pte != pte {
        bus.write_32(pte_addr, new_pte);
    }
    Ok(Walked { page: pte & 0xFFFF_F000, user_ok, write_ok, dirty: new_pte & PTE_D != 0, large: false })
}

impl Cpu {
    /// Physical address of a linear one with paging off: the A20 gate
    /// decides whether address line 20 follows or is held at 0.
    #[inline(always)]
    pub fn translate(&self, lin: u32) -> u32 {
        lin & self.bus.a20_mask()
    }

    /// Physical address of an access at `lin`: through the page tables
    /// when paging is on. `user` is an access at privilege level 3; system
    /// structures (descriptor tables, the TSS) are accessed as supervisor.
    #[inline(always)]
    pub fn lin_to_phys(&mut self, lin: u32, write: bool, user: bool) -> CpuResult<u32> {
        if self.cr0 & CR0_PG == 0 {
            return Ok(self.translate(lin));
        }
        let page = lin >> 12;
        let e = self.tlb.entries[Tlb::slot(page, user)];
        let tag = if write { e.write_tag } else { e.read_tag };
        if tag == page + 1 {
            return Ok(self.translate(e.phys | (lin & 0xFFF)));
        }
        self.walk(lin, write, user)
    }

    pub(crate) fn write_protect(&self) -> bool {
        self.model >= CpuModel::I486 && self.cr0 & CR0_WP != 0
    }

    /// Whether page directory entries can map 4 MB pages (CR4.PSE).
    pub(crate) fn pse(&self) -> bool {
        self.model >= CpuModel::Pentium && self.cr4 & CR4_PSE != 0
    }

    /// How the processor's accesses at privilege level 3 (`user`) or the
    /// others walk the page tables.
    pub(crate) fn guest_paging(&self, user: bool) -> GuestPaging {
        GuestPaging { cr3: self.cr3, user, write_protect: self.write_protect(), pse: self.pse() }
    }

    /// Walk the page tables for `lin`, setting the accessed and dirty bits
    /// and filling the TLB, or raise a page fault with CR2 = `lin`.
    fn walk(&mut self, lin: u32, write: bool, user: bool) -> CpuResult<u32> {
        let paging = self.guest_paging(user);
        let walked = match walk_tables(&mut self.bus, paging, lin, write) {
            Ok(walked) => walked,
            Err(error) => return Err(self.page_fault(lin, error)),
        };
        let page = lin >> 12;
        self.tlb.large |= walked.large;
        // Writes go through the TLB only once the page is dirty, so the
        // first write to it still sets the bit.
        let writable = walked.dirty && allows(walked.user_ok, walked.write_ok, true, paging);
        let plain = self.bus.is_plain_ram(walked.page as usize, 0x1000);
        let at = lin & !0xFFF;
        self.tlb.entries[Tlb::slot(page, user)] = TlbEntry {
            read_tag: page + 1,
            write_tag: if writable { page + 1 } else { 0 },
            phys: walked.page,
            jit_read: if plain { at } else { 1 },
            jit_write: if plain && writable { at } else { 1 },
            jit_delta: walked.page.wrapping_sub(at),
        };
        Ok(self.translate(walked.page | (lin & 0xFFF)))
    }

    fn page_fault(&mut self, lin: u32, error: u32) -> Fault {
        self.cr2 = lin;
        Fault::pf(error)
    }

    /// The physical address `lin` maps to, without faulting or changing
    /// anything, for debuggers. None for an unmapped page.
    pub fn peek_translate(&self, lin: u32) -> Option<u32> {
        if self.cr0 & CR0_PG == 0 {
            return Some(self.translate(lin));
        }
        let pde = self.bus.read_32(self.translate((self.cr3 & 0xFFFF_F000) | ((lin >> 20) & 0xFFC)) as usize);
        if pde & PTE_P == 0 {
            return None;
        }
        if self.pse() && pde & PDE_PS != 0 {
            return Some(self.translate((pde & 0xFFC0_0000) | (lin & 0x003F_FFFF)));
        }
        let pte = self.bus.read_32(self.translate((pde & 0xFFFF_F000) | ((lin >> 10) & 0xFFC)) as usize);
        if pte & PTE_P == 0 {
            return None;
        }
        Some(self.translate((pte & 0xFFFF_F000) | (lin & 0xFFF)))
    }

    /// Whether conventional memory, from the first MCB up, is at the
    /// physical addresses its linear ones name, as DOS reaches it here: so
    /// without paging, and in Windows' System VM, but not in the DOS
    /// machines of its 386 enhanced mode, which have memory of their own.
    pub fn conventional_memory_in_place(&self) -> bool {
        let first = (crate::mcb::FIRST_MCB_SEG as u32 * 16) >> 12;
        let end = crate::mcb::END_OF_CONVENTIONAL as u32 * 16 >> 12;
        (first..end).all(|page| self.peek_translate(page << 12) == Some(page << 12))
    }

    /// The page directory and page table entries for `lin`, for debuggers:
    /// (PDE address, PDE, PTE address and PTE if the table is present; a
    /// 4 MB page has none).
    pub fn page_walk(&self, lin: u32) -> (u32, u32, Option<(u32, u32)>) {
        let pde_addr = self.translate((self.cr3 & 0xFFFF_F000) | ((lin >> 20) & 0xFFC));
        let pde = self.bus.read_32(pde_addr as usize);
        if pde & PTE_P == 0 || (self.pse() && pde & PDE_PS != 0) {
            return (pde_addr, pde, None);
        }
        let pte_addr = self.translate((pde & 0xFFFF_F000) | ((lin >> 10) & 0xFFC));
        (pde_addr, pde, Some((pte_addr, self.bus.read_32(pte_addr as usize))))
    }
}
