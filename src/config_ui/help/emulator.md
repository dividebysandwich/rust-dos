# CPU speed (cycles) {#cycles}

How fast the emulated processor runs, in instructions per millisecond.

- **auto**: the default. This setting has the emulator watch the frames 
  a game draws and gives it as much speed as needed for the target 
  framerate. This is either determined by the game itself (many use 
  20, 30 or similar hardcoded fps values), the monitor, or user input.
  Games that don't draw full frames (mostly early games from the 1980s)
  run at 3000 cycles. Such games can be forced to a higher cycle value 
  by typing **auto 10000**.
  Games for DOS extenders (DOS/4GW) get all the speed there is. 
  Finding the speed takes a few seconds when a game starts.
- **max**: as fast as your computer allows, for every game.
- A number: a fixed speed. **3000** is about a 286, **10000** a fast
  386, **50000** a fast 486.

If a game runs far too fast (a 1980s game where enemies zip across the
screen, or music plays at double speed), try **1000** to **5000**. If a
game runs too fast or too slow at auto, give it a number.

> **Ctrl+F11** and **Ctrl+Shift+F11** slow down and speed up the CPU
> while a game runs, from the speed it has. At auto, they change to a
> fixed speed.

# Auto speed target {#auto-fps}

The frame rate auto speed aims for, at most. **auto**, the default,
aims for the display's refresh rate (70 for most VGA games): frames
faster than that are never seen.

A number gives games only enough speed for that many frames a second.
A lower one, such as **30**, saves your computer's power where a game
would draw 70 frames a second if it could, or stops one that plays too
fast at high frame rates. A higher one, such as **120**, lets a game
that draws frames it doesn't show go faster. Games that cap their own
frame rate below it stay where they are.

Shown only while the CPU speed is auto.

# Auto: busy Windows at full {#auto-busy-os}

Windows 95 and 98 halt the processor, or wait in a loop the emulator
skips, while nothing runs. Once auto speed has seen a protected-mode
program idle like that, one that keeps it from idling for a whole
measurement, such as a 3D game building its scene, gets all the speed
your computer has. **off** treats it as any other program that draws
no frames, which auto speed slows down to a spinning menu's speed.

Shown only while the CPU speed is auto.

# CPU core {#core}

What runs the games' code:

- **auto**: the interpreter for plain DOS games, the fast dynamic
  recompiler once a game switches to protected mode (DOS/4GW games such
  as Doom or Descent). Best for everything.
- **dynamic**: always the recompiler.
- **normal**: always the interpreter.

Both run games the same way, so there is no need to change this when a
game misbehaves.

# FPU arithmetic {#fpu}

How the floating-point unit (the 387 maths coprocessor) adds and
subtracts:

- **exact**: on 80 bits, as a real 387 does. Every game computes exactly
  what it would on a real PC. The default.
- **fast**: on the host's 64-bit doubles, as multiplications and
  divisions always are. Games that compute a lot with the FPU (Quake,
  flight simulators, 3D benchmarks) need noticeably less host CPU, which
  helps on slow hosts such as a Raspberry Pi.

Fast results can differ from a real machine's in their last digits, and
always round to nearest. Most games never notice. Pick **exact** if a
game draws or behaves oddly in fast mode, or for benchmarks that check
the FPU's precision.

Takes effect at once, on both CPU cores. Saved states load in either
mode.

# Processor {#cpu}

The processor games find: **386**, **486**, **Pentium**, **Pentium
MMX**, **K6-2**, **Pentium Pro**, **Pentium II** or **Pentium III**.

**486** suits almost everything. Pick **Pentium** for late-1990s games
that ask for one, and for 3dfx games; **Pentium MMX** for those that
need MMX; **Pentium II** for programs built for a Pentium Pro or later,
which use its conditional moves (CMOV); **Pentium III** for those that
need SSE. **K6-2** is AMD's Pentium MMX with 3DNow!, for the games with
code of their own for it. **Pentium Pro** is the Pentium II without MMX.
Takes effect at the DOS prompt.

