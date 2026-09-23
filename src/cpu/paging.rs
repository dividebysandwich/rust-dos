//! Paging: linear to physical translation through the two-level page
//! tables at CR3, the TLB that caches it, and page faults.

use super::fault::{CpuResult, Fault};
use super::{CR0_PG, CR0_WP, Cpu, CpuModel};

/// Page table entry bits.
const PTE_P: u32 = 0x001;
const PTE_RW: u32 = 0x002;
const PTE_US: u32 = 0x004;
const PTE_A: u32 = 0x020;
const PTE_D: u32 = 0x040;

/// #PF error code bits.
const PF_PROTECTION: u32 = 0x1;
const PF_WRITE: u32 = 0x2;
const PF_USER: u32 = 0x4;

const TLB_ENTRIES: usize = 1024;

/// A translation, valid for reads when `read_tag` is the linear page number
/// + 1, and for writes when `write_tag` is: a page that may not be written,
/// or whose dirty bit isn't set yet, has a write tag of 0, so writes to it
/// walk the page tables.
#[derive(Clone, Copy)]
struct TlbEntry {
    read_tag: u32,
    write_tag: u32,
    /// Physical address of the page.
    phys: u32,
}

const EMPTY: TlbEntry = TlbEntry { read_tag: 0, write_tag: 0, phys: 0 };

/// Translations the CPU has walked the page tables for, direct-mapped by
/// linear page number, in two sets: for accesses at privilege level 3,
/// which the pages' user bits restrict, and for the others. Like the
/// hardware's, it is only flushed by a CR3 load, a change of CR0.PG or WP,
/// INVLPG and task switches, so a program must flush it after changing
/// page tables, as on a real 386.
pub struct Tlb {
    entries: Box<[TlbEntry]>,
}

impl Default for Tlb {
    fn default() -> Self {
        Self { entries: vec![EMPTY; 2 * TLB_ENTRIES].into_boxed_slice() }
    }
}

impl Tlb {
    pub fn flush(&mut self) {
        self.entries.fill(EMPTY);
    }

    /// Drop the translations of the page holding `lin` (INVLPG).
    pub fn flush_page(&mut self, lin: u32) {
        let page = lin >> 12;
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

    /// Whether the page tables allow an access. Supervisor code may write
    /// read-only pages, unless CR0.WP (486) says otherwise.
    #[inline(always)]
    fn allows(&self, user_ok: bool, write_ok: bool, write: bool, user: bool) -> bool {
        if user && !user_ok {
            return false;
        }
        !write || write_ok || (!user && !self.write_protect())
    }

    fn write_protect(&self) -> bool {
        self.model == CpuModel::I486 && self.cr0 & CR0_WP != 0
    }

    /// Walk the page tables for `lin`, setting the accessed and dirty bits
    /// and filling the TLB, or raise a page fault with CR2 = `lin`.
    fn walk(&mut self, lin: u32, write: bool, user: bool) -> CpuResult<u32> {
        let mut error = if write { PF_WRITE } else { 0 } | if user { PF_USER } else { 0 };
        let pde_addr = self.translate((self.cr3 & 0xFFFF_F000) | ((lin >> 20) & 0xFFC)) as usize;
        let pde = self.bus.read_32(pde_addr);
        if pde & PTE_P == 0 {
            return Err(self.page_fault(lin, error));
        }
        let pte_addr = self.translate((pde & 0xFFFF_F000) | ((lin >> 10) & 0xFFC)) as usize;
        let pte = self.bus.read_32(pte_addr);
        if pte & PTE_P == 0 {
            return Err(self.page_fault(lin, error));
        }
        let user_ok = pde & pte & PTE_US != 0;
        let write_ok = pde & pte & PTE_RW != 0;
        if !self.allows(user_ok, write_ok, write, user) {
            error |= PF_PROTECTION;
            return Err(self.page_fault(lin, error));
        }
        if pde & PTE_A == 0 {
            self.bus.write_32(pde_addr, pde | PTE_A);
        }
        let new_pte = pte | PTE_A | if write { PTE_D } else { 0 };
        if new_pte != pte {
            self.bus.write_32(pte_addr, new_pte);
        }
        let page = lin >> 12;
        let phys = pte & 0xFFFF_F000;
        // Writes go through the TLB only once the page is dirty, so the
        // first write to it still sets the bit.
        let writable = new_pte & PTE_D != 0 && self.allows(user_ok, write_ok, true, user);
        self.tlb.entries[Tlb::slot(page, user)] =
            TlbEntry { read_tag: page + 1, write_tag: if writable { page + 1 } else { 0 }, phys };
        Ok(self.translate(phys | (lin & 0xFFF)))
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
        let pte = self.bus.read_32(self.translate((pde & 0xFFFF_F000) | ((lin >> 10) & 0xFFC)) as usize);
        if pte & PTE_P == 0 {
            return None;
        }
        Some(self.translate((pte & 0xFFFF_F000) | (lin & 0xFFF)))
    }

    /// The page directory and page table entries for `lin`, for debuggers:
    /// (PDE address, PDE, PTE address and PTE if the table is present).
    pub fn page_walk(&self, lin: u32) -> (u32, u32, Option<(u32, u32)>) {
        let pde_addr = self.translate((self.cr3 & 0xFFFF_F000) | ((lin >> 20) & 0xFFC));
        let pde = self.bus.read_32(pde_addr as usize);
        if pde & PTE_P == 0 {
            return (pde_addr, pde, None);
        }
        let pte_addr = self.translate((pde & 0xFFFF_F000) | ((lin >> 10) & 0xFFC));
        (pde_addr, pde, Some((pte_addr, self.bus.read_32(pte_addr as usize))))
    }
}
