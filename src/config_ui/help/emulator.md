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

# CPU core {#core}

What runs the games' code:

- **auto**: the interpreter for plain DOS games, the fast dynamic
  recompiler once a game switches to protected mode (DOS/4GW games such
  as Doom or Descent). Best for everything.
- **dynamic**: always the recompiler.
- **normal**: always the interpreter.

Both run games the same way, so there is no need to change this when a
game misbehaves.

# Processor {#cpu}

The processor games find: **386**, **486**, **Pentium** or **Pentium
MMX**.

**486** suits almost everything. Pick **Pentium** for late-1990s games
that ask for one, and for 3dfx games; **Pentium MMX** for those that
need MMX. Takes effect at the DOS prompt.

# Video card {#machine}

The display card games find, which decides the graphics they choose:

- **Super VGA (VESA)**: a VGA with high-resolution modes. Best for most
  games.
- **S3 Trio64**: for Windows 95 or 3.1 booted from a disk
  image.
- **S3 ViRGE (3D)**, **S3 ViRGE/VX (3D)**: for Direct3D games in
  Windows 95, with S3's driver installed there.
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

# Memory {#memsize}

The PC's RAM. **16 MB** runs nearly every DOS game. Windows 95 and a few
late games want **32** or **64 MB**.

How much there can be depends on the processor, as with the motherboards
of its day: up to **64 MB** with a 386, **128 MB** with a 486, **256 MB**
with a Pentium and **512 MB** with a Pentium MMX. Picking a smaller
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
