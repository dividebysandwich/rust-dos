---
name: debug-emulator
description: Run and debug DOS programs in the rust-dos emulator through its HTTP/WebSocket debug server — launching headless, typing input, reading the screen, instruction traces, breakpoints, registers and memory. Use when asked to run a DOS program, reproduce or investigate emulator misbehaviour, or verify an emulator change in a real program.
---

Read `docs/llm-debugging.md` in the repository root before using the debug
server, and follow it. It covers launching the emulator headless, the
act-then-look input loop, debugging recipes, address syntax, and pitfalls.

Start the emulator with `--no-config` so a user's `rust-dos.conf` doesn't
change which drives exist or what runs at startup.

Launch headless with `SDL_VIDEODRIVER=dummy` alone, or `--hidden` for
OpenGL: the sound goes to the null output, which needs no sound device.
Never set `SDL_AUDIODRIVER=dummy`: SDL's dummy audio driver runs faster
than real time and must not be used for any measurement. For a timing run
or an underrun count, use a real device with `--mute`
(`SDL_AUDIODRIVER=wasapi` on Windows), or `--audio-output null` on a
machine without one, and check `audio_driver` in `/api/stats` or
`/api/underruns` before quoting the result.

For the full endpoint reference, query the running server: `curl localhost:8086/`.

When you are done, stop the emulator you started, by its own PID (`kill <pid>`), not
with `pkill` or `pgrep -x rust-dos`, which would stop other sessions' emulators too.