# BIOS POST screen {#post}

A BIOS's power-on self test before DOS starts, and at each reboot: the
memory counted, the CPU, the drives and the sound cards. **quick** shows
it for a moment, **full** counts the memory as it is tested. **Del** while
it shows opens these settings as a BIOS setup, where **F10** saves them and
restarts the machine; **Esc** skips the memory test.

# Video card {#machine}

The display card games find, which decides the graphics they choose:

- **Super VGA (VESA)**: a VGA with high-resolution modes. Best for most
  games.
- **S3 Trio64**: for Windows 95 or 3.1 booted from a disk
  image.
- **S3 ViRGE (3D)**, **S3 ViRGE/VX (3D)**: for Direct3D games in
  Windows 95, with S3's driver installed there.
- **Rendition Vérité (3D)**: for DOS games' Vérité versions, such as
  Tomb Raider's (`3DPATCH\RENDVRT` on the Tomb Raider Gold CD).
- **VGA**, **EGA**, **CGA**, **Hercules**: older cards, for games that
  misbehave on newer ones, or to see a game's EGA or CGA graphics.
- **Tandy 1000**, **IBM PCjr**: the home computers some 1980s Sierra games have
  better graphics and sound for.

Takes effect at the DOS prompt.

# 3dfx Voodoo Graphics {#voodoo}

Adds a 3dfx Voodoo 3D card, for games with a 3dfx or Glide version, such
as Tomb Raider's 3dfx patch. Games without such a version don't use it.

Set *Processor* to **Pentium** too: 3dfx games expect one. Takes effect
at the DOS prompt.

# 3dfx memory {#voodoo-memory}

The Voodoo's memory: **12 MB** with two texture units (best) or **4 MB**
with one, as on the cheapest boards. Leave it at 12 MB unless a game
has trouble with it.

# 3dfx drawn by {#voodoo-renderer}

What draws the 3dfx picture in the window:

- **Rust-DOS**: the card's own resolution, exactly as the card drew it.
- **OpenGL**: drawn again by your graphics card at a higher resolution
  (*3dfx OpenGL size*), with sharper edges and textures.

# 3dfx OpenGL size {#voodoo-scale}

How many times the Voodoo's resolution OpenGL draws at: **2x** turns
640x480 into 1280x960. Higher is sharper and takes more of your
graphics card.

# 3dfx OpenGL size in the CRT look {#voodoo-scale-shader}

With *3dfx OpenGL size* above 1x, **On** keeps the CRT look's scanlines,
mask and glow sized for the card's own lines, as at 1x. **Off** gives a
scanline to every pixel of the bigger picture, so the lines are finer.
Only the CRT looks use it, and it isn't shown at 1x.

# 3dfx antialiasing {#voodoo-msaa}

Smooths the jagged edges of the 3dfx picture drawn with OpenGL by
multisampling (MSAA): **2x**, **4x** or **8x** samples a pixel, more
being smoother and taking more of your graphics card. It adds to *3dfx
OpenGL size*. Antialiasing forced in your graphics driver's control panel
doesn't reach these pictures; this setting does.

# 3dfx dithering {#voodoo-dither}

The Voodoo stores 16-bit colour and covers the steps between its shades
with a fine pattern of dots where a game turns dithering on. **On** (the
default) gives the OpenGL picture the same pattern, so it looks like the
card's, the dots growing with *3dfx OpenGL size*. **Off** keeps 8 bits a
colour: a cleaner, smoother picture without the pattern or its banding.
**On** costs a little of your graphics card's time and none of the
CPU's. It applies to what the game draws next, so while the game is
paused the picture doesn't change. Only the **OpenGL** renderer uses it;
**Rust-DOS** always draws what the card draws.

# 3dfx anisotropic filtering {#voodoo-anisotropy}

