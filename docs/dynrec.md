# The dynamic recompiler

`src/dynrec` translates blocks of the program's code into host machine code
and runs them in place of the interpreter, as DOSBox's dynamic core does.
It is **exact**: every instruction sees the same instruction count (the
emulated clock), interrupts arrive between the same instructions, and
faults leave the same state as with the interpreter. The two cores can
therefore be run side by side and compared after every batch, which is how
the recompiler is tested (see [Testing](#testing)).

It generates code for x86-64 hosts (`x64.rs`) and ARM64 hosts (`a64.rs`).
On other hosts and in the browser,
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
  (`uop.rs`), which the code generator turns into host code:
  - MOV, the ALU operations, INC, DEC, NEG and NOT;
  - shifts and rotates but RCL and RCR, by a constant or CL, of registers
    and memory;
  - SHLD and SHRD by a constant or CL;
  - MUL and IMUL in all their forms, DIV and IDIV;
  - LEA, MOVZX, MOVSX, XCHG of registers, CBW, CWD, CWDE and CDQ;
  - the flag instructions, and SETcc;
  - PUSH and POP of registers and constants;
  - near JMP, CALL, RET, Jcc, LOOPcc and JCXZ.

  Each does what the instruction's interpreter handler does, in the same
  order, flags included. Where a form has rare cases the operations don't
  cover (a byte or word shifted by CL past its width), a `Bail` operation
  checks for them first and runs the instruction through its handler
  instead.
- **A call of its interpreter handler.** Everything else runs through
  `jit_fallback`, which does what `exec::execute_at` does. Every
  instruction works in a block from the start; translating more forms only
  makes them faster.

Both code generators keep the guest's registers in the `Cpu`, and its
arithmetic flags (CF, PF, AF, ZF, SF, OF) in a host register from the
first instruction in a block that changes them:

- **Where they go back.** The flags go back into the `Cpu` wherever
  anything else may read them: where the block leaves (to another block
  too) and before a handler runs. An instruction that stops the block
  (a fault, a store into its later bytes) sets `EXIT_FLAGS` in its exit
  code instead: the trampoline leaves the register in the `JitCtx`, and
  the execution loop puts it back.
- **Only the live ones.** `flags.rs` finds, for each operation, which of
  the flags it sets are read before another operation sets them again:
  by a condition, a carry in, or anything outside the block, which sees
  all of them wherever the block may leave, a fault included. The
  others aren't computed.
- **On x86-64** they come from the host's own flags (PUSHF), from host
  instructions of the operand's size, where they match the interpreter's
  (`cpu::alu`). They are fixed up where the interpreter defines what the
  host leaves undefined: AF of the logic operations, OF of shifts by more
  than 1, and the 386's AF of SHL and SHR. ADD, SUB, CMP, ADC, SBB and
  NEG, whose flags are the host's, are two instructions (`pushfq; pop
  rbp`); INC and DEC get the guest's CF into the host's first.
- **On ARM64**, which has neither a parity nor an auxiliary carry flag,
  they are computed as `cpu::alu` defines them, one by one: the carry
  from a 64-bit sum or difference of the operands, PF from a table in the
  `JitCtx`.
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

| Holds | x86-64 | ARM64 |
|---|---|---|
| the `Cpu` | RBX | X19, and X27 for its fields past X19's offsets' reach |
| the `JitCtx` | R12 | X20 |
| RAM | R13 | X21 |
| the code generations | R14 | X22 |
| set when a store hit the block's later bytes | R15 | W23 |
| the guest's arithmetic flags | EBP | W28 |
| the operations' temporaries | R8–R11 (saved around calls) | W24–W26 (kept by calls) |

On x86-64, calls into Rust use the System V convention, which Rust offers
on every x86-64 host, Windows included; on ARM64 the platform's own. ARM64
code never touches X18, which macOS reserves.

### Linking

A block leaves to another block without the execution loop through its
links (`BlockData::links`): two for exits to a known EIP (a jump's target,
a conditional jump's next instruction), and one for a return. Links are
data, not code:

- A link starts at a stub that returns to the execution loop.
- The execution loop then finds or translates the block at the target and
  sets the link.
- When a block is thrown away, the links to it are set back to their stubs
  (`Block::backlinks`).

Nothing a chain of blocks can do changes what the execution loop checks
before a block (see [What a block holds](#what-a-block-holds)), so a
linked block only needs its own prologue. Where the next block's code is
takes more care:

- **Within the block's page.** The link is taken as it is. The page's
  translation, CS, CPL and A20 only change through instructions that end
  a block without a link, so the next block is where it was.
- **To another page, and returns.** The link keeps a `Guard`: the CS base,
  the A20 gate and paging when it was made, with paging the target page's
  physical page in the TLB, and for a return its target EIP. The code
  takes the link only while all of these are the same. Fetching the target
  then goes as the interpreter's fetch would: to the same physical page,
  without walking the page tables. Otherwise it goes back to the execution
  loop through the stub.
  - The execution loop makes these links when the block it runs next is
    at the stub's target (`Pending`): its own fetch just found the target,
    under the translation the guard records.
  - After a chain that crossed pages, the execution loop's code window is
    moved to the page the last instruction was in, as the interpreter's
    would have been (`CodeWindow::moved`).

Loops, and calls and returns between pages, run in translated code until
the next timer event.

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
itself, which shows the comparison is deterministic. `DYNDIFF_KEYS`
presses keys before given batches, as `BATCH:KEY` pairs with the debug
server's key names, to get a program past its menus: Descent's first
demo, from the registered version's directory, is

```sh
DYNDIFF_PROGRAMS=Descent:DESCENTR DYNDIFF_BATCHES=9000 \
  DYNDIFF_KEYS="3600:enter,3800:down,3820:down,3840:down,3860:down,3880:down,3950:enter,4150:enter"
```

which plays from about batch 4500.

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

**ARM64 on an x86-64 machine.** The tests run under qemu's user-mode
emulation, with an aarch64 cross linker (Arch: `qemu-user` and
`aarch64-linux-gnu-gcc`) and `rustup target add aarch64-unknown-linux-gnu`.
The test binaries don't use SDL, but the emulator binary built with them
links it, so a stub library stands in:

```sh
mkdir -p /tmp/a64stub && aarch64-linux-gnu-gcc -shared -o /tmp/a64stub/libSDL2.so -x c /dev/null
export CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER=aarch64-linux-gnu-gcc
export CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_RUNNER="qemu-aarch64 -L /usr/aarch64-linux-gnu"
export CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_RUSTFLAGS="-L /tmp/a64stub -C link-arg=-Wl,--unresolved-symbols=ignore-all"
RUST_DOS_CORE=dynamic cargo test --release --target aarch64-unknown-linux-gnu
```

The lockstep and conformance commands above take the same `--target`. CI
runs every test on both cores on Linux x86-64 and ARM64 runners, and the
recompiler's own tests on macOS and Windows.

**Speed.** `compute_speed` runs CPU-bound protected-mode programs (a
CRC-32, a bubble sort, shifts and rotates) on both cores in lockstep and
prints their speeds. `local_program_alone` runs one program on one core,
for profiling, with the keys of `DYNDIFF_KEYS`:

```sh
cargo test --release --test dyndiff_tests compute_speed -- --ignored --nocapture
DYNDIFF_PROGRAMS=TD3:TD3 cargo test --release --test dyndiff_tests local_program_alone -- --ignored --nocapture
```

It times only the batches from `DYNDIFF_TIME_FROM` on (`4500` for the
Descent demo above), and `DYNDIFF_SHOTS=N` saves the screen every N
batches into `target/dyndiff/<dir>/`, to find where a program gets to.

## Translating another instruction form

1. Add the form to `translate::translate`. Its operations must do what
   the instruction's handler does, in the same order: everything that can
   fault (`MemRef`, `CheckLimit`) before anything that changes state
   (`Set`, `Store`, flags). The exception is where the handler itself
   writes memory before it faults, as CALL does.
2. If it needs an operation the code generators don't have, add one to
   `uop.rs` and its code to `x64.rs` and `a64.rs`, and say in
   `Uop::flags_set` and `Uop::flags_used` (`flags.rs`) which flags it sets
   and which must be right before it: those it reads, and all of them if
   it may leave the block. Take flags from the host only where they match
   `cpu::alu`'s; compute the rest, and only the live ones.
3. Run `dynrec_tests`, the lockstep tests, and SingleStepTests with
   `RUST_DOS_CORE=dynamic`, whose report must be the interpreter's, on
   both hosts (ARM64 under qemu, see [Testing](#testing)).
