//! Scripts: what a common control's window procedure does when it has
//! to call into the program on its way, as a list of steps that the
//! controls' task (`TASKS`'s `COMCTL32`) runs.
//!
//! The common controls notify their parents with WM_NOTIFY at almost
//! every change, paint through BeginPaint (which sends the window
//! WM_ERASEBKGND, to a procedure the program may have subclassed),
//! UpdateWindow and SetWindowPos, all of which call into the program.
//! No Rust frame may live across such a call (call.rs), so a window
//! procedure that makes one returns a `Script` instead: calls to make
//! one after the other, with the structures they take in scratch space
//! on the stack, and continuations, functions of the control's module
//! that run with what the call before returned and may add steps of
//! their own. A script is plain data in the task's variables, so a
//! state saved while the program runs one has it.

use super::{Module, dispatch_then};
use crate::cpu::Cpu;
use crate::win32::task::{self, Next, Task};

/// An argument of a call.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Arg {
    /// This value.
    V(u32),
    /// The address of this offset in the scratch space.
    S(u32),
    /// What the call before returned.
    Last,
}

/// A step of a script.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Op {
    /// Call the function at `target` (stdcall) with `args`.
    Call { target: u32, args: Vec<Arg> },
    /// Run the continuation `id` of the script's module, with `data`.
    Then { id: u16, data: Vec<u32> },
    /// Write `words` at the offset `at` of the scratch space.
    Write { at: u32, words: Vec<u32> },
    /// The window procedure returns this.
    Return(Arg),
}

/// What a continuation is given.
pub struct Ctx {
    /// The control the script is for.
    pub hwnd: u32,
    /// What the last call returned.
    pub last: u32,
    /// The scratch space's address.
    pub scratch: u32,
    pub data: Vec<u32>,
    /// What the script returns if no step returns: a continuation may
    /// change it.
    pub result: u32,
}

impl Ctx {
    pub fn word(&self, i: usize) -> u32 {
        self.data.get(i).copied().unwrap_or(0)
    }
}

/// A script being made.
#[derive(Clone, Debug, Default)]
pub struct Script {
    pub hwnd: u32,
    pub ops: Vec<Op>,
    /// Bytes of scratch space it takes.
    pub scratch: u32,
    /// What it returns if no step returns.
    pub result: u32,
}

impl Script {
    pub fn new(hwnd: u32) -> Script {
        Script {
            hwnd,
            ..Script::default()
        }
    }

    /// `bytes` bytes of scratch space: their offset.
    pub fn alloc(&mut self, bytes: u32) -> u32 {
        let at = self.scratch;
        self.scratch += bytes.next_multiple_of(4);
        at
    }

    /// Scratch space holding `words`: its offset.
    pub fn data(&mut self, words: &[u32]) -> u32 {
        let at = self.alloc(4 * words.len() as u32);
        self.ops.push(Op::Write {
            at,
            words: words.to_vec(),
        });
        at
    }

    /// Scratch space holding `bytes` and a 0 after them: its offset.
    pub fn string(&mut self, bytes: &[u8]) -> u32 {
        let mut b = bytes.to_vec();
        b.push(0);
        self.data(&words_of(&b))
    }

    pub fn call(&mut self, target: u32, args: &[Arg]) -> &mut Self {
        self.ops.push(Op::Call {
            target,
            args: args.to_vec(),
        });
        self
    }

    /// Call USER32's function `name` with plain values.
    pub fn user(&mut self, name: &str, args: &[u32]) -> &mut Self {
        let args: Vec<Arg> = args.iter().map(|&a| Arg::V(a)).collect();
        self.call(crate::win32::user32::address_of(name), &args)
    }

    /// SendMessageA(hwnd, msg, wp, lp).
    pub fn send(&mut self, hwnd: u32, msg: u32, wp: Arg, lp: Arg) -> &mut Self {
        let target = crate::win32::user32::address_of("SendMessageA");
        self.call(target, &[Arg::V(hwnd), Arg::V(msg), wp, lp])
    }

    pub fn then(&mut self, id: u16, data: &[u32]) -> &mut Self {
        self.ops.push(Op::Then {
            id,
            data: data.to_vec(),
        });
        self
    }

    pub fn ret(&mut self, value: u32) -> &mut Self {
        self.ops.push(Op::Return(Arg::V(value)));
        self
    }

    /// Return what the last call returned.
    pub fn ret_last(&mut self) -> &mut Self {
        self.ops.push(Op::Return(Arg::Last));
        self
    }

    /// Whether running it calls into the program.
    fn calls(&self) -> bool {
        self.ops
            .iter()
            .any(|op| matches!(op, Op::Call { .. } | Op::Then { .. }))
    }
}

/// `bytes` as little-endian words, the last padded with zeros.
pub fn words_of(bytes: &[u8]) -> Vec<u32> {
    bytes
        .chunks(4)
        .map(|c| {
            let mut w = [0u8; 4];
            w[..c.len()].copy_from_slice(c);
            u32::from_le_bytes(w)
        })
        .collect()
}

