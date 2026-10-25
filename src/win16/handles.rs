//! The handles 16-bit programs and the 32-bit functions pass each other.
//! Windows and GDI objects have handles below 10000h already, which both
//! use as they are. Icons, cursors, menus and accelerator tables have
//! USER32 handles above that, kept 4 apart: a 16-bit handle is the low
//! word with a mark in its low bits (2 for an icon or cursor, 1 for a
//! menu, 3 for an accelerator table), so none is 0. An instance or module
//! handle stands for the module's image of resources (resource.rs).

use crate::cpu::Cpu;

const ICON_BASE: u32 = 0x0001_0000;
const MENU_BASE: u32 = 0x0002_0000;
const ACCEL_BASE: u32 = 0x0003_0000;
const ICON_MARK: u16 = 2;
const MENU_MARK: u16 = 1;
const ACCEL_MARK: u16 = 3;

/// A 16-bit program's icon or cursor handle as USER32 has it.
pub fn icon32(h: u16) -> u32 {
    if h & 3 == ICON_MARK {
        ICON_BASE | (h & !3) as u32
    } else {
        h as u32
    }
}

/// USER32's icon or cursor handle as 16-bit programs have it.
pub fn icon16(h: u32) -> u16 {
    if h & 0xFFFF_0000 == ICON_BASE {
        h as u16 | ICON_MARK
    } else {
        h as u16
    }
}

pub fn accel32(h: u16) -> u32 {
    if h & 3 == ACCEL_MARK {
        ACCEL_BASE | (h & !3) as u32
    } else {
        h as u32
    }
}

pub fn accel16(h: u32) -> u16 {
    if h & 0xFFFF_0000 == ACCEL_BASE {
        h as u16 | ACCEL_MARK
    } else {
        h as u16
    }
}

pub fn menu32(h: u16) -> u32 {
    if h & 3 == MENU_MARK {
        MENU_BASE | (h & !3) as u32
    } else {
        h as u32
    }
}

pub fn menu16(h: u32) -> u16 {
    if h & 0xFFFF_0000 == MENU_BASE {
        h as u16 | MENU_MARK
    } else {
        h as u16
    }
}

/// The base of the image of resources of the module whose instance or
/// module handle `h` is, made now if need be; 0 stays 0, and a handle of
/// no module is passed as it is.
pub fn instance32(cpu: &mut Cpu, h: u16) -> u32 {
    if h == 0 {
        return 0;
    }
    match super::module_index(cpu, h) {
        Some(i) => match super::resource::image32(cpu, i) {
            0 => h as u32,
            base => base,
        },
        None => h as u32,
    }
}

/// The instance handle a 32-bit function's `h` stands for: the running
/// task's instance for its own module's image, a library's for its, the
/// module's handle for one without an instance.
pub fn instance16(cpu: &Cpu, h: u32) -> u16 {
    if h < 0x1_0000 {
        return h as u16;
    }
    let win16 = &cpu.bus.win32.win16;
    let Some(module) = win16.modules.iter().find(|m| m.resources32 == h) else {
        return 0;
    };
    if let Some(t) = super::current_task(cpu) {
        let task = &win16.tasks[t];
        if task.module == module.handle {
            return task.instance;
        }
    }
    if let Some(task) = win16.tasks.iter().find(|t| t.module == module.handle) {
        return task.instance;
    }
    if module.dgroup != 0 {
        module.dgroup
    } else {
        module.handle
    }
}
