//! Values the Cheats page froze: put back before every frame while the
//! program runs, and let go when it ends.

use rust_dos::cheats::{Freeze, Width};
use rust_dos::cpu::Cpu;
use std::path::PathBuf;

#[test]
fn freezes_are_put_back_and_let_go_when_the_program_ends() {
    let mut cpu = Cpu::new(PathBuf::from("."));
    cpu.load_shell();
    cpu.bus.freezes.push(Freeze { addr: 0x20010, width: Width::Word, value: 999 });
    cpu.bus.write_16(0x20010, 3);
    cpu.bus.apply_freezes();
    assert_eq!(cpu.bus.read_16(0x20010), 999);
    // The next program has other values at those addresses.
    cpu.load_shell();
    assert!(cpu.bus.freezes.is_empty());
    cpu.bus.write_16(0x20010, 3);
    cpu.bus.apply_freezes();
    assert_eq!(cpu.bus.read_16(0x20010), 3);
}
