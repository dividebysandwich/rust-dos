//! The x86-64 code generator.
//!
//! Registers while translated code runs: RBX the CPU, R12 the context
//! (`JitCtx`), R13 RAM, R14 RAM's code generations; RAX, RCX, RDX, RSI,
//! RDI and R8-R11 are scratch. The guest's registers stay in the CPU.
//! Translated code calls Rust with the System V convention, which Rust
//! offers on every x86-64 host.

use dynasmrt::x64::X64Relocation;
use dynasmrt::{DynasmApi, DynasmLabelApi, VecAssembler, dynasm};

use super::block::BlockData;
use super::helpers::*;
use crate::cpu::layout;

type Asm = VecAssembler<X64Relocation>;

const ICOUNT: i32 = layout::ICOUNT as i32;
const DEADLINE: i32 = layout::DEADLINE as i32;
const EXECUTED: i32 = layout::EXECUTED as i32;

/// The way in from Rust: `enter(cpu, ctx, code)` saves the registers Rust
/// expects kept, sets up the fixed ones and jumps to `code`; translated
/// code leaves through `exit` with the exit code in EAX and its block in
/// RDX, which it stores in the context before returning the code.
pub struct Trampoline {
    pub bytes: Vec<u8>,
    pub enter: usize,
    pub exit: usize,
}

pub type Enter = unsafe extern "sysv64" fn(*mut crate::cpu::Cpu, *mut JitCtx, *const u8) -> u64;

pub fn trampoline() -> Trampoline {
    let mut ops = Asm::new(0);
    let enter = ops.offset().0;
    dynasm!(ops
        ; .arch x64
        ; push rbx
        ; push rbp
        ; push r12
        ; push r13
        ; push r14
        ; push r15
        // Calls from translated code need RSP 16-byte aligned.
        ; sub rsp, 8
        ; mov rbx, rdi
        ; mov r12, rsi
        ; mov r13, QWORD [r12 + CTX_RAM]
        ; mov r14, QWORD [r12 + CTX_PAGE_GEN]
        ; jmp rdx
    );
    let exit = ops.offset().0;
    dynasm!(ops
        ; .arch x64
        ; mov QWORD [r12 + CTX_EXIT_DATA], rdx
        ; add rsp, 8
        ; pop r15
        ; pop r14
        ; pop r13
        ; pop r12
        ; pop rbp
        ; pop rbx
        ; ret
    );
    Trampoline { bytes: ops.finalize().expect("trampoline"), enter, exit }
}

/// Translate a block whose instructions all run through their interpreter
/// handlers.
///
/// The prologue checks that the block fits before the timer deadline and
/// that its bytes' chunks haven't been written since it was translated
/// (or if they have, that its bytes are still the same). Before each
/// instruction the instruction count is brought up to date, as devices
/// read the time from it; the count of executed instructions is only
/// brought up to date where the code returns.
pub fn block(data: &BlockData) -> Vec<u8> {
    let mut ops = Asm::new(0);
    let data_ptr = data as *const BlockData as i64;
    let n = data.count() as i32;
    let deadline = ops.new_dynamic_label();
    let revalidate = ops.new_dynamic_label();
    let body = ops.new_dynamic_label();
    let fallback_exits: Vec<_> = (0..n).map(|_| ops.new_dynamic_label()).collect();

    // Prologue.
    dynasm!(ops
        ; .arch x64
        ; mov rax, QWORD [rbx + ICOUNT]
        ; add rax, n
        ; cmp rax, QWORD [rbx + DEADLINE]
        ; ja =>deadline
        ; mov rdx, QWORD data_ptr
        ; mov eax, DWORD [r14 + (data.chunk_first * 4) as i32]
    );
    for chunk in data.chunk_first + 1..=data.chunk_last {
        dynasm!(ops
            ; .arch x64
            ; add eax, DWORD [r14 + (chunk * 4) as i32]
        );
    }
    dynasm!(ops
        ; .arch x64
        ; cmp eax, DWORD [rdx + DATA_GEN_SUM]
        ; jne =>revalidate
        ; =>body
    );

    // The instructions.
    let mut icount_synced = 0;
    for ix in 0..n {
        if ix > icount_synced {
            dynasm!(ops
                ; .arch x64
                ; add QWORD [rbx + ICOUNT], ix - icount_synced
            );
            icount_synced = ix;
        }
        dynasm!(ops
            ; .arch x64
            ; mov rdi, rbx
            ; mov rsi, r12
            ; mov rdx, QWORD data_ptr
            ; mov ecx, ix
            ; call QWORD [r12 + CTX_FALLBACK]
            ; test eax, eax
            ; jnz =>fallback_exits[ix as usize]
        );
    }

    // The end: the counts, and back to the execution loop.
    dynasm!(ops
        ; .arch x64
        ; add QWORD [rbx + ICOUNT], n - icount_synced
        ; add QWORD [rbx + EXECUTED], n
        ; mov eax, EXIT_NEXT as i32
        ; mov rdx, QWORD data_ptr
        ; jmp QWORD [r12 + CTX_EXIT]
    );

    // A fallback stopped the block at instruction ix, having counted it
    // as executed but not in the instruction count, as the interpreter
    // does until the instruction is done.
    for ix in 0..n {
        dynasm!(ops
            ; .arch x64
            ; =>fallback_exits[ix as usize]
            ; add QWORD [rbx + EXECUTED], ix + 1
            ; or eax, ix << 8
            ; mov rdx, QWORD data_ptr
            ; jmp QWORD [r12 + CTX_EXIT]
        );
    }

    dynasm!(ops
        ; .arch x64
        ; =>deadline
        ; mov eax, EXIT_DEADLINE as i32
        ; mov rdx, QWORD data_ptr
        ; jmp QWORD [r12 + CTX_EXIT]
        ; =>revalidate
        ; mov rdi, rbx
        ; mov rsi, r12
        ; call QWORD [r12 + CTX_REVALIDATE]
        ; test eax, eax
        ; jz =>body
        ; mov eax, EXIT_STALE as i32
        ; mov rdx, QWORD data_ptr
        ; jmp QWORD [r12 + CTX_EXIT]
    );
    ops.finalize().expect("block")
}
