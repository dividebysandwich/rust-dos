# Game regression suite

`tests/game_suite.rs` plays real DOS games on the emulator. The games are
shareware and freeware: Doom, Heretic, Duke Nukem 3D, Descent, Raptor,
Wolfenstein 3D, Commander Keen, Jill of the Jungle, Cosmo, Duke Nukum,
Scorched Earth, and the early PC games Alley Cat, Digger, Sopwith and Rogue.

Each game is played on machines set up in many ways:

- **CPU:** 386, 486 and Pentium, at speeds from 300 to 80000 cycles.
- **Core:** the interpreter and the dynamic recompiler.
- **Video:** CGA, EGA and VGA.
- **Sound:** the PC speaker, SB 2.0, SB Pro 2, SB16, Gravis Ultrasound,
  General MIDI and the Disney Sound Source.
- **Links between two machines:** IPX, a null-modem cable and a modem call.

The suite is opt-in and local. It downloads about 20 MB of games on first
use, and it never runs in CI or in a plain `cargo test`.

```sh
cargo test --release --test game_suite -- --ignored --nocapture        # fetch, install, play
cargo test --release --test game_suite -- --ignored fetch --nocapture  # only download and install
cargo test --release --test game_suite -- --ignored suite --nocapture  # only play
```

Always use a release build. On a 16-core machine a full run takes about a
minute and a half.

| Variable | Meaning |
|---|---|
| `GAME_SUITE=doom,keen1/ega` | Run only the scenarios whose name contains one of these parts. |
| `GAME_SUITE_BLESS=1` | Record the golden hashes, rather than checking against them. |
| `GAME_SUITE_JOBS=4` | Scenarios run at once, one thread each. The default is the number of CPUs. |
| `GAME_SUITE_CACHE=dir` | Where downloads, installed games and blessed pictures go. The default is `~/.cache/rust-dos/game-suite`. |
| `GAME_SUITE_SNAP=1000` | Save the screen every so many emulated milliseconds: PNG in graphics modes, text in text modes. Use it when writing a script. |

## What a scenario checks

A scenario is a game, a configuration and a script. The script presses keys
at emulated times and waits for conditions: a text on the screen, a video
mode, the prompt being back. It takes **shots** (hashes of the picture) and
**listens** (hashes of the sound over a stretch of time).

A scenario fails in any of these cases:

- a wait times out;
- a check doesn't hold;
- a `Listen` hears silence;
- the emulator panics;
- a hash differs from the golden file in `tests/gamesuite/golden/`.

The machines are deterministic, so every run has to give the same hashes:

- the speed is fixed;
- input is timed in emulated milliseconds;
- the real-time clock starts at 1995-04-11 12:34:56 and runs with emulated time.

Scenarios named `…-normal` and `…-dynamic` share one golden file, so the
recompiler has to produce the same pictures and sound as the interpreter.

Linked two-machine scenarios (`net-*`) depend on the relay's timing on the
host clock. They check only that both machines reach the game and stay in
it; they have no hashes.

A failed run leaves its files in `target/game_suite/<game>__<variant>/`:

- the PNG and WAV of each shot and listen;
- `last.png`;
- `screen.txt` in text modes;
- `rust-dos.log`;
- the scenario's drive C: (`c/`, and `c2/` for the second machine).

Next to a changed picture or sound it also puts the one recorded at the
last bless, as `<label>.expected.png` or `.wav`, kept in the cache.

`target/game_suite/report.md` lists every scenario with its result, sound
peaks, picture sizes and colours, and the log lines that look like trouble
(unhandled interrupts, unknown DSP commands).

After a change that alters a game's picture or sound on purpose, check the
new pictures, then re-record:

```sh
GAME_SUITE_BLESS=1 cargo test --release --test game_suite -- --ignored suite --nocapture
```

## Known failures

A scenario marked `expect_fail` stays in the suite and is reported as
`known-fail`. If it starts passing, the run fails with "unexpected pass", so
that someone removes the mark.

| Scenario | Why |
|---|---|
| `doom/timedemo-sbpro2`, `raptor/sbpro2`, `descent/sbpro2` | On an SB Pro 2 or SB 2.0, DSP commands that only the SB16 takes parameters for (41h, 0Eh) run without them. `src/sb.rs` panics on an index out of bounds. |
| `descent/sb2` | Descent's setup exits (code D2h) while detecting an SB 2.0, after DSP command E7h. |
| `duke3d/setup-50000` | Duke Nukem 3D's setup calibrates a delay loop around INT 21h AH=2Ch. That call costs next to nothing here, so at 50000 cycles the loop runs for hours. The other Duke scenarios run the setup at 20000. |

## Adding a game

Everything about the games is in `tests/gamesuite/catalog.rs`.

1. Add an `archive!` with the download URL and the archive's SHA-256.
2. Add a `Game` to `games()`. Archives that run as they are use
   `Install::Unpack`. Archives with an installer use `Install::Installer`,
   which unpacks the archives onto D: and runs the installer onto an empty C:
   with a script. id's De-ICE installers share `deice()`. The installed tree
   is kept in the cache and copied afresh for every scenario.
3. Write the script with `GAME_SUITE_SNAP=1000` to see what the game does.
   For text screens, `snap-*.txt` are easier to read than pictures.
   - `WaitFor(Cond::Text(..))` is sturdier than fixed waits for setup programs.
   - `PressUntil` handles dialogs that may or may not come up.
   - `Cycles(n)` changes the speed mid-script, for example a slow setup
     before a fast game.
   - `Focus(1)` sends input to the second machine of a linked pair.
4. Build the variants with `Hw` and `sound(..)`. Use `both_cores(..)` for a
   scenario that has to come out the same on both cores.
5. Bless, look at the shots in `target/game_suite/`, then run twice more
   without bless to see that the scenario is deterministic.

## How the harness runs a machine

`tests/gamesuite/machine.rs` builds the machine the way `src/main.rs` does:

- `config::parse` of the scenario's INI text;
- `Settings::from_config`;
- `hardware::configure`;
- mounting the other drives;
- `load_shell`;
- queuing the `[autoexec]` lines.

It then runs one emulated millisecond at a time. After each millisecond it
calls `Bus::sync_display()`, so the CRTC latches its start address as it
does in the window, and it mixes the sound every 10 ms.

The window's own loop has a few behaviours the harness copies:

- **Start address.** Without `sync_display`, page-flipping games show a
  stale page.
- **Clock.** The thread's clock (`hosttime::fix`) is advanced with emulated
  time. A frozen clock hangs programs that wait for the RTC's seconds to
  change, such as Duke Nukem 3D's setup.
- **Second machine.** The second machine of a pair runs 4987.654 s ahead.
  DOOM's SERSETUP takes its ID from the time, and two machines with the same
  ID stop with "Duplicate id string".
