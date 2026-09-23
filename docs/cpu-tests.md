# CPU conformance tests (SingleStepTests/80386)

`tests/sst386.rs` runs the [SingleStepTests/80386](https://github.com/SingleStepTests/80386)
real-mode suite against the emulator's CPU and reports pass rates per opcode.
The suite was captured from a real 80386EX. Each test gives the initial
registers and memory, runs one instruction plus the `HLT` (F4) after it, and
records the final registers and memory. That makes 1.76 million tests in 941
opcode files: `01`, `6601`, `6701` and `676601` are ADD with no prefix, the
66h (operand size) prefix, the 67h (address size) prefix, and both. Group
opcodes carry the ModR/M reg field in the name (`80.4` is AND).

The harness is opt-in (`#[ignore]`) and does not fail unless you ask it to,
so it can measure progress while the CPU is incomplete.

## Fetch the suite

```sh
tests/sst386/fetch.sh            # into target/sst386/ (about 580 MB)
```

This makes a shallow, sparse git clone with only `v1_ex_real_mode/`
(`*.MOO.gz`) and the top-level files. The harness reads two of them:
`revocation_list.txt` (hashes of bad tests) and `80386.csv` (per-opcode
metadata, including the undefined-flags mask `f_umask`). Run the script
again to update the clone.

## Run

```sh
SST386_DIR=target/sst386/v1_ex_real_mode \
  cargo test --release --test sst386 -- --ignored --nocapture
```

A full run takes a few seconds on a multi-core machine. The summary goes to
stdout (use `--nocapture`, or libtest hides it), and the full report goes to
`target/sst386-report.txt`.

| Variable | Meaning |
|---|---|
| `SST386_DIR` | Directory of the `.MOO.gz` files. If it is unset, the test prints a note and passes. |
| `SST386_FILTER` | Comma-separated substrings of file names, for example `0FB6` or `80.4,C1.`. The match is on the whole name, so `50.MOO` also matches `6650.MOO`. |
| `SST386_STRICT=1` | Fail unless every test that ran passed. Skipped tests don't count. |
| `SST386_SAMPLES` | Failure details kept per file in the report file (default 3). Stdout shows the first one. |
| `SST386_THREADS` | Worker threads (default: all cores). |

To work on one instruction, filter to it and ask for more samples:

```sh
SST386_DIR=target/sst386/v1_ex_real_mode SST386_FILTER=D0.0 SST386_SAMPLES=20 \
  cargo test --release --test sst386 -- --ignored --nocapture
```

## Reading the report

Each file gets one row: `tests`, `pass`, `fail`, `panic`, `skip`, and `pass%`
(passes divided by the tests that ran, which excludes skipped tests). A
failing file also gets its first failures, one per line:

```
#0 'rol dl,1' [D0 C2 F4] hash e17f10...: eflags got 00887 want 00087 (OF)
#1 'lock int 5Fh' [F0 CD 5F F4] hash 617d3f...: esp got 5586 want 5580; ... (test raises int 06h) | log: [CPU] Unhandled: (bad) ...
```

A failure line shows:

- **The test:** its index, disassembly, bytes and SHA-1 hash. The hash is
  the ID the revocation list uses.
- **Every difference:** registers that differ (for EFLAGS, the names of the
  differing flags), then memory bytes. `(unchanged)` marks a byte from the
  initial state that the final state doesn't list, so the instruction should
  have left it alone.
- **Extra context:** the exception or interrupt the real CPU took (from the
  suite's `EXCP` chunk), and the first line the emulator logged during the
  test.
- **Other outcomes:** `no HLT within 256 instructions` means the test never
  halted. `PANIC: ... (at src/...:line)` means the emulator panicked; the
  harness catches the panic and moves on to the next test.

After the file rows comes a table per opcode family (opcode map by size
prefixes) and the totals.

## What is compared

- **Registers:** EAX-EDI, ESP and EBP (32 bits), the CS/DS/ES/FS/GS/SS
  selectors, EIP, CR0, and EFLAGS bits 0-17. The suite's register dumps come
  from SMM, which sets EFLAGS bits 18-31 to 1, so those bits are ignored.
  CR3, DR6 and DR7 are not compared. The final state lists only registers
  that changed; any other register must still hold its initial value.
- **CPU model:** the harness sets `cpu.model = CpuModel::I386`, since the
  suite comes from a 386EX. 486 instructions then raise #UD, and the 386's
  quirks with undefined encodings apply (see below).
- **Undefined bits:** a bit is ignored if any of these marks it undefined:
  the file's `RM32` mask, the test's own mask, or the opcode's `f_umask` in
  `80386.csv`. The CSV marks OF undefined for every count of C0/C1 (shifts
  and rotates by an immediate), so OF isn't checked even for a count of 1.
  The suite's README warns about this.
- **Memory:** every byte the final state lists. Every other byte of the
  initial state must be unchanged. When the test raises an exception, the
  undefined flags are also ignored in the FLAGS image it pushes (the `EXCP`
  flag address).
- **Bus cycles:** not checked.

## Skipped tests

| Reason | Why |
|---|---|
| revoked | Its hash is in `revocation_list.txt`. |
| address beyond emulated RAM | A byte of its initial or final memory is at or above `bus.ram().len()`. With the default 16 MB, none are. |
| tripwire | The exec loop's IVT tripwire (CS=0, IP<100h, DS not 0) fired and asked for a shell reload. |
| service trap at CS:IP | The instruction under test starts with FE 38 or FE 39, the emulator's service traps (BOPs). Reaching one later in a test counts as a failure, because the real CPU would have halted first. |
| file `FE.7` | FE /7 is the BOP encoding. The suite has no such file today. |
| port input files | IN and INS (`E4`, `E5`, `EC`, `ED`, `6C`, `6D` and their prefixed forms). The suite expects every port to read FFh; the emulator's devices (PIT, PIC, port 61h, DMA) answer. |

## Harness setup

The tests assume flat, writable RAM and no interrupts, so each test gets
this setup:

- **RAM:** cleared at start. After each test, every 4 KiB page whose
  `bus.page_gen` moved is cleared again. That covers the test's own bytes
  and any stray writes.
- **Initial bytes:** written twice. The raw RAM write feeds instruction
  fetch. The bus write goes where data reads look, which is the VGA planes
  inside A0000-AFFFF.
- **VGA:** set to chain-4 with a full bit mask, so A0000-AFFFF reads back
  what was written, like RAM. B8000-BFFFF is the text buffer, which always
  reads back.
- **Interrupts:** all IRQs are masked at the PIC and the timer deadline is
  disabled, so POPF, IRET or STI setting IF can't let an interrupt in.
- **A20:** open. The 386EX the suite comes from has no A20 gate, so
  addresses above 1 MB don't wrap.
- **Emulator log:** lines are collected for the failure details. They don't
  reach `trace.log`, and stdout is silenced during the run.
- **Machines:** each file runs on a fresh `Cpu`, so results don't depend on
  thread scheduling.

`tests/sst386/machine.rs` is the only file that uses the emulator's API.
When the CPU changes (EFLAGS setter, CR0 accessor, larger RAM, a different
`page_gen`), update that file.

## Debugging a failing test

| Variable | Meaning |
|---|---|
| `SST386_VERBOSE=1` | Add each failing test's initial registers to its report line. |
| `SST386_INDEX=n` | Run only test `n` of each file, and print its initial memory to stderr. |
| `SST386_DUMP=1` | Print one line per test to stderr: `DUMP`, the name, the initial and expected registers, the initial and final memory, and the exception number, separated by `\|`. For offline analysis of how the hardware behaves across many inputs. |

## Current results

With the emulator's CPU rewritten for the 386, 1,749,683 of the 1,749,699
tests pass (one revoked test is skipped). The 16 failures:

- **REP STOS/MOVS writing over their own code** (4): the 386 has already
  fetched the next instructions into its prefetch queue, so it runs the
  bytes that were there before. The emulator decodes from memory.
- **IDIV byte** (9) and **AAM 0** (3): results that look like 386
  microcode quirks for rare operand combinations.

Hardware behaviour the emulator reproduces because the suite showed it:

- **EIP past FFFFh:** 16-bit code doesn't wrap IP at FFFFh; the next fetch
  raises #GP(0) at EIP 10000h.
- **Shifts by more than the operand width** (bytes and words): the result
  is 0, and CF is the last bit shifted out only when the count is a
  multiple of the width (16 or 24 for a byte); otherwise it's 0. SHR sets
  OF from the top two bits of the result.
- **SHLD/SHRD with 16-bit operands:** the 386 shifts dest:src:src
  (src:src:dest for SHRD), so counts of 16 to 31 rotate the source in.
- **PUSHA/POPA faults:** PUSHA stores from the lowest slot up and POPA
  loads register by register, so a stack fault part way leaves the slots
  or registers done before it.
- **32-bit POP of a segment register** reads only the selector word.
- **386 model only:** a SIB byte with no index (100b) but a scale above 1
  scales the base register, and POPAD on a 16-bit stack loads the upper
  half of ESP from the ESP image it otherwise skips.
- **DAS/DAA** compare the original AL with 99h (the 8086 compares the
  adjusted AL with 9Fh).
