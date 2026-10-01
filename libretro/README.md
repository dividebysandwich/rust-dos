# Rust-DOS as a libretro core

Rust-DOS runs in [RetroArch](https://www.retroarch.com/) and the other
[libretro](https://www.libretro.com/) frontends as the core `rust_dos_libretro`.
It is the same emulator as the Rust-DOS program, with the frontend's window,
sound, gamepads, save states and RetroAchievements.

## Installing

Take the `rust-dos-<version>-libretro-<system>.zip` for your system from the
[releases page](https://github.com/dividebysandwich/rust-dos/releases), and
copy:

* `rust_dos_libretro.so` (Linux), `.dll` (Windows) or `.dylib` (macOS) into
  the frontend's cores folder (in RetroArch, *Settings → Directory → Cores*);
* `rust_dos_libretro.info` into its info folder (*Core Info*).

To build it yourself you need Rust only (no SDL):

```sh
cd libretro
cargo build --release
retroarch -L target/release/librust_dos_libretro.so <content>
```

The libretro buildbot builds the core for RetroArch's *Online Updater* from
[.gitlab-ci.yml](../.gitlab-ci.yml): Windows, Linux, macOS, iOS and tvOS,
Android and webOS. The dynamic recompiler is there on x86-64 and AArch64;
on 32-bit ARM the interpreter runs everything, as it does where the system
allows no JIT (iOS and tvOS without a debugger's), which the core says as it
starts.

## Content

| Content | What the core does |
|---|---|
| none (*Start Core*) | starts at the prompt; C: is an empty folder, `saves/rust-dos/drive_c` |
| a folder | C: is the folder. With a `rust-dos.conf` in it, its settings apply, and its `[autoexec]` starts the game. A GOG install or a folder of DOSBox configurations is imported as a game profile |
| `.exe`, `.com`, `.bat` | C: is the program's folder, and the program runs |
| `.zip`, `.dosz`, `.7z` | C:, read from the archive where it is, with a profile in `saves/rust-dos/games` made the first time and found after; it starts its one program, if it has one. What the game writes goes to `saves/rust-dos/saves/<name>` |
| `.conf` | a [game profile](../CONFIGURATION.md#game-profiles), launched; a DOSBox configuration is imported as one first |
| floppy image (`.img`, `.ima`, `.vfd`, …) | A:, with the prompt at A: |
| CD image (`.iso`, `.cue`) | D:, with the prompt at D: |
| hard disk image (`.img`, `.vhd`, `.hdd`) | C: |
| `.m3u` | a list of disk images for one drive: the first goes in, the frontend's *Disc Control* changes them |

On Android, content from folders RetroArch reaches through the system's file
picker (Storage Access Framework, `saf://` and `content://` paths) works
through the frontend's file access (VFS): folders as C:, disk and CD images,
archives and `rust-dos.conf` files. DOS sees the files there without their
dates, and renaming a file there copies it.

With the core option **Boot disk images**, floppy and hard disk images start
from their boot sector (`BOOT`) instead, for systems such as Windows 95 and
self-booting games.

## Settings

Settings come in layers, each over the one before:

1. Rust-DOS's defaults;
2. `rust-dos.conf` in the frontend's system folder, in `system/rust-dos/`
   (or `system/` itself), with every setting of
   [CONFIGURATION.md](../CONFIGURATION.md) and `[drives]` and `[autoexec]`;
3. the core options (*Quick Menu → Core Options*), each of which leaves the
   setting to `rust-dos.conf` until it is changed;
4. the content's own profile or `rust-dos.conf`.

A SoundFont (`soundfont=`) and MT-32 ROMs (`mt32roms=`) are set in
`rust-dos.conf`; relative paths are relative to its folder, so they can be put
beside it. The MT-32 also needs munt's library (`libmt32emu`) on the system.

The settings window (**Ctrl+F12**, `DOSCONFIG` at the prompt, or **L3+R3** on
the gamepad) is there too: F2 saves into `rust-dos.conf`, or into the game's
profile while one plays. Hardware changes wait for the program running to end,
as in the program; the memory size (`memsize`) changes when the content starts
again.

## Controls

* **Keyboard**: every key of a PC keyboard. RetroArch's hotkeys take some
  keys first; turn on *Game Focus* (Scroll Lock by default) to give the whole
  keyboard to DOS. The keyboard layout DOS types with is the core option
  *Keyboard layout* (US by default).
* **Mouse**: RetroArch's mouse is the DOS mouse (*Mouse speed* scales it).
  With *Right stick moves the mouse*, the gamepad's right stick moves it and
  R2 and L2 are its buttons.
* **Gamepads**: ports 1 and 2 are the game port's joysticks (*Gamepad (game
  port joystick)*): the sticks, the D-pad and four buttons. As *Gamepad as
  keyboard*, a gamepad presses keys instead: the D-pad and the left stick
  are the cursor keys, B Ctrl, A Alt, Y Space, X Shift, Start Enter, Select
  Esc, L and R Page Up and Page Down, L2 Tab and R2 Backspace.
* **Ctrl+F4** puts the next disk in the drives mounted from lists of images.
* **Ctrl+Shift+F12** shows the performance overlay.

The program's save state, fast forward, rewind, pause and recording hotkeys
are the frontend's own here.

## Save states and RetroAchievements

Save states hold the whole machine, as the program's do, and the frontend
keeps them; rewind works through them. Files on the host and in disk images
aren't part of a state.

RetroAchievements are the frontend's: the core gives it the MS-DOS memory
map DOSBox Pure does, with the game's memory at address 0, so the DOS
achievement sets made for DOSBox Pure work. The frontend knows DOS games by
their `.zip` or `.dosz` archive, so load them that way for their
achievements. The Rust-DOS program's own RetroAchievements client isn't in
the core.

## What isn't in the core

* The 3dfx Voodoo is drawn in software; the program's OpenGL renderer and
  CRT shaders aren't there (the frontend has shaders of its own).
* MIDI out of the host's MIDI ports (`midisynth=host`).
* The debug server.
