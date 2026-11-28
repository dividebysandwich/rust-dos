# Rust-DOS documentation

The documentation comes in three kinds. Guides say how to get one thing
done and leave out the rest. Reference pages describe everything there
is about a subject, to look things up in. Developer pages are about how
Rust-DOS is built and tested.

New to Rust-DOS? Start with the [README](../README.md#start-here): it
says where to download it, how to start a first game, and which tutorial
fits what you want to play.

## Guides

| Page | What it helps with |
|---|---|
| [TUTORIALS.md](../TUTORIALS.md) | Whole configurations for common cases: a DOS game from a folder, a zip, a GOG install, a CD or floppies; better music; 3dfx games; Windows games on the built-in Windows, on the ToadForce and on a Windows 98 image |
| [win9x-howto.md](win9x-howto.md) | Making a Windows 95 or 98 disk image, installing its drivers, and running games in it |
| [3dfx.md](3dfx.md) | Which 3dfx card a game needs, and how to set it up |
| [builtin-windows.md](builtin-windows.md) | Using the built-in Windows: turning it on, installing games, and how it differs from Windows 98 |
| [GAME-PACKAGES.md](../GAME-PACKAGES.md) | Adding manuals to a game, and putting a game, its CD, settings and manuals in one zip |
| [compatibility.md](compatibility.md) | What particular games need: settings, files, drivers and known limits |
| [copy-protection.md](copy-protection.md) | Which copy protections run, and which image of a protected disc they need |
| [vr.md](vr.md) | Building a room in Blender for the 3D scene and VR headsets |

## Reference

| Page | What it describes |
|---|---|
| [CONFIGURATION.md](../CONFIGURATION.md) | Every setting of `rust-dos.conf`, the settings window, game profiles, drives and disk images, booting a disk image, the emulated graphics and sound cards, networking, and the command-line options |
| [REFERENCE.md](../REFERENCE.md) | The DOS prompt and its commands, the keyboard shortcuts, save states, the log file, the browser and libretro versions, the debug server and the profiler |
| [libretro/README.md](../libretro/README.md) | The libretro core: its options, content and frontend features |
| [vfx1.md](vfx1.md) | The VFX1 headset: the resolution each eye gets in the games that support it |
| [toadforce.md](toadforce.md) | The ToadForce card: why it exists, what it does and doesn't do, and how it is drawn |
| [toadforce-spec.md](toadforce-spec.md) | The ToadForce card's registers and commands, for driver writers |

## For developers

| Page | What it covers |
|---|---|
| [building.md](building.md) | Building Rust-DOS, its browser version and its libretro core; running the tests |
| [llm-debugging.md](llm-debugging.md) | Driving the emulator through its debug server, for scripts and AI agents |
| [profiling.md](profiling.md) | Finding where the emulator's time goes |
| [benchmarks.md](benchmarks.md) | The microbenchmarks of the emulator's hot paths |
| [game-suite.md](game-suite.md) | The game regression suite, and blessing its screenshots |
| [regression-programs.md](regression-programs.md) | Which programs and fixture scenes reach each area of the emulator, for a pull request's regression check |
| [cpu-tests.md](cpu-tests.md) | The CPU conformance suites against real hardware |
| [dynrec.md](dynrec.md) | The dynamic recompiler |
| [builtin-windows-architecture.md](builtin-windows-architecture.md) | How the built-in Windows is built |
| [AGENTS.md](../AGENTS.md) | How code and documentation are laid out, and how a pull request goes from draft to ready |