// How ops are kept in a task's variables: a tag, then its own words.
const T_CALL: u32 = 1;
const T_THEN: u32 = 2;
const T_WRITE: u32 = 3;
const T_RETURN: u32 = 4;

fn arg_kind(a: Arg) -> (u32, u32) {
    match a {
        Arg::V(v) => (0, v),
        Arg::S(o) => (1, o),
        Arg::Last => (2, 0),
    }
}

fn encode(ops: &[Op]) -> Vec<u32> {
    let mut v = Vec::new();
    for op in ops {
        match op {
            Op::Call { target, args } => {
                let kinds = args
                    .iter()
                    .enumerate()
                    .fold(0, |k, (i, &a)| k | arg_kind(a).0 << (2 * i));
                v.extend([T_CALL, *target, args.len() as u32, kinds]);
                v.extend(args.iter().map(|&a| arg_kind(a).1));
            }
            Op::Then { id, data } => {
                v.extend([T_THEN, *id as u32, data.len() as u32]);
                v.extend(data);
            }
            Op::Write { at, words } => {
                v.extend([T_WRITE, *at, words.len() as u32]);
                v.extend(words);
            }
            Op::Return(a) => {
                let (k, value) = arg_kind(*a);
                v.extend([T_RETURN, k, value]);
            }
        }
    }
    v
}

/// The task's variables before the ops: the module, the control, where
/// the next op is, what the last call returned, the default result.
const V_MODULE: usize = 0;
const V_HWND: usize = 1;
const V_PC: usize = 2;
const V_LAST: usize = 3;
const V_RESULT: usize = 4;
const V_OPS: usize = 5;

/// The step a call returns to.
const STEP_RETURNED: u16 = 1;

/// Run `script` for the module `module`: what the window procedure
/// returns, or None when it calls into the program and returns later.
pub fn run(cpu: &mut Cpu, module: Module, script: Script) -> Option<u32> {
    if !script.calls() {
        let mut result = script.result;
        for op in &script.ops {
            if let Op::Return(Arg::V(v)) = op {
                result = *v;
                break;
            }
        }
        return Some(result);
    }
    let mut vars = vec![module as u32, script.hwnd, V_OPS as u32, 0, script.result];
    vars.extend(encode(&script.ops));
    task::start(cpu, task::COMCTL32, vars, script.scratch.max(4))
}

/// The steps of a script.
pub fn step(cpu: &mut Cpu, t: &mut Task, value: u32) -> Next {
    if t.step == STEP_RETURNED {
        t.set(V_LAST, value);
    }
    loop {
        let pc = t.var(V_PC) as usize;
        if pc >= t.vars.len() {
            return Next::Done(t.var(V_RESULT));
        }
        let resolve = |t: &Task, kind: u32, v: u32| match kind {
            1 => t.scratch.wrapping_add(v),
            2 => t.var(V_LAST),
            _ => v,
        };
        match t.var(pc) {
            T_CALL => {
                let (target, n, kinds) = (t.var(pc + 1), t.var(pc + 2) as usize, t.var(pc + 3));
                let args: Vec<u32> = (0..n)
                    .map(|i| resolve(t, kinds >> (2 * i) & 3, t.var(pc + 4 + i)))
                    .collect();
                t.set(V_PC, (pc + 4 + n) as u32);
                return Next::call(target, &args, STEP_RETURNED);
            }
            T_THEN => {
                let (id, n) = (t.var(pc + 1) as u16, t.var(pc + 2) as usize);
                let data = t.vars[pc + 3..(pc + 3 + n).min(t.vars.len())].to_vec();
                let next = pc + 3 + n;
                t.set(V_PC, next as u32);
                let mut ctx = Ctx {
                    hwnd: t.var(V_HWND),
                    last: t.var(V_LAST),
                    scratch: t.scratch,
                    data,
                    result: t.var(V_RESULT),
                };
                let module = t.var(V_MODULE);
                let ops = dispatch_then(cpu, module, id, &mut ctx);
                t.set(V_RESULT, ctx.result);
                let words = encode(&ops);
                let at = next.min(t.vars.len());
                t.vars.splice(at..at, words);
            }
            T_WRITE => {
                let (at, n) = (t.var(pc + 1), t.var(pc + 2) as usize);
                for i in 0..n {
                    let w = t.var(pc + 3 + i);
                    cpu.bus
                        .guest_write_32(t.scratch.wrapping_add(at + 4 * i as u32), w);
                }
                t.set(V_PC, (pc + 3 + n) as u32);
            }
            T_RETURN => {
                let v = resolve(t, t.var(pc + 1), t.var(pc + 2));
                return Next::Done(v);
            }
            _ => return Next::Done(t.var(V_RESULT)),
        }
    }
}
