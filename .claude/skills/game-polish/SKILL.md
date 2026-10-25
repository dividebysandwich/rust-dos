---
name: game-polish
description: Play a DOS game in the rust-dos emulator for a set time (30 minutes by default), file a GitHub issue for every emulation glitch, bug, performance problem and bottleneck found, then fix each one in its own git worktree and check the fix against the same game. Use when asked to polish, play-test or shake out bugs in a game, or to find and fix what goes wrong when a game runs in rust-dos.
argument-hint: <game> [minutes] [--no-fix] [--no-issues]
---

# Game polish

A play session, then a fix session. Play the game the way a player would,
watch the emulator while doing it, and write down everything that is
wrong. Then file each finding as an issue and fix it in its own worktree,
validated against the game it was found in.

Read `docs/llm-debugging.md` before you start the emulator, and follow it
and the `debug-emulator` skill for launching, input and stopping. This
skill covers what to do on top of that.

## Arguments

- **game** (required): a directory with the game's files, a game package
  (a zip, a 7z archive or a folder, see `GAME-PACKAGES.md`), or the name
  of a game in the game suite (`tests/gamesuite/catalog.rs`, for example
  `doom` or `keen1`). If none is given, ask which game.
- **minutes** (default 30): wall-clock time to spend playing. Setup,
  triage and fixing don't count toward it.
- **`--no-fix`**: stop after filing the issues.
- **`--no-issues`**: don't file issues; write the findings to the report
  only, and fix them only if asked.

## Phase 0: set up

1. **Work directory.** Put everything from the session in one directory
   outside the repository, in your scratchpad if you have one, else
   `/tmp/game-polish-<game>-<date>`. Call it `$W`. It holds the log,
   screenshots, save states, profiles and `findings.md`.
2. **The game.**
   - A suite game: `cargo test --release --test game_suite -- --ignored
     fetch --nocapture` downloads and installs it into
     `~/.cache/rust-dos/game-suite`; copy its installed tree into `$W/c`,
     so the play session doesn't change the cached copy.
   - A directory: copy it into `$W/c` for the same reason, unless it is
     large; then use it in place and say so in the report.
   - A package: unpack it into `$W/c`, and read its `rust-dos.conf` (or
     DOSBox configuration) for the machine it wants: pass those settings
     on the command line or with `--config $W/game.conf`.
3. **Build** a release build of the current `HEAD` (`cargo build
   --release`; on Linux it needs `libsdl2-dev` and `libasound2-dev`).
   Record `git rev-parse HEAD`: every finding is against this commit.
4. **Launch** headless on a free port, with the log in `$W`:

   ```sh
   : > $W/rust-dos.out
   SDL_VIDEODRIVER=dummy SDL_AUDIODRIVER=dummy \
     ./target/release/rust-dos --no-config -d $W/c \
     --debug-server 127.0.0.1:0 > $W/rust-dos.out 2> $W/rust-dos.err &
   echo $! > $W/emu.pid
   until H=$(grep -om1 '127\.0\.0\.1:[0-9][0-9]*' $W/rust-dos.out); do
     sleep 0.2; done
   ```

   Use `--deterministic --cycles N` only if you need exact repeats; a
   player's session normally runs at `cycles=auto`, which is part of what
   is being tested.
5. **Start the watcher**, so speed drops and log trouble are recorded
   while you play without spending context on them:

   ```sh
   .claude/skills/game-polish/scripts/watch.sh $H $W/watch 5 &
   ```

   It writes `stats.jsonl` (speed, frame rate, CPU use, video mode, audio
   peak and underruns every 5 s), `alerts.log` (samples that ran slower
   than real time or saturated the host thread) and `trouble.log` (log
   lines such as `Unhandled`, `Unknown`, `Invalid opcode`, panics). It
   stops when the emulator does.
6. **Note the start time.** Play until `minutes` have passed since now.

## Phase 1: play

Play for the whole time, and cover as much of the game as you can rather
than replaying one level. A useful order:

1. **Install and setup programs.** Run `SETUP`/`INSTALL` if the game has
   one; choose each sound card the setup offers in turn, if time allows.
2. **Title, intro and menus.** Watch the intro through once, then open
   every menu and options screen.
3. **The game itself.** Start a new game and play: move, fight, open
   doors, collect things, die, use the in-game save and load, change the
   detail level or screen size, pause and quit to the menu.
4. **Later content.** Use the game's level select, cheat codes or the
   emulator's cheats, if it has them, to reach later levels, bosses and
   the ending, which use code the first level doesn't.
5. **Exit to DOS.** Quit the game properly and check the prompt comes
   back with a clean screen and the right video mode.

