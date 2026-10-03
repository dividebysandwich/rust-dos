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
| a folder | C: is the folder. With a `rust-dos.conf` in it, it is a [game package](../GAME-PACKAGES.md): a profile is made of it in `saves/rust-dos/games` the first time, with its settings, drives, manuals and `[autoexec]`, and what the game writes goes to `saves/rust-dos/saves/<name>` (unless its `[game]` has `overlay=false`). A GOG install or a folder of DOSBox configurations is imported as a game profile |
| `.exe`, `.com`, `.bat` | C: is the program's folder, and the program runs |
| `.dosc` | its game's `.dosz` (or `.zip`) beside it, with the `.dosc` over it (see [Packages made for DOSBox](../GAME-PACKAGES.md#packages-made-for-dosbox)) |
| `.zip`, `.dosz`, `.7z` | a [game package](../GAME-PACKAGES.md): C:, read from the archive where it is, with a profile in `saves/rust-dos/games` made the first time and found after (made again when its configuration changes), from its `rust-dos.conf` if it has one; else it starts its one program, if it has one. What the game writes goes to `saves/rust-dos/saves/<name>` |
| `.conf` | a [game profile](../CONFIGURATION.md#game-profiles), launched; a DOSBox configuration is imported as one first |
| floppy image (`.img`, `.ima`, `.vfd`, …) | A:, with the prompt at A: |
| CD image (`.iso`, `.cue`) | D:, with the prompt at D: |
| hard disk image (`.img`, `.vhd`, `.hdd`) | C:. The image is left as it is: its changes go to a delta file in `saves/rust-dos/saves/<name>`, so one Windows install can be the base of many games (see [`-overlay`](../CONFIGURATION.md#mounting-drives)) |
| `.m3u` | a list of disk images for one drive: the first goes in, the frontend's *Disc Control* changes them |

On Android, content from folders RetroArch reaches through the system's file
picker (Storage Access Framework, `saf://` and `content://` paths) works
through the frontend's file access (VFS): folders as C:, disk and CD images,
archives and `rust-dos.conf` files. DOS sees the files there without their
dates, and renaming a file there copies it.

With the core option **Boot disk images**, floppy and hard disk images start
from their boot sector (`BOOT`) instead, for systems such as Windows 95 and
self-booting games.

A DOSBox `GAME.conf` beside `GAME.zip` or `GAME.dosz`, even one with only
an `[autoexec]`, is imported into the game's profile with the archive as C:
before its commands: `REMOUNT C D` moves the game to D:,
`SUBST` makes one of its folders a drive, and an `IMGMOUNT C` or `BOOT` of an
operating system's image beside it, or named from the OS images folder, takes
C:. `[rust-dos] os=win98se` in it starts the game in that system, as the
**Boot OS** option does. A `GAME.dosc` and the `DOS.YML` in either are used too
(see [Packages made for DOSBox](../GAME-PACKAGES.md#packages-made-for-dosbox)).
A package with launch configurations opens the settings window on the ways to
start it (pick with the D-pad, A to start). A game's gamepad mapping plays on
the first port set to **Gamepad as keyboard** in *Controls → Port 1 Controls*,
its action wheel included.

**OS images.** Hard disk images of installed operating systems go in
`system/rust-dos/os/`. The core
option **Boot OS** (*System*) lists them: with one chosen, a zip, `.dosz` or
folder without a configuration of its own starts that system from its image
(`BOOT -l C`), with the game as its second disk, D:. Each game keeps the
image's changes in its own delta file in `saves/rust-dos/saves/<name>`, so the
image stays as it was installed and one install serves every game. At the
prompt, `REMOUNT WIN98SE C` (or `IMGMOUNT C WIN98SE`) mounts an image by its
name.

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
The Roland Sound Canvas (`midisynth=sc55`) finds its ROMs in a `sc55-roms`
folder of the system directory or of its `rust-dos` folder, by their contents
(zip archives too); the core doesn't download them.

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
* **Ctrl+Shift+M** shows the running game's manuals and extras (its
  profile's `manual=` lines and `saves/rust-dos/games/<name>.extras`) over
  the picture.
* **Ctrl+Shift+F12** shows the performance overlay.

The program's save state, fast forward, rewind, pause and recording hotkeys
are the frontend's own here.

## Save states and RetroAchievements

Save states hold the whole machine, as the program's do, and the frontend
keeps them; rewind works through them. Files on the host and in disk images
aren't part of a state.

RetroAchievements are the frontend's: the core gives it the MS-DOS memory
map the DOS achievement sets read, with the game's memory at address 0. The frontend knows DOS games by
their `.zip` or `.dosz` archive, so load them that way for their
achievements. The Rust-DOS program's own RetroAchievements client isn't in
the core.

## What isn't in the core

* The 3dfx Voodoo is drawn in software; the program's OpenGL renderer and
  CRT shaders aren't there (the frontend has shaders of its own).
* MIDI out of the host's MIDI ports (`midisynth=host`).
* The debug server.
