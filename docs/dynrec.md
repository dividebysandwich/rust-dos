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

- none starting in the last 15 bytes of the page, but on x86-64 hosts
  (see below);
- none less than 15 bytes before the CS limit;
- never linear page 0, where the interpreter watches for jumps into the
  interrupt table.

Everything else goes through `locate`, whose page walks, Accessed bits,
CR2 and tripwire the recompiler never needs to reproduce.

On x86-64 hosts a block goes on into the page's last 15 bytes, with the
instructions that end in the page (`BlockData::in_tail`). There
`locate` looks the next page up for an instruction, in case it runs on
into it, which with paging may walk the page tables (setting their
Accessed bits and filling the TLB). With paging on, the code leaves the
block before such an instruction (`EXIT_NEXT_PAGE`) unless the TLB holds
the next page, where the lookup changes nothing, and the interpreter runs
it. Code at the end of a page then runs in blocks and their links rather
than in the interpreter, where DOS extenders map a program's pages apart
from each other in physical memory, as through VCPI. Blocks aren't
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
| String port I/O (INS, OUTS) | Devices, their interrupts, the timer deadline, the A20 gate, the reset line |
| IRET | IF and the interrupt shadow |
| Writes to CR0, CR3, DRn, TRn, LMSW, CLTS, INVLPG, LGDT, LIDT, LLDT, LTR | The mode, paging, the TLB, the descriptor tables |

IN, OUT, STI, POPF, MOV SS, POP SS and LSS can change the same, but
programs run them so often (a timer read, a sound driver's status, a CLI
and STI or a PUSHF and POPF around each, a switch to a stack of its own)
that the block goes on after them where they changed none of it. They run
in the block (IN, OUT and STI translated on x86-64 hosts, with IN and OUT
through `jit_port`; the others, and all of them elsewhere, through their
handlers and `jit_fallback`), which stops after the instruction
(`EXIT_AFTER`) only if:

- after IN or OUT, an interrupt can be delivered (a device raised one, or
  the PIC let one through, with IF set), the timer deadline or the A20
  gate changed, the rest of the block no longer fits before the deadline
  (a port access takes emulated time), a reset was asked for, or the CPU
  no longer runs;
- after STI, an interrupt waits: the execution loop runs the next
  instruction, in the interrupt shadow, and delivers it. Where none waits
  the shadow ends in the block, at the next instruction;
- after POPF (through its handler), TF is set, or IF is set with an
  interrupt waiting;
