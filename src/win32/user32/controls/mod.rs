//! The system's control classes: Button, Static, Edit, ListBox,
//! ComboBox, ScrollBar and MDIClient, each in its module.
//!
//! What a control keeps is in its window's `control`. A control's
//! window procedure that calls into the program (a notification to its
//! parent, WM_CTLCOLOR*) does it as a task, the controls' task
//! (`TASKS` 13): its first variable says which module's steps it runs,
//! the module in its high byte (`start`).

use crate::cpu::Cpu;
use crate::win32::task::{self, Next, Task};

pub mod button;
pub mod combobox;
pub mod edit;
pub mod listbox;
pub mod mdi;
pub mod scrollbar;
pub mod statik;

pub use button::button_proc;
pub use combobox::combobox_proc;
pub use edit::edit_proc;
pub use listbox::listbox_proc;
pub use mdi::{create_mdi_window, def_frame_proc, def_mdi_child_proc, mdiclient_proc};
pub use scrollbar::scrollbar_proc;
pub use statik::static_proc;

/// What a system class's window procedure keeps of a window.
#[derive(Clone, Debug, Default)]
pub struct Control {
    pub button: button::Button,
    pub stat: statik::Static,
    pub edit: edit::Edit,
    pub list: listbox::ListBox,
    pub combo: combobox::Combo,
    pub mdi: mdi::Mdi,
}

/// The modules, as `start` and `step` number them.
pub const BUTTON: u32 = 0;
pub const STATIC: u32 = 1;
pub const EDIT: u32 = 2;
pub const LISTBOX: u32 = 3;
pub const COMBOBOX: u32 = 4;
pub const SCROLLBAR: u32 = 5;
pub const MDI: u32 = 6;

const TASK_CONTROLS: u16 = 13;

/// Start the controls' task for the module `module`: `kind` (below
/// 100h) is the module's own, the rest of `vars` follow it.
pub fn start(cpu: &mut Cpu, module: u32, kind: u32, vars: Vec<u32>, scratch: u32) -> Option<u32> {
    let mut all = vec![module << 8 | kind];
    all.extend(vars);
    task::start(cpu, TASK_CONTROLS, all, scratch)
}

/// The kind `start` was given, for the module's steps.
pub fn kind(t: &Task) -> u32 {
    t.var(0) & 0xFF
}

/// A window of one of the classes is made: CreateWindowExA calls this
/// before WM_NCCREATE.
pub fn init(cpu: &mut Cpu, hwnd: u32) {
    scrollbar::init(cpu, hwnd);
}

/// The controls' task.
pub fn step(cpu: &mut Cpu, t: &mut Task, value: u32) -> Next {
    match t.var(0) >> 8 {
        BUTTON => button::step(cpu, t, value),
        STATIC => statik::step(cpu, t, value),
        EDIT => edit::step(cpu, t, value),
        LISTBOX => listbox::step(cpu, t, value),
        COMBOBOX => combobox::step(cpu, t, value),
        SCROLLBAR => scrollbar::step(cpu, t, value),
        _ => mdi::step(cpu, t, value),
    }
}

crate::state_fields!(Control {
    button,
    stat,
    edit,
    list,
    combo,
    mdi
});
