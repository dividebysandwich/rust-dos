//! INT 2Fh — DOS multiplex interrupt.
//!
//! Only the XMS driver (AX=43xxh), the MSCDEX CD-ROM extensions (AH=15h,
//! see `mscdex.rs`), the DOS internal calls that find DOS's data segment
//! and file tables (AX=1203h, 1216h, 1220h) and DOS's interface for
//! Windows' DOSMGR (AX=1607h BX=0015h) are there. Every other function
//! leaves the registers untouched, which callers read as "not installed"
//! (AL stays 00h for the usual install checks, AX stays 1687h for the DPMI
//! check, ...).

use crate::cpu::{Cpu, CpuFlags};
use crate::dos_data;
use crate::dos_files;

/// What DOSMGR's calls return in AX and DX when the DOS kernel did them.
const DOSMGR_AX: u16 = 0xB97C;
const DOSMGR_DX: u16 = 0xA2AB;

/// ES:DI at the linear address `at`, or CF set for none.
fn point_es_di(cpu: &mut Cpu, at: Option<usize>) {
    if let Some(at) = at {
        cpu.set_es((at >> 4) as u16);
        cpu.set_di((at & 0x0F) as u16);
    }
    cpu.set_cpu_flag(CpuFlags::CF, at.is_none());
}

pub fn handle(cpu: &mut Cpu) {
    match cpu.ax() {
        // XMS driver installation check: AL=80h.
        0x4300 => cpu.set_ax(0x4380),
        // XMS driver entry point in ES:BX.
        0x4310 => {
            cpu.set_es(0xF000);
            cpu.set_bx(crate::bios::XMS_ENTRY);
        }
        // DOS's data segment in DS.
        0x1203 => cpu.set_ds(dos_data::SEGMENT),
        // Windows' DOS manager asking about DOS's data, which the DOS 5
        // kernel answers itself.
        0x1607 if cpu.bx() == 0x0015 => dosmgr(cpu),
        // The System File Table entry BX in ES:DI.
        0x1216 => {
            let sft = cpu.bx();
            point_es_di(cpu, (sft < crate::disk::FILES).then(|| dos_files::entry_address(sft)));
        }
        // The slot of handle BX in the running process's job file table
        // in ES:DI, which holds the handle's System File Table entry.
        0x1220 => {
            let at = dos_files::slot_address(&cpu.bus, cpu.current_psp, cpu.bx());
            point_es_di(cpu, at);
        }
        _ if cpu.get_ah() == 0x15 => super::mscdex::handle(cpu, cpu.get_al()),
        // Everything else (DPMI 1687h, Windows 16xxh, ...) is not installed:
        // the registers come back unchanged.
        _ => {}
    }
}

/// The DOSMGR interface of the MS-DOS 5 kernel (INT 2Fh AX=1607h BX=0015h),
/// function CX.
fn dosmgr(cpu: &mut Cpu) {
    match cpu.cx() {
        // Whether DOS instances its data itself: yes, IO.SYS at the default
        // 0070h, and the patch table in ES:BX.
        0x0000 => {
            cpu.set_cx(0x0001);
            cpu.set_dx(0x0000);
            cpu.set_es(dos_data::SEGMENT);
            cpu.set_bx(dos_data::DOSMGR_PATCHES);
        }
        // Patch DOS for the requests in DX. DOS services run in one step
        // here, which is what each patch achieves: they are all applied.
        0x0001 => {
            cpu.set_bx(cpu.dx());
            cpu.set_ax(DOSMGR_AX);
            cpu.set_dx(DOSMGR_DX);
        }
        // Take the patches out again.
        0x0002 => cpu.set_cx(0x0000),
        // The size of a DOS data structure: bit 0 of DX, a current
        // directory structure.
        0x0003 if cpu.dx() == 0x0001 => {
            cpu.set_cx(0x0058);
            cpu.set_ax(DOSMGR_AX);
            cpu.set_dx(DOSMGR_DX);
        }
        0x0003 => cpu.set_cx(0x0000),
        // Which structures are instanced: not answered, as DOS 5 doesn't.
        0x0004 => cpu.set_dx(0x0000),
        // The size of the device driver at ES: none is loaded.
        0x0005 => {
            cpu.set_ax(0x0000);
            cpu.set_dx(0x0000);
        }
        _ => {}
    }
}
