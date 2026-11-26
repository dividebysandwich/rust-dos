# Rust-DOS reference

The complete description of using Rust-DOS: its DOS prompt and commands,
the keys, save states, the log, the browser and libretro versions, and
the debug server. Every setting of the configuration file is in
[CONFIGURATION.md](CONFIGURATION.md). To get started instead, see the
[README](README.md#start-here) and the [tutorials](TUTORIALS.md).

* [The DOS prompt](#the-dos-prompt): [batch files](#batch-files),
  [commands](#commands) and [the prompt's line editing](#the-prompt)
* [Keyboard shortcuts](#keyboard-shortcuts)
* [Save states](#save-states)
* [Running in a browser](#running-in-a-browser)
* [As a libretro core](#as-a-libretro-core)
* [Debug and remote-control server](#debug--remote-control-server)
* [Profiling](#profiling)
* [Log file](#log-file)

## The DOS prompt

### Batch files

`.BAT` files run as COMMAND.COM runs them, a line at a time, with `%0` to
`%9` for the parameters, `%NAME%` for environment variables, `ECHO OFF` and
`@`, `:labels`, `GOTO`, `IF [NOT] ERRORLEVEL n`, `IF [NOT] EXIST file`,
`IF [NOT] a==b`, `FOR %%v IN (set) DO`, `SHIFT`, `CALL`, `PAUSE` and
`CHOICE`. A batch file started from another takes its place unless it is
`CALL`ed, and Ctrl+C between two lines ends them. The configuration's
`[autoexec]` and a game profile's commands run as batch files too, before
`C:\AUTOEXEC.BAT`.

### Commands

A program or batch file is found in the current directory and then in
`PATH` (`C:\;Z:\` to begin with); without an extension, the first `.COM`,
`.EXE` or `.BAT` of that name. `Z:\COMMAND.COM` is the `COMSPEC`, which
programs that shell out to DOS run (`C:\COMMAND.COM` works too). Besides
programs, the prompt has these commands:

| Command | What it does |
|---|---|
| `DIR [path] [/W] [/P] [/B]`, `LS [/A] [path]` | list files: `/W` five names a line, `/P` a screenful at a time, `/B` names only; `LS` in wide columns |
| `CD [path]`, `D:` | change the directory, or the drive |
| `TYPE file` | show a text file |
| `MORE [file]` | show text a screenful at a time |
| `HISTORY [n] \| DELETE n \| CLEAR` | list the lines typed at the prompt, or forget them; see [The prompt](#the-prompt) |
| `EDIT [file]` | edit a text file full-screen, as MS-DOS's EDIT: menus (Alt or F10), Open and Save As dialogs, cut, copy and paste (Shift+Del, Ctrl+Ins, Shift+Ins, or Ctrl+X, C, V), Find (F3 repeats it) and Replace; F1 lists the keys |
| `RC` | Rat Commander, a file manager in the style of Norton Commander 4: two panels (Brief, Full, Info, Tree, Quick view, sorted by name, extension, time or size), marking (Ins, Gray + - *), copy (F5), rename or move (F6), make a directory (F7), delete (F8), view (F3), edit in `EDIT` (F4), drives (Alt+F1, Alt+F2), the menus (F9) and the mouse. Enter opens a `.ZIP` archive as a directory, to copy, move, rename and delete in, out of and within it. Its command line runs programs, and RC comes back when they end. F1 lists the keys |
| `FIND [/V] [/C] [/N] [/I] "text" [file ...]` | the lines with the text in them (`/V` without it), numbered with `/N`, counted with `/C`, in any case with `/I`; `ERRORLEVEL` 0 if one was found, 1 if none |
| `SORT [/R] [/+n] [file]` | lines in order, from their n-th column, backwards with `/R` |
| `COPY source[+source...] [destination] [/A\|/B]` | copy files, with wildcards, or join them into one |
| `DEL file`, `ERASE file` | delete files, all of a directory's when given one |
| `REN old new`, `RENAME` | rename files, with wildcards (`REN *.TXT *.BAK`) |
| `MD dir`, `MKDIR`, `RD dir`, `RMDIR` | make and remove directories |
| `VOL [d:]` | a drive's label and serial number |
| `KEYB [code]` | type in another [keyboard layout](CONFIGURATION.md#emulator), or show which |
| `DATE [mm-dd-yy]`, `TIME [hh:mm[:ss]]` | set the machine's date or time, or show it |
| `CLS`, `VER`, `ECHO`, `SET`, `PATH`, `PROMPT` | as in DOS |
| `VERIFY [ON\|OFF]`, `BREAK [ON\|OFF]` | set DOS's verify and Ctrl+Break check flags, or show them; programs can read them, and they change nothing else |
| `MOUNT`, `IMGMOUNT` | see [Mounting drives](CONFIGURATION.md#mounting-drives) |
| `REMOUNT from to` | move a mounted drive to another letter |
| `SUBST drive: path` | make a folder a drive of its own (`/D` removes it) |
| `MAKEIMG` | make a new floppy or hard disk image, as in DOSBox Staging; see [New disk images](CONFIGURATION.md#new-disk-images) |
| `BOOT [image ...] [-l drive]` | start a system from a disk image; see [Booting a disk image](CONFIGURATION.md#booting-a-disk-image) |
| `MIXER` | see [`[mixer]`](CONFIGURATION.md#mixer) |
| `LAN [HOST\|JOIN\|LIST\|LEAVE\|DISBAND\|STOP]` | join rust-dos instances into a LAN; see [Playing over a LAN](CONFIGURATION.md#playing-over-a-lan) |
| `LOADHIGH` (`LH`) | load a program into upper memory |
| `MEM [/C\|/D\|/F\|/M name]` | the memory used and free, as in MS-DOS 6.22; `/C` lists the programs in memory, `/F` the free blocks, `/M` one program's blocks and `/D` all of them |
| `DOSCONFIG` | open the settings window |
| `EXIT` | quit Rust-DOS, or go back from `COMMAND` |
| `COMMAND [/C command \| /K command]` | a second prompt, until `EXIT`; `/C` runs the command and goes back, `/K` runs it and stays |

`>file`, `>>file` and `<file` redirect a command's output and input, and
`|` pipes one command's output into the next (`TYPE README.TXT | MORE`,
`DIR | SORT | FIND "EXE"`), as DOS does, through `PIPEn.$$$` files in `TEMP`
or the current drive's root, deleted once the line has run. `MORE`, `FIND`
and `SORT` read a pipe, `<file` or the file they are given; `PAUSE`,
`CHOICE`, `DATE` and `TIME` take their keys or lines from a pipe too
(`ECHO Y | CHOICE`). `PROMPT` takes DOS's `$` codes (`$P$G` is the
default), and ANSI colors after `$E` (`$E[1;33m`). DOS's output, `ECHO`'s
and programs', takes ANSI.SYS's escape sequences as DOSBox's does:
colors (`ESC[1;33m`), moving the cursor (`ESC[r;cH`, `ESC[nA` to
`ESC[nD`, `ESC[s`, `ESC[u`) and clearing (`ESC[2J`, `ESC[K`). The
`ansi_sys` setting turns them off.

### The prompt

The prompt edits its line as [clink](https://chrisant996.github.io/clink/)
does for `cmd.exe`. It colors the commands it knows, the programs it finds,
switches and arguments, and it suggests the rest of the line in dark grey
from the history. The history is kept between sessions. The
[`[shell]`](CONFIGURATION.md#shell) section has the settings and colors.

| Key | Action |
|---|---|
| Left, Right, Home, End | move the cursor; Right or End at the end takes the suggestion |
| Ctrl+Left, Ctrl+Right | move a word; Ctrl+Right at the end takes a word of the suggestion |
| Backspace, Del | delete a character anywhere in the line |
| Ctrl+Backspace, Ctrl+W | delete the word before the cursor (Ctrl+Backspace stops at a `\`) |
| Ctrl+Del | delete the word after the cursor |
| Ctrl+Home, Ctrl+U / Ctrl+End, Ctrl+K | delete up to the cursor / from the cursor on |
| Ins | overwrite or insert |
| Ctrl+Z | undo |
| Esc | clear the line |
| Up, Down (F5) | the lines typed before |
| PgUp, PgDn (F8) | the lines typed before that begin as the line does up to the cursor |
| Ctrl+R, Ctrl+S | search the history as you type; Ctrl+R again for an older match, Enter runs it, Esc gives up |
| F7 | the history in a window: type to narrow it, Enter runs a line, Tab edits it, Del forgets it |
| F1, F3 | copy a character / the rest of the previous line, as in COMMAND.COM |
| Tab, Shift+Tab | complete a command, program, file, directory (after `CD`) or variable (after `SET` or `%`), and go through the others |
| Ctrl+Space, Alt+= | list what Tab would complete to |
| Ctrl+C | give up the line |

`HISTORY` lists the lines, `HISTORY CLEAR` forgets them all and
`HISTORY DELETE n` forgets one. A line that begins with a space isn't kept.

## Keyboard shortcuts

These are the keys of the `rust-dos` hotkey profile, the default.
[`[hotkeys]`](CONFIGURATION.md#hotkeys) in the configuration file picks
another emulator's keys ([hotkey profiles](#hotkey-profiles)) or changes
single keys, and the settings window's Emulator page picks the profile.
A hotkey's key isn't sent to the program running in the machine.

| Key | Action |
|---|---|
| Ctrl+F12 | Open or close the [settings window](CONFIGURATION.md#settings-window) |
| Right mouse button (held still for 1.5 s) | Open the settings window without a keyboard (`mouse_menu_hold`) |
| Ctrl+Shift+F12 | Show or hide the performance overlay: the frames a second and the host's CPU use, with small graphs, at the bottom right, and what the [frame rate cap](CONFIGURATION.md#frame-rate-cap) holds while a game's frames go through it |
| Ctrl+Shift+D | Degauss the monitor: the picture wobbles with colour fringes, with the coil's thump with [`monitor_effects`](CONFIGURATION.md#emulator) sound |
| Ctrl+Shift+S | Show or hide the sound visualizer: an oscilloscope of each sound source, the FM synthesizer's channels, the Ultrasound's voices and the MIDI notes held; click one to mute it, or a source's S to hear it alone (until the visualizer closes) |
| Ctrl+F1 | Save the machine to the current [save state](#save-states) slot |
| Ctrl+F2 | Load the current slot |
| Ctrl+F3 | Pick the next slot (it says what is in it) |
| Ctrl+Shift+F3 | Pick the previous slot |
| Ctrl+F9 | Open the settings window on the save states |
| Ctrl+Shift+M | Show the running game's [manuals and extras](CONFIGURATION.md#manuals-and-extras) over the picture, and hide them |
| Ctrl+F4 | Put the next disk in the drives mounted from lists of images |
| Alt+Pause | Pause the machine, and resume it |
| Alt+F12 (held) | Fast forward: the machine runs up to eight times as fast, without sound |
| Alt+F11 (held) | Rewind: go back in time, half a second every third frame (with `rewind=true`) |
| Ctrl+F11 | Slow the CPU down by a tenth (from `max`, from the speed it reached) |
| Ctrl+Shift+F11 | Speed the CPU up by a tenth |
| Ctrl+F8 | Turn the sound off and on (in the browser, the page's Sound button) |
| Ctrl+Alt | Capture the mouse for the program, and let it go, as in VMware (pressed on their own and released). Moving the mouse over the window captures it by itself once a program uses the mouse driver, and the program's cursor leaving the screen lets it go (`mouse_autocapture`); a click captures it too |
| Ctrl+F10 | Capture the mouse and let it go, as in DOSBox |
| Ctrl+Shift+C | Copy the text selected by dragging with the right mouse button (while the mouse isn't captured), or the whole text screen, to the clipboard |
| Ctrl+Shift+V | Type the clipboard's text on the machine's keyboard (a key stops it) |
| Alt+Enter | Switch between the window and fullscreen |
| Ctrl+F5 | Save a screenshot (PNG), with the settings window and the performance overlay if they show and `record_ui` is on |
| Ctrl+Shift+F5 | Eject the printer's page and end the print job |
| Ctrl+F6 | Start and stop recording the sound (WAV) |
| Ctrl+F7 | Start and stop recording video with sound (AVI) |
| PrintScreen | Start and stop recording an animation (GIF) |
| Ctrl+Shift + mouse, Q, E, Home | In the [3D scene](CONFIGURATION.md#3d-scene-and-vr): fly its camera, and center a VR headset's view |

Screenshots and recordings go in `capture_dir` (`capture` in the directory
Rust-DOS started in).

## Hotkey profiles

`profile` in [`[hotkeys]`](CONFIGURATION.md#hotkeys), or *Hotkeys* on
the settings window's Emulator page, picks one of these sets of keys.
Each takes its emulator's default key for an action Rust-DOS has too,
and Rust-DOS's own key for the rest. A dash is an action without a key.

| Action | `rust-dos` | `pcsx2` | `duckstation` | `pcsx-redux` | `epsxe` | `retroarch` | `dosbox` | `86box` |
|---|---|---|---|---|---|---|---|---|
| `settings_window` | Ctrl+F12 | Ctrl+F12 | Ctrl+F12 | Ctrl+F12 | Ctrl+F12 | F1, Ctrl+F12 | Ctrl+F1 | Ctrl+F12 |
| `save_states_page` | Ctrl+F9 | Ctrl+F9 | Ctrl+F9 | Ctrl+F9 | Ctrl+F9 | Ctrl+F9 | Ctrl+F9 | Ctrl+F9 |
| `performance_overlay` | Ctrl+Shift+F12 | Ctrl+Shift+F12 | Ctrl+Shift+F12 | Ctrl+Shift+F12 | Ctrl+Shift+F12 | Ctrl+Shift+F12 | Ctrl+Shift+F12 | Ctrl+Shift+F12 |
| `sound_visualizer` | Ctrl+Shift+S | Ctrl+Shift+S | Ctrl+Shift+S | Ctrl+Shift+S | Ctrl+Shift+S | Ctrl+Shift+S | Ctrl+Shift+S | Ctrl+Shift+S |
| `manuals` | Ctrl+Shift+M | Ctrl+Shift+M | Ctrl+Shift+M | Ctrl+Shift+M | Ctrl+Shift+M | Ctrl+Shift+M | Ctrl+Shift+M | Ctrl+Shift+M |
| `save_state` | Ctrl+F1 | F1 | F2 | F1 | F1 | F2 | Ctrl+Shift+F1 | Ctrl+F1 |
| `load_state` | Ctrl+F2 | F3 | F1 | F2 | F3 | F4 | Ctrl+F2 | Ctrl+F2 |
| `next_slot` | Ctrl+F3 | F2 | F4 | Ctrl+F3 | F2 | F7 | Ctrl+F3 | Ctrl+F3 |
| `previous_slot` | Ctrl+Shift+F3 | Shift+F2 | F3 | Ctrl+Shift+F3 | Ctrl+Shift+F3 | F6 | Ctrl+Shift+F3 | Ctrl+Shift+F3 |
| `pause` | Alt+Pause | Alt+Pause | Alt+Pause | Pause | Alt+Pause | Alt+Pause | Alt+Pause | Ctrl+Alt+F1 |
| `fast_forward` | Alt+F12 | Alt+F12 | Alt+F12 | Alt+F12 | Alt+F12 | Alt+F12 | Alt+F12 | Alt+F12 |
| `rewind` | Alt+F11 | Alt+F11 | Alt+F11 | Alt+F11 | Alt+F11 | Alt+F11 | Alt+F11 | Alt+F11 |
| `slower` | Ctrl+F11 | Ctrl+F11 | Ctrl+F11 | Ctrl+F11 | Ctrl+F11 | Ctrl+F11 | Ctrl+F11 | - |
| `faster` | Ctrl+Shift+F11 | Ctrl+Shift+F11 | Ctrl+Shift+F11 | Ctrl+Shift+F11 | Ctrl+Shift+F11 | Ctrl+Shift+F11 | Ctrl+F12 | - |
| `fullscreen` | Alt+Enter | Alt+Enter | F11 | Alt+Enter | Alt+Enter | Alt+Enter | Alt+Enter | Ctrl+Alt+PageUp |
| `mouse_capture` | Ctrl+F10 | Ctrl+F10 | Ctrl+F10 | Ctrl+F10 | Ctrl+F10 | F11 | Ctrl+F10 | Ctrl+End |
| `mute` | Ctrl+F8 | Ctrl+M | Ctrl+F8 | Ctrl+F8 | Ctrl+F8 | F9 | Ctrl+F8 | Ctrl+Alt+M |
| `screenshot` | Ctrl+F5 | F8 | F10 | F12 | Ctrl+F5 | F8 | Ctrl+F5 | Ctrl+F11 |
| `record_sound` | Ctrl+F6 | Ctrl+F6 | Ctrl+F6 | Ctrl+F6 | Ctrl+F6 | Ctrl+F6 | Ctrl+F6 | Ctrl+F6 |
| `record_video` | Ctrl+F7 | Ctrl+F7 | Ctrl+F7 | Ctrl+F7 | Ctrl+F7 | Ctrl+F7 | Ctrl+F7, Ctrl+Alt+F5 | Ctrl+F7 |
| `record_animation` | PrintScreen, Shift+PrintScreen, Ctrl+PrintScreen, Alt+PrintScreen, Ctrl+Alt+PrintScreen | PrintScreen, Shift+PrintScreen, Ctrl+PrintScreen, Alt+PrintScreen, Ctrl+Alt+PrintScreen | PrintScreen, Shift+PrintScreen, Ctrl+PrintScreen, Alt+PrintScreen, Ctrl+Alt+PrintScreen | PrintScreen, Shift+PrintScreen, Ctrl+PrintScreen, Alt+PrintScreen, Ctrl+Alt+PrintScreen | PrintScreen, Shift+PrintScreen, Ctrl+PrintScreen, Alt+PrintScreen, Ctrl+Alt+PrintScreen | PrintScreen, Shift+PrintScreen, Ctrl+PrintScreen, Alt+PrintScreen, Ctrl+Alt+PrintScreen | PrintScreen, Shift+PrintScreen, Ctrl+PrintScreen, Alt+PrintScreen, Ctrl+Alt+PrintScreen | PrintScreen, Shift+PrintScreen, Ctrl+PrintScreen, Alt+PrintScreen, Ctrl+Alt+PrintScreen |
| `swap_disk` | Ctrl+F4 | Ctrl+F4 | Ctrl+F4 | Ctrl+F4 | Ctrl+F4 | Ctrl+F4 | Ctrl+F4 | Ctrl+F4 |
| `copy_text` | Ctrl+Shift+C | Ctrl+Shift+C | Ctrl+Shift+C | Ctrl+Shift+C | Ctrl+Shift+C | Ctrl+Shift+C | Ctrl+Shift+C | Ctrl+Shift+C |
| `paste_text` | Ctrl+Shift+V | Ctrl+Shift+V | Ctrl+Shift+V | Ctrl+Shift+V | Ctrl+Shift+V | Ctrl+Shift+V | Ctrl+Shift+V | Ctrl+Shift+V |
| `printer_eject` | Ctrl+Shift+F5 | Ctrl+Shift+F5 | Ctrl+Shift+F5 | Ctrl+Shift+F5 | Ctrl+Shift+F5 | Ctrl+Shift+F5 | Ctrl+Shift+F5 | Ctrl+Shift+F5 |
| `degauss` | Ctrl+Shift+D | Ctrl+Shift+D | Ctrl+Shift+D | Ctrl+Shift+D | Ctrl+Shift+D | Ctrl+Shift+D | Ctrl+Shift+D | Ctrl+Shift+D |
| `recenter_view` | Ctrl+Shift+Home, Ctrl+Alt+Shift+Home | Ctrl+Shift+Home, Ctrl+Alt+Shift+Home | Ctrl+Shift+Home, Ctrl+Alt+Shift+Home | Ctrl+Shift+Home, Ctrl+Alt+Shift+Home | Ctrl+Shift+Home, Ctrl+Alt+Shift+Home | Ctrl+Shift+Home, Ctrl+Alt+Shift+Home | Ctrl+Shift+Home, Ctrl+Alt+Shift+Home | Ctrl+Shift+Home, Ctrl+Alt+Shift+Home |

Keys of these emulators that type in DOS (Space, Tab, Backspace, a letter
without Ctrl or Alt) aren't in the profiles, as a DOS program needs them:
bind them yourself in `[hotkeys]` if you want them. What else each
profile leaves out or changes:

* `pcsx2`, from PCSX2's `Pad::SetDefaultHotkeyConfig`
  (`pcsx2/SIO/Pad/Pad.cpp`). Not taken: Space (pause), Tab and Period
  (turbo), Escape (pause menu), F4 (frame limiter), Shift+R (input
  recording), Shift+F8 and Ctrl+Shift+F8 (GS dumps), F9 (software
  renderer).
* `duckstation`, from DuckStation's `Settings::SetDefaultHotkeyConfig`
  (`src/core/hotkeys.cpp`). Not taken: Space (pause), Tab (fast forward
  while held), Escape (pause menu).
* `pcsx-redux`, from the keys PCSX-Redux checks in `src/gui/gui.cc`.
  Pause pauses and resumes. It has one quick-save slot, so the slot keys
  are Rust-DOS's. Not taken: F5 and F6 (resume, pause), Backspace
  (rewind), Tab (turbo), Escape (menu), F8 and Shift+F8 (reset), F10,
  F11 and Shift+F11 (debugger steps), Ctrl+PrintScreen (screenshot to
  the clipboard).
* `epsxe`, from ePSXe's FAQ (`www.epsxe.com/files/ePSXe_FAQ.txt`): F1,
  F2 and F3. Not taken: F4 (SIO IRQ) and F6 (XA reading).
* `retroarch`, from RetroArch's keyboard defaults
  (`config.def.keybinds.h`). F1, its menu, opens the settings window, and
  so does Ctrl+F12. Not taken: Space and L (fast forward), R (rewind), P
  (pause), F (fullscreen), H (reset), K (frame advance), E (slow motion),
  Escape (quit), Scroll Lock (game focus), the keypad's + and - (volume).
  Running Rust-DOS as a libretro core, the frontend's own keys apply
  instead (see [As a libretro core](#as-a-libretro-core)).
* `dosbox`, from DOSBox 0.74's README and DOSBox Staging's key handlers
  (its [shortcuts page](https://www.dosbox-staging.org/0.83/manual/appendices/shortcuts/)
  agrees). Rust-DOS's own keys already follow DOSBox's, apart from
  Ctrl+F12, which speeds the CPU up here as in DOSBox. Ctrl+F1, DOSBox's
  keymapper, opens the settings window, and saving a state, which DOSBox
  has no key for, moves to Ctrl+Shift+F1. Not taken: Ctrl+F7 and Ctrl+F8
  of 0.74 (frame skip; here they record video and mute, as in DOSBox
  Staging), Ctrl+F9 (quit), Ctrl+Alt+Home (restart), Alt+F5 (rendered
  screenshot), Ctrl+Alt+F6, Ctrl+Alt+F7 and Ctrl+Alt+F8 (MIDI and OPL
  recording), Ctrl+F2 (reload shader).
* `86box`, from 86Box 6.0's `def_acc_keys` (`src/86box.c`). Its
  screenshot key is Rust-DOS's Ctrl+F11, so the CPU speed has no key
  here (the settings window's Emulator page has it). Not taken:
  Ctrl+Alt+F (fast forward, a switch in 86Box, held in Rust-DOS),
  Ctrl+F12 (Ctrl+Alt+Del to the machine; it opens the settings window
  here), Ctrl+F10 (Ctrl+Alt+Esc), Ctrl+Alt+PageDown (the menus in
  fullscreen), Ctrl+Alt+F12 (reset), Ctrl+Alt+I.

DOSBox-X has no profile: its hotkeys are held with its host key (F11 on
Windows, F12 elsewhere), which isn't a modifier Rust-DOS can bind.

## Save states

A save state is the whole machine at one moment: the processor, memory,
screen, sound cards and music, DOS's open files, and where a batch file had
got to. **Ctrl+F1** saves to the current slot, **Ctrl+F2** loads it, and
**Ctrl+F3** and **Ctrl+Shift+F3** pick one of the nine slots; the settings
window's **States** page (Ctrl+F9) shows when each was saved, and a picture.
A [game profile](CONFIGURATION.md#game-profiles) has slots of its own, in
`states/<game>` beside the configuration file; otherwise they are in
`states/dos`.

* Loading a state puts its hardware settings back first, the memory size
  (`memsize`) and network card among them. (The libretro core loads only
  states with its own memory size.) The speed (`cycles`) isn't one of
  them: the machine goes on at the speed it runs at, except in
  deterministic mode.
* A slot is the file `slot1.state` (to `slot9.state`): the machine,
  deflated, with a picture of the screen. Its size follows the memory in
  use, from about a MB for a DOS game to 10 to 30 MB for Windows with
  128 MB.
* Files on the host and in disk images aren't part of a state: a drive
  mounted from a folder is recorded by its path, and what a program wrote
  since stays written, as in DOSBox. So a state of a game stays small
  however big the game is, and loading it after you change the game's
  files gives the game the changed files. A file the game had open is
  opened again at the place it had got to: one changed since reads as it
  is now, and one deleted since stays closed.
* A system [booted from a disk
  image](CONFIGURATION.md#booting-a-disk-image) is the exception: its
  disks go back with its state. A hard disk image's changes are kept
  beside the slot (`slot1.C.rdelta`), a few MB to a few hundred, and
  load only over the same image: after a `COMMIT`, or after the image
  is replaced, the slots of the system from before no longer load. A
  floppy, or a disk mounted with `-inplace`, is copied whole
  (`slot1.A.img`, `slot1.C.img`). Copy or delete a slot's `.rdelta` and
  `.img` files along with its `.state`.
* The FM chip and MIDI synthesizers are told their registers and
  instruments again, so notes that were playing start again.

## Running in a browser

Rust-DOS runs in a web browser as a WebAssembly module and a page that
runs it. Releases include the whole site as
`rust-dos-<version>-web.zip`, for any web server that serves `.wasm` as
`application/wasm`; [docs/building.md](docs/building.md#the-browser-version)
says how to build it.

* C: is a 250 MB hard disk image kept in the browser's storage. Drop files,
  folders or `.zip` archives on the page (or use *Add files* and *Add
  folder*) to copy them to C:, and floppy, CD or hard disk images to insert
  them. *Drives* inserts and ejects images, and downloads or erases C:.
* *Settings* opens the [settings window](CONFIGURATION.md#settings-window),
  and *Config file* edits `rust-dos.conf` as text; the browser keeps both.
  There are no host directories to mount.
* Save states, screenshots (*Screenshot*) and video recordings (*Record*,
  WebM or MP4) work as in the program; rewind doesn't.
* Gamepads work as joysticks once a button is pressed on them. On phones
  and tablets, *Touch* shows a D-pad and buttons, a finger moves the mouse
  like a trackpad, and *Keyboard* shows a PC keyboard.
* Clicking the screen while a program uses the mouse captures it; Ctrl+F10
  or Esc releases it. Sound starts with the first key press or click.

The page takes parameters: `?zip=URL` copies an archive to C: at startup,
`?run=COMMAND` types a command at the first prompt (both can repeat), and
`?game=NAME` launches a game profile, for example
`?zip=games/keen.zip&run=cd%20keen&run=keen1`. `?persist=0` keeps C: in
memory only, `?log` sends the log to the browser console, and
`?renderer=2d` draws without WebGL 2 (and so without the CRT shaders).
`?section.key=value` sets any [`rust-dos.conf`](CONFIGURATION.md) setting
for that visit only, for example `?emulator.cycles=3000&sound.sbtype=sb2`:
the browser's saved settings stay as they are, and a game profile's own
settings still win over them (`[autoexec]` and `[drives]` can't be set
this way).

## As a libretro core

Rust-DOS also comes as a [libretro](https://www.libretro.com/) core,
for RetroArch and the other libretro frontends. Releases include it as
`rust-dos-<version>-libretro-<system>.zip`, whose core a frontend loads
with a game:

```sh
retroarch -L rust_dos_libretro.so ~/games/keen.zip
```

It takes a game's folder, a program, a zip or 7z archive, a
[game profile](CONFIGURATION.md#game-profiles) or DOSBox configuration, and
floppy, hard disk and CD images (an `.m3u` list of them for the frontend's
disk control), or starts at the prompt without content. The core options set
the machine, `system/rust-dos/rust-dos.conf` the rest, and the settings
window (Ctrl+F12) is there too. Save states, rewind and
RetroAchievements are the frontend's.
**[libretro/README.md](libretro/README.md)** has the details, and
[docs/building.md](docs/building.md#the-libretro-core) says how to build
the core.

## Debug & remote-control server

`--debug-server` starts a local HTTP/WebSocket interface on
`127.0.0.1:8086` (or `--debug-server ADDR`) for scripted and LLM-driven
debugging. It has no authentication, so keep it on localhost. With port 0
(`--debug-server 127.0.0.1:0`) the system picks a free port, and the
`Debug server listening on` line on stdout gives the address.

<img width="1349" height="1017" alt="image" src="https://github.com/user-attachments/assets/3d01c401-3e3a-4ef3-a2df-ddb280b983a1" />

Open the address in a browser for the debugger: the screen with remote
input, pause, step and run to an address, the disassembly with breakpoints,
the registers, memory, stack and watchpoints, and the log, the instruction
trace, a heatmap of the memory the machine writes (`/ui#heat`, where code
shows too) and the rest of the machine's state. For scripts, `curl
localhost:8086/` lists every endpoint:

```sh
curl 'localhost:8086/api/screen/text?format=text'
curl -XPOST -H content-type:application/json -d '{"text":"dir\n"}' localhost:8086/api/input/type
curl -o screen.png localhost:8086/api/screenshot
curl 'localhost:8086/api/memory?addr=DS:SI&len=64'
```

Set `SDL_VIDEO_DRIVER=dummy` to run without a window, which also needs no
sound device: the sound goes to the null output
([`audio_output`](CONFIGURATION.md#mixer)). Don't set
`SDL_AUDIO_DRIVER=dummy`: SDL's dummy driver doesn't keep real time, at a
rate that changes from run to run, and makes any measurement wrong. SDL3
still reads SDL2's names, `SDL_VIDEODRIVER` and `SDL_AUDIODRIVER`, when
the new ones are unset. [docs/llm-debugging.md](docs/llm-debugging.md) is
a guide to using the server, and says what to use for timing runs.

## Profiling

`POST /api/profile {"enabled":true}` on the debug server, or `--profile
FILE` from startup to exit, samples where the emulator's time goes (the
interpreter, translated code, each device's ports, drawing, sound) and which
guest code it runs, and counts port accesses, BIOS and DOS calls and the
recompiler's work. The report comes as JSON, a table, or folded stacks for
flame graphs. Without a profile running it costs nothing.
[docs/profiling.md](docs/profiling.md) describes it.

## Log file

Rust-DOS logs what it does to `rust-dos.log` in the
[per-user configuration directory](CONFIGURATION.md#configuration-file):
the programs it runs, DOS and BIOS functions and I/O ports it doesn't
emulate, failed file operations, CPU exceptions, configuration warnings
and the calls of the built-in Windows' functions (`win32_trace`). Attach
it to bug reports. It is replaced on every start and holds at most
64 MB: its first 16 MB stay, and once it is full, the lines after them
are cut down to the latest 24 MB, behind a `[LOG]` line that says how
much was left out, so it keeps the start of the session and its latest
lines. `--log FILE` writes it to `FILE` instead, so instances running
at the same time keep separate logs. Rust-DOS stops with an error if it
cannot create `FILE`.

If Rust-DOS crashes, the log's last line starts with `[CRASH]` and says
why: the error and where it happened, such as the graphics driver's
file. A run in a window also shows the error in a dialog, with the text
to report and the link to report it at; on Windows, Ctrl+C copies the
dialog's text. Runs with `--hidden` or `--debug-server` only log it.