- after a load of SS (through its handler), the stack's width changed,
  which the block was translated for (its key's stack bit). Otherwise the
  interrupt shadow ends in the block, at the next instruction.

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
   (`Bus::page_gen`, one per 64-byte chunk, see [Code chunks](#code-chunks))
   of its chunks is the one it was translated with. If the sum differs,
   `jit_revalidate` compares the bytes, and the block goes on if only
   something else in its chunks was written, or only its watched bytes
   (see [Poked code](#poked-code)).

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
  the block after the storing instruction. Native stores into a chunk with
  code compare their physical address with those bytes; instructions run
  through their handlers compare the code generations and then the bytes.
  The block is translated again next time.

### Code chunks

The code generations only matter for RAM that code was decoded from:
the interpreter's decoded-instruction cache and the blocks check the
generations of the chunks their bytes are in. So the bus marks those
chunks (`Bus::code_blocks`) where the interpreter decodes an instruction
and where the recompiler translates a block or finds none can start, and
a chunk stays marked. The chunk before one with code is marked as well:
a write of up to 4 bytes that reaches into a chunk with code starts in it
or in the chunk before.

The bus bumps the generations of every chunk it writes. The x86-64 code
generator's stores check the mark of the chunk their first byte is in,
and write a chunk without code directly, without bumping its generation:
the stores of programs that keep their data apart from their code (the
pixels a texture mapper draws, the stack) cost one byte's test. A store
into a chunk with code goes out of line, bumping the generations and
checking for a store into the block's later bytes. The code generations
of the two cores therefore differ, and the lockstep test doesn't compare
them.

### Poked code

Some programs change a byte of their code over and over and put it back:
Doom-engine games poke a RET into an unrolled loop at the end of every
span they draw. Translating every block over that byte again each time
would take more time than the drawing. So where a block goes stale or
writes over itself with at most four of its bytes changed (a poke, not
new code), the engine counts those bytes, per physical page
(`Engine::pokes`). Once a byte has been counted twice
(`block::WATCH_AFTER`), blocks translated over it watch it:

- The instruction the byte is in compares it with the byte it was
  translated from before it runs. If they differ, the block leaves before
  the instruction (`EXIT_WATCHED`), with the instructions before it done,
  and the interpreter runs whatever is there now.
- The block's own checks leave the watched bytes out, so it stays valid
  while only they change.
- A watched instruction that ends the block where it is now (the poked
  RET itself) isn't put in a block: the block stops before it and goes
  on through a link, and the interpreter runs it.

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
  - PUSH and POP of registers and constants, PUSHA and POPA;
  - MOV and PUSH of segment registers (reading their selectors), and CLI;
  - on x86-64 hosts, MOV and POP into segment registers but CS and SS
    (through `jit_load_seg`, which runs `Cpu::load_segment`), IN and OUT
    (through `jit_port`, see [What a block holds](#what-a-block-holds)),
    and STI;
  - near JMP, CALL (of a register or memory too), RET, Jcc, LOOPcc and
    JCXZ.

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
  them where the chunk holds code (see [Code chunks](#code-chunks)). The
  x86-64 code is translated for what the block runs under (see
  [Environments](#environments)): through a flat segment, with paging
  off and the A20 gate open, an operand is plain RAM if none of its bytes
  is in the video memory or ROMs and it ends before the end of RAM, two
  compares with constants.
- Anything else (a TLB miss, video memory, page-crossing operands, a
  fault) goes through `jit_memref`, which runs the real `Cpu::mem_ref`. It
  returns the physical address when the operand turns out to be plain
  RAM, so the loads and stores after it are direct as well.

On x86-64, the four guest registers a block's operations use most (twice
or more) stay in host registers within it (`x64::Cache`). An instruction
loads those it uses from the `Cpu` as it starts, if they aren't there
yet, so where they are doesn't change within it; the changed ones go back
into the `Cpu` where the block leaves and before a handler's call, after
which they are loaded again, and those loaded where an instruction stops
the block (it changes none before it may fault). The slow paths keep them
around their calls into Rust.

Registers while translated code runs:

| Holds | x86-64 | ARM64 |
|---|---|---|
| the `Cpu` | RBX | X19, and X27 for its fields past X19's offsets' reach |
| the `JitCtx` | R12 | X20 |
| RAM | R13 | X21 |
| the code generations | | X22 |
| which chunks hold code (`Bus::code_blocks`) | R14 | |
| set when a store hit the block's later bytes | `JitCtx::smc` | W23 |
| the TLB's entries | in the `Cpu`, from RBX | |
| the guest's arithmetic flags | EBP | W28 |
| the operations' temporaries | R8–R10 (saved around calls) | W24–W26 (kept by calls) |
| the guest registers the block uses most | R11, RSI, RDI (saved around calls), R15 (kept by calls) | |

On x86-64, calls into Rust use the System V convention, which Rust offers
on every x86-64 host, Windows included; on ARM64 the platform's own. ARM64
code never touches X18, which macOS reserves.

### Environments

Blocks are translated for what their memory operands go through, which
is part of their key (`Key::mode`, the `ENV_*` bits of `dynrec::Env`)
along with the code and stack sizes:

- paging, and with it CPL 3 (the TLB's user entries);
- the A20 gate;
- which segment registers are flat: base 0, every offset within their
  limits, readable and writable, as DOS extenders' data and stack
  segments are. Such a segment's offset is the linear address, and it
  needs no limit check but for a wraparound past 4 GB, which the check
  for the end of RAM catches;
- which are plain: expand-up, readable and writable, with any base and
  limit, as a texture's segment is. An access through one checks only
  the end of the limit (and a wraparound), and adds the base.

The execution loop finds the block for the environment it runs in, and
none of it changes within a block or a chain of linked blocks: paging
and CPL change only in instructions that end the block without a link,
and the A20 gate stops the block after the port access that changed it
(`EXIT_AFTER`). A segment load (MOV, POP, LDS, LES, LFS or LGS) doesn't
stop the block: `jit_fallback` notes which segments are flat after it
(`JitCtx::flat`), the rest of the block checks the segment's accesses as
it does a segment's that isn't flat, and its links are taken only where
the segments are flat as the block's environment has them; elsewhere it
returns to the execution loop. Code that loads a data segment and puts
it back within a block or chain (as 16-bit drivers do) goes on in
translated code.
The x86-64 code generator leaves out what the environment makes
unnecessary: the limit checks and base of flat segments, the TLB lookup
with paging off, the A20 mask with the gate open, and then the check
for an operand in two pages, whose RAM is contiguous. With paging on, it
looks the page up in the TLB's set for the CPL, whose 32-byte entries (in
the `Cpu`) it indexes with the linear address shifted and masked. It
compares the entry's tag for its code with the page of the operand's last
byte, which an entry of the first byte's page never holds for an operand
in two pages, nor one of a page that isn't plain RAM (`TlbEntry::jit_read`
and `jit_write`), and adds the entry's difference between the physical
and linear page to the address. A link to another page checks the TLB's
translation of its target, but not the A20 gate and paging, which are as
when the link was made. The ARM64 one checks everything at run time.

### Linking

A block leaves to another block without the execution loop through its
links (`BlockData::links`): two for exits to a known EIP (a jump's target,
a conditional jump's next instruction), and four for a return or a call
through a register or memory, to the last places it went to: a function
called from two places in turn returns to each through its own link.
Links are data, not code:

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
  - A return or indirect call to none of the EIPs its links were made to
    leaves through a stub of its own (`RETURN_MISS`), and the execution
    loop links one it hasn't made yet, or else the one after the one it
    made last.
  - After a chain that crossed pages, the execution loop's code window is
    moved to the page the last instruction was in, as the interpreter's
    would have been (`CodeWindow::moved`).

Loops, and calls and returns between pages, run in translated code until
the next timer event.

### Code memory

`codemem.rs` reserves 128 MB:

- read, write and execute where the host allows it;
- otherwise executable, with the pages made writable for each copy;
- MAP_JIT and `pthread_jit_write_protect_np` on Apple Silicon.

Blocks are assembled with dynasm-rs into a buffer and copied in, into
the smallest space a thrown-away block left that they fit, else after
everything. A block whose bytes changed gives its space back: code that
is rewritten with new code over and over would otherwise fill the memory.
When nothing fits, everything is thrown away (with the counts of poked
bytes) and translation starts over. So it is at the shell prompt
(`load_shell`), and when the CPU model changes.

## Statistics

`/api/stats` on the debug server has `core`, `recompiler` (whether it runs
now) and `dynrec`:

| Field | Counts |
|---|---|
| `blocks`, `instructions` | Blocks translated and their instructions |
| `native` | Instructions translated into host code (the rest call handlers) |
| `live_blocks`, `code_bytes`, `links` | Blocks translated now, their host code, and the links between them |
| `flushes` | Times all translated code was thrown away |
| `runs` | Blocks entered from the execution loop |
| `deadline` | Blocks that stopped at once for the timer |
| `stale` | Blocks that stopped at once because their bytes changed |
| `smc` | Blocks whose later bytes an instruction changed |
| `watched` | Blocks that stopped at an instruction whose watched bytes had changed |

The settings window's Stats page says whether the instructions are
recompiled or interpreted.

## Testing

**Lockstep.** `tests/dyndiff` runs two machines side by side in batches
and compares them after each:

- the registers, CR0, CR2, CR3 and CPL;
- the instruction counts and exceptions;
- RAM, video memory, the palette and the debug console.

The host's time is fixed for both (`hosttime::fix`).

| Test | Checks |
|---|---|
| `tests/dyndiff_tests.rs` | A protected-mode program with a fast timer interrupt |
| `tests/dynrec_tests.rs` | Stores into the rest of a block, faults and page faults in the middle of one, interrupt shadows, an interrupt a POPF lets through, a switch to a stack of another width, timer reads, a full code memory, the auto latch, rewriting a linked block, a RET poked into an unrolled loop, returns and indirect calls to several places, PUSHAD and POPAD past the stack's limit, and a smaller CS limit under a link |

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

which plays from about batch 4500. `DYNDIFF_EMS=1` gives the machines
expanded memory and upper memory blocks, as the front ends have them by
default: DOS extenders then switch modes through VCPI, with paging on.

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