Work in the act-then-look loop. Use `/api/screen/text` in text modes and
`/api/screenshot` in graphics modes, and look at each screenshot rather
than assuming what it shows. Every few minutes, `tail` the watcher's
`alerts.log` and `trouble.log`, and read `/api/status` for the audio
`peak` (0 while the game should make sound is a finding).

**Save a state at the start of every new area** (`POST
/api/state/save`), named after where it is, such as
`$W/states/e1m3-start.state`. When something goes wrong, the nearest state
is how the finding is reproduced and how the fix is checked.

### What counts as a finding

Report what is wrong in the emulator, not the game's own design. A good
question to ask: would this happen on a real PC of the time, or in
DOSBox? If it would, it isn't a finding, unless it is a known bug in the
game that the emulator should work around the way real machines did.

| Kind | What to look for |
|---|---|
| **Crash or hang** | A panic, a freeze, `Invalid opcode`, a triple fault, a return to the prompt with an error, input that stops working. |
| **Graphics** | Wrong colours or palette, flicker, tearing, half-drawn frames, missing sprites or text, garbage on the screen, a wrong aspect or a cut-off picture, a mode switch that fails. |
| **Sound and music** | Silence, crackle, notes that hang, wrong speed or pitch, effects missing, music that stops, underruns. |
| **Input** | Keys lost or stuck, a mouse that drifts or doesn't reach an edge, a joystick that isn't seen. |
| **Timing** | The game running too fast or too slow, a timer-driven delay that is wrong, a setup that misdetects the CPU speed. |
| **Performance** | `emulated_speed` under 1 or `cpu_use` near 100 in `stats.jsonl`, frame rate drops, stutter. Profile it (below). |
| **Emulator gaps** | Lines in `trouble.log`: an unhandled interrupt or DOS function, an unknown port or DSP command. Each distinct one is a finding, even when nothing visible breaks. |
| **Emulator UI** | Wrong on-screen messages, settings that don't apply, the settings window misbehaving with the game. |

### Recording a finding

The moment you see one, before playing on:

1. **Capture.** A screenshot (`$W/shots/<n>-<slug>.png`), the log lines
   around it (`/api/log?limit=100&format=text`), `/api/status`, and a save
   state from just before it if you can get one (load the area's state,
   play up to the moment, save).
2. **Reproduce once.** Load the state and do it again. Write down whether
   it happens every time, sometimes or once.
3. **Narrow it, briefly.** Spend a few minutes, not more, on the likely
   cause: the trace around it, the unhandled call in the log, `pic`,
   `video.vga` or the audio fields in `/api/status`. For a performance
   finding, profile 30 seconds of it:

   ```sh
   curl -s -X POST $H/api/profile -H 'content-type: application/json' \
     -d '{"enabled":true,"duration_ms":30000}'
   # ...play the slow part...
   curl -s "$H/api/profile?wait_ms=40000&format=text" > $W/profiles/<n>.txt
   ```

   The hottest emulator zones and guest places are the bottleneck; see
   `docs/profiling.md`.
4. **Write it down** in `$W/findings.md`: a number, a short title, the
   kind, what you did, what happened, what should have happened, how often,
   the files you captured, and the suspected cause if you have one.

Then play on. The fix session is the place for long investigations; the
play session is for coverage.

### When time is up

Quit the game, stop the watcher, and stop the emulator by its own PID
(`kill $(cat $W/emu.pid)`), never with `pkill`. Read through
`stats.jsonl`, `alerts.log` and `trouble.log` once more for findings you
didn't record while playing, such as a speed drop in one video mode.

## Phase 2: triage and file issues

1. **Merge duplicates.** One finding for each cause: five flickering
   screens with the same unhandled VGA register are one issue.