Sharpens textures viewed at an angle with anisotropic filtering: **Off**,
**2x**, **4x**, **8x** or **16x**. It takes effect only when the OpenGL
driver supports it. With *3dfx texture sampling* set to **Unfiltered**,
also blends between mip levels to reduce shimmer, while keeping
nearest-neighbour magnification.

# 3dfx texture sampling {#voodoo-texture-sampling}

- **Default**: keeps the game's texture filtering and, with OpenGL, your
  anisotropic filtering setting.
- **Unfiltered**: nearest-neighbour sampling for sharp, blocky texels,
  without bilinear filtering. With supported OpenGL anisotropic filtering
  enabled, also blends between mip levels and allows anisotropic filtering
  to reduce distant and angled texture shimmer. This may soften textures;
  magnification remains nearest-neighbour. With anisotropy **Off**, or
  with **Rust-DOS**, sampling stays strictly nearest-neighbour.

Works with both **Rust-DOS** and **OpenGL** and takes effect immediately.
Mip levels, texture wrapping and clamping still follow the game.

# 3dfx frame rate cap {#voodoo-fps-cap}

The most frames a second a game may show on the 3dfx card. A game that
could draw faster waits for each frame's turn, so its frames come at an
even pace: smoother than slowing the whole PC down with the CPU speed.
Interrupts, sound and the game's clock go on while it waits.

Pick a rate the game reaches everywhere: a frame that comes late waits
for the next turn. **30** or **60** suit a 60 Hz display; with
*Variable refresh rate* on, the window refreshes at the cap.

# 3dfx gamma {#voodoo-gamma}

The gamma Glide gives the game's 3dfx picture, as the environment
variables **SST_RGAMMA**, **SST_GGAMMA** and **SST_BGAMMA**: **Off**
leaves it to the game. A variable you set yourself, with **SET**, is
yours and stays as you set it. Takes effect at the DOS prompt.

# Glide's DOS overlay {#voodoo-overlay}

Games made for the Voodoo Rush and later 3dfx boards, such as Tomb
Raider's Rush version (`3DPATCH\VOORUSH` on the Tomb Raider Gold CD),
don't draw with Glide of their own: they load it from **GLIDE2X.OVL**,
which came with 3dfx's drivers. The one of the Voodoo Graphics driver
drives Rust-DOS's card, whichever board the game was made for.

It is 3dfx's and doesn't come with Rust-DOS. Enter downloads 3dfx's last
Voodoo Graphics driver from the Internet Archive, after asking, and keeps
its GLIDE2X.OVL in Rust-DOS's configuration directory. It is then on
**Z:**, where games find it on the PATH. A GLIDE2X.OVL in the game's own
folder comes first. A game that looks for it while it is missing says so.

# PowerVR card {#powervr}

Adds a PowerVR 3D card for games with a PowerVR version: the **PCX1** of
the VideoLogic Apocalypse 3D, or the **PCX2** of the Matrox m3D and
VideoLogic Apocalypse 3Dx. Tomb Raider's PowerVR patch on the Tomb Raider
Gold CD has a program for each (`3DPATCH\PWRVR\TOMBPCX1.EXE` and
`TOMBPCX2.EXE`). The card has no picture of its own: it draws into the
VGA card's memory, so it works with any *Display adapter* with a VESA
BIOS.

Set *Processor* to **Pentium** and *Memory* to **32 MB**: Tomb Raider's
PowerVR version wants 20 MB free. Takes effect at the DOS prompt.

# PowerVR filtering {#powervr-filter}

How the PowerVR card smooths textures:

- **As the game sets it**: the card's own bilinear filtering, which most
  games turn on. The PCX1 has none; it blends between mip map levels.
- **Point**: blocky texels, as with filtering off.
- **Bilinear**: smoothed, even where the game turns filtering off.

# Vérité RISC {#verite-risc}

How the Rendition Vérité runs the microcode programs load into it:

- **Emulated**: its RISC processor runs the microcode instruction by
  instruction, as the card does. Every Vérité game and driver works this
  way: Speedy3D games, Windows drivers, and the 3D Blaster PCI games
  written for Creative's CGL.
- **Commands only**: the commands of Rendition's Speedy3D and Windows 2D
  microcode are carried out without running it. Faster, but only for
  Tomb Raider's and vQuake's kind of microcode and the Windows desktop.

Takes effect at the DOS prompt.

# RISC speed {#verite-mhz}

How many million instructions a second the Vérité's RISC runs: **25** as
on the V1000. More draws faster than the card could, in games that wait
for it.

# Board ROM {#verite-rom}

A dump of a Vérité board's 128 KB ROM, for its RISC. The 3D Blaster PCI
games' CGL modules start the card with a loader they copy from the ROM.
Without a dump, Rust-DOS's own loader does the same, so none is needed.
Enter picks the file, Del goes back to the built-in loader.

# Vérité IRQ {#verite-irq}

The interrupt line of the Vérité. **10** suits most games. Battle Race
needs one below 8 (**5** or **7**).

# Memory {#memsize}

The PC's RAM. **16 MB** runs nearly every DOS game. Windows 95 and a few
late games want **32** or **64 MB**.

How much there can be depends on the processor, as with the motherboards
of its day: up to **64 MB** with a 386, **128 MB** with a 486, **256 MB**
with a Pentium and **512 MB** with a Pentium MMX, K6-2, Pentium Pro,
Pentium II or Pentium III. Picking a smaller
processor brings the memory down to what it takes. Save states and rewind
grow with the memory.

Takes effect the next time Rust-DOS starts: save with **F2**, then restart.

# Expanded memory (EMS) {#ems}

EMS is the memory many games of the early 1990s ask for (they say
"EMS", "expanded memory" or "EMM386"). Leave it **on**. Turn it off only
for a game that says it can't run with EMS or a memory manager.

# Upper memory (UMB) {#umb}

Memory between 640 KB and 1 MB where drivers can go with `LH` (LOADHIGH),
leaving more of the 640 KB for games. Leave it **on**; turn it off only
if a game fails with it.

# DOS high {#dos-high}

Packs DOS's own tables at the bottom of memory, as `DOS=HIGH` in
CONFIG.SYS does, so programs get about 40 KB more of the 640 KB: `MEM`
shows about 617 KB free rather than 576 KB. Leave it **on**.

Programs then load below 64 KB. If an old game says *Packed file is
corrupt* or crashes at start, try it **off**. A change takes effect at the
DOS prompt, unless a TSR is resident.

# DPMI host {#dpmi}

Lets protected-mode games (DOS/4GW, DOS/32A, DJGPP and Borland games, such
as Jazz Jackrabbit) get their memory the modern way. Leave it **on**.

If a protected-mode game crashes at start, turning it **off** is worth a
try: those games then run as on plain DOS.

# Reported DOS version {#dos-version}

The DOS version games are told. **5.00** suits nearly everything. A
program that complains about the DOS version may want **6.22**. Disk
tools for Windows 95's disks want its DOS: **7.00** as in the first
Windows 95, **7.10** as in the later ones with FAT32.

# IDE hard disks {#ide-hard-disks}

Only matters for a system booted from a disk image with `BOOT`, such as
Windows 95: **on** lets it reach its hard disks with its own fast
drivers. Leave it on.

# CD-ROM drive (BOOT) {#boot-cdrom}

Only matters for a system booted from a disk image with `BOOT`.
**always** gives it a CD-ROM drive even when no CD is mounted, so that a
CD image or a folder of yours can go in while it runs: mount it on a CD
drive letter in the *Drives* page. **with a CD** gives it one only when a
CD is mounted as it boots, as older rust-dos did; a system installed
without a CD-ROM drive then finds no new hardware.

# Floppy controller (BOOT) {#floppy-controller}

