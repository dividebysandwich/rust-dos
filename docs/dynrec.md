# The dynamic recompiler

`src/dynrec` translates blocks of the program's code into host machine code
and runs them in place of the interpreter, as DOSBox's dynamic core does.
It is **exact**: every instruction sees the same instruction count (the
emulated clock), interrupts arrive between the same instructions, and
faults leave the same state as with the interpreter. The two cores can
therefore be run side by side and compared after every batch, which is how
the recompiler is tested (see [Testing](#testing)).

It generates code for x86-64 hosts. On other hosts and in the browser,
`dynrec::AVAILABLE` is false and the interpreter runs everything.

## Choosing the core

The `core` setting (`[emulator]` in the configuration file, `--core`, the
settings window's Emulator page) picks what runs the instructions:

| `core` | Runs |
|---|---|
| `auto` (default) | The interpreter, and the recompiler for a program from its first switch to protected mode (CR0.PE 0 to 1) until it ends, as DOSBox's `core=auto` does. The latch is `Cpu::dyn_latched`: `note_mode_switch` sets it, `restore_process_context` and `load_shell` clear it. |
| `dynamic` | The recompiler throughout. |
| `normal` | The interpreter. |

`Cpu::dynamic_active()` says which applies now. `run_batch` picks its loop
by it at the start of a batch, so a program that switches to protected
mode goes on the recompiler from the next batch. The debugger's
per-instruction hooks (breakpoints, stepping, tracing) run on the
interpreter; a hook whose `ExecHook::per_instruction` is false (the tests'
`StopAtHlt`) sees the instructions the execution loop dispatches, which
include every HLT.

`RUST_DOS_CORE=dynamic|normal|auto` in the environment sets the core
`Cpu::new` starts with. That is how the whole test suite runs on the
recompiler; the front ends set the configured core.

## How it runs

`exec::run` checks the timer deadline, delivers interrupts and serves the
shell before each dispatch, as the interpreter's loop does. Then
`exec::dynamic` finds the instruction at CS:EIP (`fetch_location`, the
interpreter's own code) and hands it to `DynState::run`. That looks up or
translates the block starting there and enters it. The interpreter runs
the instruction instead when no block can start there, or when the block
doesn't fit before the next timer event.

### What a block holds

A block (`block.rs`) is a run of instructions in one page, at most 64 of
them. It holds exactly the instructions the interpreter would fetch
through its code window (`exec::CodeWindow`):

- none starting in the last 15 bytes of the page;
- none less than 15 bytes before the CS limit;
- never linear page 0, where the interpreter watches for jumps into the
  interrupt table.

Everything else goes through `locate`, whose page walks, Accessed bits,
CR2 and tripwire the recompiler never needs to reproduce. Blocks aren't
made in the shell's code, or in the page of the mouse driver's stub, which
clears a byte of RAM that decides whether the next mouse event can be
delivered.

A block stops **before** an instruction the interpreter must run itself:
an invalid encoding (the emulator's service traps, FE 38/39, are ones) and
HLT.

It stops **after** one that may change anything the execution loop checks
between instructions:

| Instructions | What they can change |
|---|---|
| Control transfers: jumps, calls, returns, INT, IRET, LOOP, JCXZ | Where execution goes |
| Port I/O (IN, OUT, INS, OUTS) | Devices, their interrupts, the timer deadline, the A20 gate, the reset line |
| STI, POPF | IF and the interrupt shadow |
| MOV SS, POP SS, LSS | The interrupt shadow, the stack's width |
| Writes to CR0, CR3, DRn, TRn, LMSW, CLTS, INVLPG, LGDT, LIDT, LLDT, LTR | The mode, paging, the TLB, the descriptor tables |

Because of this, whether an interrupt can be delivered never changes
inside a block, or in a chain of linked blocks: the execution loop has
just found none deliverable, and only these instructions could change
that. So blocks need no interrupt checks.

### Exactness at run time

Each block's prologue checks three things:

1. **The deadline:** it fits before the next timer event,
   `icount + n <= deadline`.
2. **The CS limit:** it is at least what the block needs to be fetched
   through the code window.
3. **The code:** its bytes are unchanged. The sum of the code generations
   (`Bus::page_gen`, one per 64-byte chunk, bumped by every RAM write) of
   its chunks is the one it was translated with. If the sum differs,
   `jit_revalidate` compares the bytes, and the block goes on if only
   something else in its chunks was written.

If any check fails, the block stops before its first instruction.

Other exactness rules:

- **Counters.** The instruction count is brought up to date before every
  call into Rust that could read it (devices read the time from it at port
  access), and at every exit. The count of executed instructions is only
  brought up to date at exits.
- **Faults.** An instruction that faults is undone as the interpreter
  undoes it: every instruction checks what can fault before it changes
  anything, and ESP is restored. It leaves the block with its index. The
  execution loop sets EIP, delivers the fault through the interpreter's
  own tail (`exec::after_fault`), and counts it.
- **Self-modifying code.** A store that hits the block's later bytes stops
  the block after the storing instruction. Native stores compare their
  physical address with those bytes; instructions run through their
  handlers compare the code generations and then the bytes. The block is
  translated again next time.

### Translation

Each instruction becomes one of two things:

- **Native code.** `translate.rs` turns the common forms into operations
  (`uop.rs`), which `x64.rs` turns into host code:
  - MOV, the ALU operations, INC, DEC, NEG and NOT;
  - shifts and rotates by a constant;
  - LEA, MOVZX, MOVSX, XCHG of registers, CBW, CWD, CWDE and CDQ;
  - the flag instructions;
  - PUSH and POP of registers and constants;
  - near JMP, CALL, RET, Jcc, LOOPcc and JCXZ.

  Each does what the instruction's interpreter handler does, in the same
  order, flags included.
- **A call of its interpreter handler.** Everything else runs through
  `jit_fallback`, which does what `exec::execute_at` does. Every
  instruction works in a block from the start; translating more forms only
  makes them faster.

The x86-64 code keeps the guest's registers and flags in the `Cpu` and
works on them with host instructions of the same size:

- Flags come from the host's own flags (PUSHF) where they match the
  interpreter's (`cpu::alu`). They are fixed up where the interpreter
  defines what the host leaves undefined: AF of the logic operations, OF of
  shifts by more than 1, and the 386's AF of SHL and SHR.
- Memory operands are checked inline against the segment's precomputed
  limits and rights. With paging on, the code looks the page up in the
  TLB as `Cpu::lin_to_phys` does. Plain RAM within a page is then read and
  written directly, with the code generations bumped as the bus bumps
  them.
- Anything else (a TLB miss, video memory, page-crossing operands, a
  fault) goes through `jit_memref`, which runs the real `Cpu::mem_ref`. It
  returns the physical address when the operand turns out to be plain
  RAM, so the loads and stores after it are direct as well.

Registers while translated code runs:

| Register | Holds |
|---|---|
| RBX | the `Cpu` |
| R12 | the `JitCtx` |
| R13 | RAM |
| R14 | the code generations |
| R15 | set when a store hit the block's later bytes |
| R8–R11 | the operations' temporaries |

Calls into Rust use the System V convention, which Rust offers on every
x86-64 host, Windows included.

### Linking

An exit to a known EIP in the block's own page jumps through one of the
block's two links (`BlockData::links`), which are data, not code:

- A link starts at a stub that returns to the execution loop, which
  translates the block at the target, sets the link and goes on in it.
- When a block is thrown away, the links to it are set back to their stubs
  (`Block::backlinks`).
- Linking only within a page means nothing a chain of blocks can do
  changes where the next block's code is: the page's translation, CS, CPL
  and A20 only change through instructions that end a block without a
  link.

Loops run in translated code until the next timer event.

### Code memory

`codemem.rs` reserves 32 MB:

- read, write and execute where the host allows it;
- otherwise executable, with the pages made writable for each copy;
- MAP_JIT and `pthread_jit_write_protect_np` on Apple Silicon.

Blocks are assembled with dynasm-rs into a buffer and copied in. When the
memory is full, everything is thrown away and translation starts over. So
it is at the shell prompt (`load_shell`), and when the CPU model changes.

## Statistics

`/api/stats` on the debug server has `core`, `recompiler` (whether it runs
now) and `dynrec`:

| Field | Counts |
|---|---|
| `blocks`, `instructions` | Blocks translated and their instructions |
| `native` | Instructions translated into host code (the rest call handlers) |
| `live_blocks`, `code_bytes` | Blocks translated now, and their host code |
| `flushes` | Times all translated code was thrown away |
| `runs` | Blocks entered from the execution loop |
| `deadline` | Blocks that stopped at once for the timer |
| `stale` | Blocks that stopped at once because their bytes changed |
| `smc` | Blocks whose later bytes an instruction changed |

The settings window's Stats page says whether the instructions are
recompiled or interpreted.

## Testing

**Lockstep.** `tests/dyndiff` runs two machines side by side in batches
and compares them after each:

- the registers, CR0, CR2, CR3 and CPL;
- the instruction counts and exceptions;
- RAM, the code generations, video memory, the palette and the debug
  console.

The host's time is fixed for both (`hosttime::fix`).

| Test | Checks |
|---|---|
| `tests/dyndiff_tests.rs` | A protected-mode program with a fast timer interrupt |
| `tests/dynrec_tests.rs` | Stores into the rest of a block, faults and page faults in the middle of one, interrupt shadows, timer reads, a full code memory, the auto latch, rewriting a linked block, and a smaller CS limit under a link |

Local DOS programs run in lockstep opt-in, from the git-ignored
`programs/` directory:

```sh
DYNDIFF_PROGRAMS="D1SW:DCNTSHR,STUNTS:STUNTS" DYNDIFF_BATCHES=3000 \
  cargo test --release --test dyndiff_tests local_programs_in_lockstep -- --ignored --nocapture
```

Each entry is a directory under `programs/` and the command that starts
the program there. `DYNDIFF_CORE=normal` compares the interpreter with
itself, which shows the comparison is deterministic.

**The whole suite on the recompiler.** `RUST_DOS_CORE=dynamic cargo test`
runs every test on the recompiler. `Cpu::step` translates one-instruction
blocks, so the instruction tests go through the code generator too.

**Conformance suites.** They run on the recompiler the same way (see
[cpu-tests.md](cpu-tests.md)):

```sh
RUST_DOS_CORE=dynamic SST386_DIR=target/sst386/v1_ex_real_mode \
  cargo test --release --test sst386 -- --ignored --nocapture
RUST_DOS_CORE=dynamic TEST386_DIR=target/test386 \
  cargo test --release --test test386 -- --ignored --nocapture
```

**Speed.** `compute_speed` runs CPU-bound protected-mode programs (a
CRC-32, a bubble sort, shifts and rotates) on both cores in lockstep and
prints their speeds. `local_program_alone` runs one program on one core,
for profiling:

```sh
cargo test --release --test dyndiff_tests compute_speed -- --ignored --nocapture
DYNDIFF_PROGRAMS=TD3:TD3 cargo test --release --test dyndiff_tests local_program_alone -- --ignored --nocapture
```

## Translating another instruction form

1. Add the form to `translate::translate`. Its operations must do what
   the instruction's handler does, in the same order: everything that can
   fault (`MemRef`, `CheckLimit`) before anything that changes state
   (`Set`, `Store`, flags). The exception is where the handler itself
   writes memory before it faults, as CALL does.
2. If it needs an operation the code generator doesn't have, add one to
   `uop.rs` and its code to `x64.rs`. Take flags from the host only where
   they match `cpu::alu`'s; compute the rest.
3. Run `dynrec_tests`, the lockstep tests, and SingleStepTests with
   `RUST_DOS_CORE=dynamic`, whose report must be the interpreter's.