2. **Check what is already filed.** Search the repository's open and
   closed issues for each finding (its error message, the port or
   function, the game's name). If an open issue covers it, add a comment
   there with the new evidence instead of filing another; if a closed one
   does, it is a regression, so file it and link the old one.
3. **Rank.** Crashes and hangs first, then what breaks play (lost input,
   missing graphics), then sound, performance and cosmetic findings, then
   gaps nothing visible depends on.
4. **File one issue for each.** Skip this with `--no-issues`. Find the
   repository from `git remote get-url origin`, and look at its labels
   first, using the ones that fit (such as `bug`, `performance`,
   `emulation`) and inventing none. Title: what goes wrong and where,
   such as "Doom: music stops after loading a saved game (SB16)". Body:

   ```markdown
   ## What happens
   <one paragraph>

   ## What should happen
   <one paragraph; how a real PC or DOSBox behaves, if known>

   ## How to reproduce
   1. Game: <name, version, where it came from>
   2. Settings: <machine, sound card, core, cycles, other options>
   3. Steps: <from the prompt, or from the save state's point>
   Happens: <every time | sometimes (n of m) | once>

   ## Evidence
   <log lines, status fields, profile excerpt; the screenshot>

   ## Suspected cause
   <the code or the missing feature, if found; else "unknown">

   Found at <commit> during a <minutes>-minute play session.
   ```

   Screenshots and save states can't be attached through the API:
   describe what the screenshot shows, and keep the files in `$W` so the
   fix session can use them. Write them to the repository only if the
   user asks.

Keep a table of finding number, issue number and title; the fix session
and the report use it.

## Phase 3: fix, each in its own worktree

Skip this with `--no-fix`. If your environment limits you to one branch,
or forbids opening pull requests, ask before going on: put every fix on
that branch as its own commit only if the user agrees.

Fix the issues in rank order. For each one:

1. **Make a worktree** from the latest default branch, with a branch named
   after the issue:

   ```sh
   git fetch origin <default>
   git worktree add ../rust-dos-wt/issue-<N> -b fix/issue-<N>-<slug> \
     origin/<default>
   ```

   Independent issues can be fixed in parallel, by subagents that each
   get one worktree, the issue, and the paths in `$W` that belong to it.
   Run at most about three at once: each needs its own release build, and
   so its own `target/` directory of several GB. Delete a worktree's
   `target/` when its fix is done.
2. **Find the cause** in the emulator, with the save state and the
   `debug-emulator` skill. Fix the cause, not the one game: a fix keyed to
   a game's name or a program's bytes isn't one, unless the emulator
   already handles that quirk that way.
3. **Add a test** in `tests/` that fails without the fix and passes with
   it, written like the tests around it. Where the finding can't be
   reduced to a test (a timing feel, a performance figure), say so in the
   pull request.
4. **Check the code** the way `AGENTS.md` asks: rustfmt's style for what
   you wrote, without reformatting other lines; `cargo clippy` clean for
   your code; `cargo test --release` for the tests that cover the code
   you changed.
5. **Open a draft pull request.** Commit in the repository's style, push
   the branch, and open the pull request as a draft. It follows
   `.github/pull_request_template.md`, with `Fixes #<N>` under related
   issues; leave manual testing to be filled in after step 7.
6. **Review it, and push the review's fixes.** In the worktree, run the
   code review skill (`/review`, named `code-review` in some setups) on
   the branch's changes:

   ```
   /review high --fix do not skip optional improvements, make judgement
   calls prioritizing long-term quality and architecture over simplest
   solution / smallest scope
   ```

   Apply each finding that is a net improvement, optional ones included,
   choosing the change that is better for the code in the long run over
   the smallest one. Leave out a finding that the review or you judge not
   actually useful (a false positive, or a change that costs more than it
   gives), and say in the pull request which ones were left out and why.
   Check the result again as in step 4, commit, and push to the pull
   request's branch. Validation in step 7 runs on this pushed head, never
   on the code from before the review: if validation leads to more
   changes, push them, and run the review again on what changed.
7. **Validate against the game.** Build the worktree's release binary
   from the pushed head, then run the game as in Phase 0 with that
   binary:
   - Load the finding's save state and repeat the steps. A state may not
     load into a build whose machine changed; then replay from the
     prompt with the steps in the issue.
   - The finding is fixed only if it no longer happens, as often as it
     happened before (ten times for one that happened sometimes).
   - For a performance finding, measure before and after on the same
     state and input: `/api/stats` or a 30-second profile from each
     build, and give both figures. Use `--deterministic --cycles N` for
     both runs when the figure varies between runs.
   - Play a few minutes around the place, to see the fix broke nothing
     nearby.
   - If the game is in the game suite, run its scenarios
     (`GAME_SUITE=<game> cargo test --release --test game_suite --
     --ignored --nocapture`); if the fix changes the hashes on purpose,
     look at the new shots before blessing them.
8. **Mark the pull request ready for review** once validation succeeds:
   write what step 7 did and found under manual testing, then take it out
   of draft. If validation fails, go back to step 2; the pull request
   stays a draft until a pushed head passes.
9. **If it can't be fixed** in reasonable time, comment on the issue with
   what you found, and go on to the next one. Leave the draft pull
   request, if there is one, and the worktree for whoever goes on with
   it, and say where they are in the report.

Remove each worktree (`git worktree remove`) once its pull request is
ready for review and nothing in it is still needed.

## Phase 4: report

Finish with a short report to the user:

- the game, the settings, the commit, and how long you played;
- what you covered, and what you didn't reach;
- a table: finding, kind, issue, pull request (draft or ready),
  validated (yes, no, or how), and what is left;
- the speed figures from `stats.jsonl`: the slowest scenes and their
  `emulated_speed`, `fps` and `cpu_use`;
- where `$W` is, with the save states and screenshots, since they aren't
  in the issues.