Only matters for a system booted from a disk image with `BOOT`, such as
Windows 95: **on** gives it a floppy disk controller, so that its own
floppy driver reads and writes the disks in A: and B:. Leave it on.

# Hard disk speed {#hard-disk-speed}

Slows the hard disk down to the speed of the time. **maximum** is as fast
as your computer, and loads games quickly.

A slower disk makes loading as long as it was, which a few games need to
play their music or animations in step, and it sounds more real with the
*Hard disk noise*.

# Floppy disk speed {#floppy-disk-speed}

Slows floppy disks down to the speed of real floppy drives. **maximum**
loads instantly. Slower speeds are for the feel of it, with the
*Floppy disk noise*.

# Joystick {#joystick}

What is plugged into the game port:

- **auto**: your game controllers (Xbox or PlayStation style). With
  none plugged in, the mouse acts as the joystick.
- **one controller, 4 axes**: both sticks, for flight simulators.
- **two controllers**: one joystick each, for two players.
- **the mouse**: the mouse is the joystick.
- **none**: no game port, for games that go wrong when they find one.

Most games need calibrating in their setup: leave the stick centred when
asked.

# Deadzone {#deadzone}

How far a controller's stick has to move before it counts. Raise it if
the game drifts in one direction while you don't touch the stick.

# Keyboard layout {#keyboard-layout}

The keyboard layout DOS types in. **auto** takes your computer's layout.
Pick one if the characters come out wrong. `KEYB` at the prompt changes
it too.

# Mouse sensitivity {#mouse-sensitivity}

Multiplies captured mouse movement on both axes. **1.0** (the default)
keeps the original speed; lower values slow it down and higher values
speed it up. The range is **0.1 to 10.0**. Left and Right step by
**0.1**. Enter lets you type a value; Delete restores **1.0**. Uncaptured
pointer positioning is unchanged.

# Mouse auto capture {#mouse-autocapture}

When **on**, the mouse cursor is captured by the DOS window as soon as 
it moves over the window, provided the current DOS program uses the mouse.
The capture is released the moment the DOS cursor travels past the edge
of the screen.
Games that steer with the mouse retain it until **Ctrl+Alt** is pressed.

When **off**, the mouse is captured by clicking in the window, or by
pressing **Ctrl+Alt** or **Ctrl+F10**.

# Mouse capture messages {#mouse-capture-messages}

When **on**, each mouse capture/release action will show a notification
message on the top of the screen. While helpful for new users, turning it
**off** can help reduce notification noise.

# Hold right button for settings {#mouse-menu-hold}

When **on** (the default), holding the right mouse button for **1.5
seconds** without moving the mouse opens this settings window, for
setups with no keyboard or no F12 key, such as a TV with a remote. The
program still sees the press, and a release as the window opens. Moving
the mouse while holding the button keeps it an ordinary press. Turn it
**off** for a game that has you hold the right button still.

# Game fixes {#game-fixes}

When **on** (the default), Rust-DOS fixes a few games that go wrong on
real PCs too, or that can't work without their original disks, once it
recognises them: the Sound Blaster music and effects of Prehistorik 2
and Blues Brothers: Jukebox Adventure, which click at every note and sit
far below the centre line, and the disk checks of MicroProse's games.
Other programs don't change. **off** runs them all as a real PC would. To
leave out one fix, name it in `game_fixes_off` in the configuration
file; CONFIGURATION.md lists them.

# Pause in settings window {#pause-in-settings}

When **on** (the default), the program pauses while this settings
window is open, except on the Mixer and Stats pages. Turn it **off** to
let the program run on behind the window. In a LAN room it always runs
on, whatever this says, as the other players' games would wait for it.

# Rewind (Alt+F11) {#rewind}

When **on**, Rust-DOS keeps the last minutes of play: hold **Alt+F11** to
go back in time, for instance right before you fell into that pit.

# Rewind memory {#rewind-memory}

