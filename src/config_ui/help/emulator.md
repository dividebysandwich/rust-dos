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

# PowerVR PCX2 {#powervr}

Adds a PowerVR PCX2 3D card, the chip of the Matrox m3D and VideoLogic
Apocalypse 3Dx, for games with a PowerVR version, such as Tomb Raider's
PowerVR patch (`3DPATCH\PWRVR\TOMBPCX2.EXE` on the Tomb Raider Gold
CD). The card has no picture of its own: it draws into the VGA card's
memory, so it works with any *Display adapter* with a VESA BIOS.

Set *Processor* to **Pentium** and *Memory* to **32 MB**: Tomb Raider's
PowerVR version wants 20 MB free. Takes effect at the DOS prompt.

# PowerVR filtering {#powervr-filter}

How the PowerVR card smooths textures:

- **As the game sets it**: the card's own bilinear filtering, which most
  games turn on.
- **Point**: blocky texels, as with filtering off.
- **Bilinear**: smoothed, even where the game turns filtering off.

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
