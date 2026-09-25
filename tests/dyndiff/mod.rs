//! Lockstep comparison of two machines: the same program runs on both in
//! batches of emulated time, and after every batch everything a program
//! can see or change must be equal: the registers, the system registers,
//! the instruction count, RAM, video memory and the code generations. With
//! one machine on the interpreter and the other on the dynamic recompiler,
//! this checks that the recompiler is exact.

#![allow(dead_code)]

use rust_dos::cpu::{Cpu, CpuSnapshot};
use rust_dos::exec::{ExecHook, NoHook, StopReason, run_batch};
/// The registers and counters compared after each batch (memory is
/// compared directly, see `memory_difference`).
#[derive(Debug, PartialEq)]
pub struct State {
    pub regs: CpuSnapshot,
    pub cr0: u32,
    pub cr2: u32,
    pub cr3: u32,
    pub cpl: u8,
    pub cpu_state: String,
    pub irq_shadow: bool,
    pub icount: u64,
    pub executed: u64,
    pub exceptions: u64,
}

impl State {
    pub fn of(cpu: &Cpu) -> Self {
        let bus = &cpu.bus;
        State {
            regs: cpu.snapshot(),
            cr0: cpu.cr0,
            cr2: cpu.cr2,
            cr3: cpu.cr3,
            cpl: cpu.cpl,
            cpu_state: format!("{:?}", cpu.state),
            irq_shadow: cpu.irq_shadow,
            icount: bus.clock.icount,
            executed: cpu.executed,
            exceptions: cpu.exceptions,
        }
    }

    /// The fields that differ, for the failure message.
    pub fn diff(&self, other: &State) -> String {
        let a = format!("{:#?}", self);
        let b = format!("{:#?}", other);
        a.lines()
            .zip(b.lines())
            .filter(|(x, y)| x != y)
            .map(|(x, y)| format!("  {}  vs  {}", x.trim(), y.trim()))
            .collect::<Vec<_>>()
            .join("\n")
    }
}

/// Where the machines' memories differ, if they do: RAM, the code
/// generations, video memory, the palette and the debug console.
pub fn memory_difference(a: &Cpu, b: &Cpu) -> Option<String> {
    fn first<T: PartialEq + std::fmt::Debug>(name: &str, x: &[T], y: &[T]) -> Option<String> {
        if x == y {
            return None;
        }
        match x.iter().zip(y).position(|(p, q)| p != q) {
            Some(at) => Some(format!("{} differs first at {:X}: {:X?} vs {:X?}", name, at, x[at], y[at])),
            None => Some(format!("{} lengths {} vs {}", name, x.len(), y.len())),
        }
    }
    let (p, q) = (&a.bus, &b.bus);
    first("RAM", p.ram(), q.ram())
        .or_else(|| first("page_gen", &p.page_gen, &q.page_gen))
        .or_else(|| first("VGA memory", &p.vga.vram_graphics, &q.vga.vram_graphics))
        .or_else(|| first("text memory", &p.vga.vram_text, &q.vga.vram_text))
        .or_else(|| first("palette", &p.vga.palette, &q.vga.palette))
        .or_else(|| first("VESA memory", &p.vbe.vram, &q.vbe.vram))
        .or_else(|| first("debug console", &p.debug_console, &q.debug_console))
}

/// Stops a batch before a HLT (which in a batch would wait for the next
/// timer event and go on).
pub struct StopAtHlt;

impl ExecHook for StopAtHlt {
    fn before_exec(&mut self, _cpu: &Cpu, phys_ip: usize, ram: &[u8]) -> bool {
        ram.get(phys_ip) == Some(&0xF4)
    }

    fn per_instruction(&self) -> bool {
        false
    }
}

/// Run one batch of `len` instructions' time, stopping before a HLT with
/// `stop_at_hlt`.
pub fn batch(cpu: &mut Cpu, len: u64, stop_at_hlt: bool) -> StopReason {
    let end = cpu.bus.clock.icount + len;
    cpu.bus.start_batch(end);
    if stop_at_hlt { run_batch(cpu, &mut StopAtHlt, true) } else { run_batch(cpu, &mut NoHook, false) }
}

/// How a lockstep run went: the batches run, and the time each machine
/// took for them.
pub struct Run {
    pub batches: usize,
    pub a_time: std::time::Duration,
    pub b_time: std::time::Duration,
}

/// Run `a` and `b` side by side for `batches` batches of `len`, calling
/// `each` on both before every batch (to type keys, say), and compare them
/// after each. Stops early once the program turned the machine off (EXIT).
/// Returns how it went, or where they diverged.
pub fn lockstep(
    a: &mut Cpu,
    b: &mut Cpu,
    batches: usize,
    len: u64,
    each: impl FnMut(usize, &mut Cpu),
) -> Result<Run, String> {
    lockstep_with(a, b, batches, len, false, each)
}

/// `lockstep`, stopping at the first HLT with `stop_at_hlt` (the batch
/// it comes in ends before it).
pub fn lockstep_with(
    a: &mut Cpu,
    b: &mut Cpu,
    batches: usize,
    len: u64,
    stop_at_hlt: bool,
    mut each: impl FnMut(usize, &mut Cpu),
) -> Result<Run, String> {
    let start = State::of(a);
    let other = State::of(b);
    if start != other {
        return Err(format!("the machines differ before they start:\n{}", start.diff(&other)));
    }
    let mut run = Run { batches, a_time: Default::default(), b_time: Default::default() };
    for n in 0..batches {
        each(n, a);
        each(n, b);
        let started = std::time::Instant::now();
        let ra = batch(a, len, stop_at_hlt);
        run.a_time += started.elapsed();
        let started = std::time::Instant::now();
        let rb = batch(b, len, stop_at_hlt);
        run.b_time += started.elapsed();
        let (sa, sb) = (State::of(a), State::of(b));
        let memory = memory_difference(a, b);
        if ra != rb || sa != sb || memory.is_some() {
            let memory = memory.map_or(String::new(), |m| format!("\n  {}", m));
            return Err(format!(
                "diverged in batch {} (icount {} vs {}), stop {:?} vs {:?}:\n{}{}",
                n,
                sa.icount,
                sb.icount,
                ra,
                rb,
                sa.diff(&sb),
                memory
            ));
        }
        if ra == StopReason::Exit || (stop_at_hlt && ra == StopReason::Paused) {
            run.batches = n + 1;
            return Ok(run);
        }
    }
    Ok(run)
}