How much memory rewind may take. More memory rewinds further back. The
default **256 MB** is plenty for most games.

# Capture folder {#capture-dir}

Where screenshots (**Ctrl+F5**), sound recordings (**Ctrl+F6**) and
videos (**Ctrl+F7**) go. Enter types a folder of your own.

# Capture window & overlay {#record-ui}

Whether screenshots and videos show this settings window and the
performance overlay when they are open. **off** captures the game alone.

# Capture CRT shader {#record-shader}

Whether screenshots and videos show the picture through the CRT shader,
as it looks in the window. **off** captures the plain picture.

# Edit the [autoexec] commands {#autoexec}

Commands that run each time Rust-DOS starts, before you get the prompt,
as in a DOS `AUTOEXEC.BAT`. For example, to mount a CD and start a game:

```
MOUNT D ~/dos/cds/game.cue
C:
CD GAME
GAME.EXE
```

**F2** saves them, **Esc** leaves them as they were. They run at the next
start. While a game profile plays, this edits that game's commands.

# Shell suggestions {#shell-suggestions}

When **on**, the DOS prompt suggests the rest of the line you are typing
in dark grey, from the newest matching line in the history or else from the
first name **Tab** would complete. **Right** or **End** takes the
suggestion, **Ctrl+Right** takes its next word, and **Enter** runs only
what you typed.

# Shell colors {#shell-colors}

When **on**, the prompt and the line typed at it are colored: built-in
commands, programs, unknown commands, switches and arguments each have
their own color. The colors are set in the `[shell]` section of the
configuration file (`prompt_color`, `command_color` and so on).
**off** keeps the screen's own color.

# Save shell history {#save-shell-history}

When **on**, the lines typed at the DOS prompt are kept in
`shell_history.txt` in the Rust-DOS settings folder, so **Up**, **Ctrl+R**
and **F7** find them again after a restart. When **off**, the history lasts
only until Rust-DOS quits. A line that begins with a space is never kept.
The `HISTORY` command lists the lines, and `HISTORY CLEAR` forgets them.

# Check for new versions {#version-check}

When **on**, Rust-DOS asks rust-dos.com at startup whether a newer version
is out. If there is one, a notification says so over the picture and a line
under the box above the first DOS prompt names it. Only the version number
is fetched; nothing about you or your games is sent. **Off** skips the
check. The change takes effect at the next start.

# Auto-apply Almanac settings {#almanac-auto}

When **on**, a program started from the DOS prompt (typed, or a batch
file's line) waits while Rust-DOS asks almanac.rust-dos.com for the
settings players found best for it. The almanac knows the program by
its file's name, size and checksums, the only things sent. Settings a
moderator blessed win over those most reports chose. The program then
starts with them, and once it has ended your own settings come back.
Programs the almanac doesn't know start with your settings. The wait
lasts ten seconds at most, and **Esc** ends it at once. A game launched
from its profile on the Games page keeps the profile's settings.
**Off** (the default) never asks.

Only the emulated machine's settings come from the almanac: the CPU,
memory, sound cards and the like, never drives, commands or paths.
Some, such as the memory size, take effect the next time Rust-DOS
starts.

# The Almanac {#almanac}

almanac.rust-dos.com collects players' reports on how games run in
Rust-DOS and with which settings.

*Fetch settings from Almanac* lists the games the almanac has for the
program started last (a √ marks those whose version has this very
file) and those named like it. Type another name and press **Enter** to
search again. **Enter** on a game puts its settings in place: the ones a
moderator blessed, or else the ones most reports chose. **F2** saves
them, into the game's profile while one plays.

*Report settings to Almanac* sends the settings as they are now, with
how the game played: pick the rating with the arrow keys, then press
**Enter** on the game. A game the almanac doesn't have yet is added by
the name typed. The program's name and checksums go with the report, so
others with the same version find it. Report as often as you like, with
other settings or after a new version of Rust-DOS.
