# ToadForce: a synthetic 3D card for games of 2000 to 2002

Status: proposal. Nothing here is implemented. The document covers the
research, the card's definition, how the emulator would draw it, what
the guest software needs, and the order of the work. Facts taken from
other projects have their source in "References"; what could not be
confirmed is listed under "Unverified facts".

## The problem

The newest 3D card rust-dos emulates is the Voodoo3. Its limits are the
real card's: 16-bit colour and depth, 256x256 textures, two texture
units, no compressed textures, no hardware transform and lighting, and a
short list of texture stage operations. Games from 2000 on were written
for the GeForce 256, GeForce2, Radeon and their successors, and many
either require what those cards added or take untested paths without it.

Cases already seen:

- Zanzarah reads DDS textures only when `EnumTextureFormats` lists a
  DXTn format. Without one it reads BMP files through code with two bugs
  of its own (#583, #606). The game patches written for them (#593,
  #613) were closed, because they work around a card below the game's
  minimum instead of emulating one that meets it.
- Grom asserts when no listed texture format has 8 bits a channel
  (#645).
- Giants: Citizen Kabuto draws its terrain tiles wrong without bump
  mapping.
- #615 asks for a card with DXTn and collects these games.

Emulating a real card of that class at register level is not practical
today. 86Box has no merged NV3, TNT or GeForce device. envytools' prose
for the NV10 and NV20 3D engines is a stub of a few hundred bytes each;
what exists is `rnndb`'s method names and bit fields, which name the
registers and say nothing of the pipeline's arithmetic. xemu's NV2A is
the closest working model of a GeForce-class pipeline, and it is driven
by the Xbox's own software, with no Windows driver in the picture. A
register-level GeForce would also have to run NVIDIA's Windows 98
driver, which uses every corner of the chip.

## The proposal in short

ToadForce is a PCI display adapter that never existed. rust-dos defines
it: its PCI identity, registers, memory layout, command set,
capabilities, and the arithmetic of its pixel pipeline. Its feature set
is that of the cards the target games were written for, taken as a
union: hardware transform and lighting, four textures at once, DXT1 to
DXT5, 32-bit colour, 24-bit depth with stencil, cube maps, and every
Direct3D 7 and 8 texture stage operation, including the three bump
mapping ones.

Five points define the design:

1. The command set carries the fixed-function state of Direct3D 7 and 8
   (render states, stage states, transforms, lights, materials) and
   draws untransformed vertices. A driver passes the API's state through
   and the card does the work, which is what VMware's SVGA3D device does
   for Direct3D 9.
2. All of the card's contents are in its memory, at offsets the guest
   chooses: frame buffers, depth buffers, textures (DXT ones stored
   compressed), and vertex buffers. The host's GPU objects are caches of
   it. This is the model the Voodoo3 emulation already uses, and it
   makes save states, `Lock` pointers and the software renderer work the
   same way as today.
3. Games are played on the host's GPU. The OpenGL renderer draws the
   commands with shaders generated from the pipeline state, as wined3d,
   DXVK and xemu do, and the card is shaped so that the GPU does the
   transform, the lighting and the pixels with nothing read back in a
   frame. A software renderer defines what the card draws, in integer
   arithmetic where the pixel pipeline allows it, and the OpenGL
   renderer is tested against it within stated tolerances. It also
   draws where there is no GPU to use, slowly.
4. The capabilities are a fixed table for each feature level. A level
   never changes once released. New features make a new level, which the
   guest's driver has to ask for.
5. The built-in Windows drives the card first. A driver for a booted
   Windows 98 comes last, and the command set is shaped so that this
   driver is a thin translation of Direct3D's driver interface.

## Target games

These eight decide what level 1 contains. The table says what each is
expected to need beyond a Voodoo3, and where that comes from. "Traced"
means seen in a run of the game (see "Phase 0 traces").

| Game | Year | API | Needs | Source |
|---|---|---|---|---|
| Giants: Citizen Kabuto | 2000 | Direct3D 7 | Dot3 bump mapping and environment maps, which its options screen shows as unavailable on the Voodoo3; reviewers say it enables Dot3 only on a T&L device | traced (options), reviews (Dot3) |
| The Operative: No One Lives Forever | 2000 | Direct3D 7 | DXT1, DXT3 and DXT5 textures | traced |
| MDK2 | 2000 | OpenGL | T&L (a check box in its launcher, off by default): with it on, OpenGL lighting with 8 lights and materials; 32-bit textures | traced |
| Black & White | 2001 | Direct3D 7 | T&L, large textures | PCGamingWiki snippet; no trace, #889 |
| Oni | 2001 | OpenGL | `ARB_multitexture`, `ARB_texture_compression`, `EXT_blend_color` (it looks all three up) | traced |
| Max Payne | 2001 | Direct3D 8 | 32-bit textures and DXT1, DXT3, DXT5 (it asks for them and is refused); D24S8 | traced |
| Zanzarah | 2002 | Direct3D 7 | DXTn in the texture format list; minimum GeForce or Voodoo 5500 | #583, #606 |
| Grom: Terror in Tibet | 2002 | Direct3D | A8R8G8B8 textures; minimum a 32 MB card | #645 |

## Phase 0 traces

On 2026-11-10 each game was run on the built-in Windows with a Voodoo3,
with a trace of what it asks of DirectDraw, Direct3D and OpenGL
(`RUST_DOS_3D_TRACE`, on the local branch `toadforce-phase0-trace`,
not merged). The Voodoo3 HAL reports a W buffer today, so each
Direct3D game was run twice, with that capability shown and hidden.

The traces found gaps in the built-in Windows that stopped or spoiled
some of the games on any card. Seven were fixed the same day, and the
text below says where a result predates its fix: `ReBaseImage` (#869),
`_EH_prolog` (#870), blits from DXT surfaces (#871), vertex blending in
Direct3D 8 (#872), a Z buffer on a texture target (#877), the C++
run-time type information of MSVCRT (#879) and OpenGL's lighting
(#890).

What the runs reached:

| Game | Reached | Stopped by |
|---|---|---|
| Giants | gameplay, mission 1 | |
| No One Lives Forever | gameplay, started from `LITHTECH.EXE` | `NOLF.EXE` is SafeDisc 2 (#889) |
| Max Payne | gameplay, the tutorial, from the release's no-CD `MaxPayne.exe`, traced with its own MSVCRT before #879 was fixed | the original `MaxPayne.exe` is SafeDisc 2 (#889) |
| Oni | gameplay, the training level | |
| Zanzarah | the 3D title scene; gameplay in the Fairy Garden with the closed fixes of #593 and #613 applied locally | #583 |
| Grom | initialisation | #645 |
| MDK2 | gameplay, the first level, once #869 was fixed | |
| Black & White | nothing | SafeDisc 2 doesn't run on the built-in Windows (#889) |

What they show:

- **Every game creates the plain HAL device**, the only one offered.
  None could ask for the T&L HAL, so what the games do with hardware
  T&L, and with the operations they gate on it, stays untraced until the
  card exists. Giants' options screen lists object bump mapping,
  landscape bump mapping and environment mapping as unavailable.
- **The W buffer.** Giants sets `ZENABLE` to `USEW` when the capability
  is shown and to plain Z when it is hidden, and reaches gameplay both
  ways. No other game set `USEW`. Whether Giants' Z path shows
  z-fighting was not looked at; the run had 16-bit depth, and the card
  gives it 24.
- **Zanzarah in gameplay** was traced on a local build with the two
  closed game fixes, which put it on the path it takes when no DXTn
  format is listed (BMP textures). What it does with DDS textures is
  untraced. In the Fairy Garden it uses one stage, one directional
  light, linear vertex fog, source colour keys, a depth bias, and
  render targets in 128 and 256 texel textures, and never a W buffer.
- **Stages.** Giants, Zanzarah and No One Lives Forever draw with stage
  0 alone. Max Payne uses two (a light map added to the diffuse colour,
  then the texture modulated). The operations seen are `SELECTARG1`,
  `SELECTARG2`, `MODULATE`, `ADD` and `BLENDFACTORALPHA`. No One Lives
  Forever asks `ValidateDevice` about `ADD` and `ADDSIGNED` in stage 1.
  No bump mapping state was set to anything but 0.
- **Textures.** On the Voodoo3 all of them create 16-bit textures of at
  most 256 texels. Max Payne first asks for A8R8G8B8, X8R8G8B8, DXT1,
  DXT3, DXT5 and the bump formats, and for D24S8, and is refused. No One
  Lives Forever keeps DXT surfaces in system memory and blits them into
  16-bit textures, which the built-in DirectDraw couldn't do at the
  time (#871, fixed). Giants and Zanzarah render into 64 to 256 texel
  textures. Zanzarah's are its heroine's shadow, and it shares the back
  buffer's Z buffer with them, which the built-in DirectDraw refused
  (#877, fixed: Windows 98 lets a texture in video memory take a Z
  buffer of any size). Giants draws into its targets without depth.
  Giants runs out of video memory creating 256x256 textures.
- **Lights and fog.** Giants uses up to 3 directional lights, Max Payne
  up to 7 point and spot lights at once. Max Payne and Zanzarah use
  linear vertex fog. No game set a user clip plane.
- **Vertex blending.** Max Payne draws its characters with indexed
  vertex blending (28 world matrices, indices as `UBYTE4`, one weight)
  on a device it created with software vertex processing. The built-in
  Direct3D 8 refused those vertex formats at the time (#872, fixed).
  It doesn't tween.
- **OpenGL.** Oni looks up every OpenGL 1.1 function, the
  `ARB_multitexture` functions, `glBlendColor`, the seven
  `ARB_texture_compression` functions, `wglSwapIntervalEXT` and 3dfx's
  gamma functions. On the Voodoo3 it draws with one texture unit,
  16-bit texture formats and its own transform (`glBegin` with
  four-component texture coordinates), and no OpenGL lights.
- **MDK2** asks for a 16-bit pixel format with 16-bit depth and no
  stencil, and looks up the OpenGL 1.1, WGL and GLU functions only. It
  loads RGBA8 and RGB8 textures of up to 256 texels with full mip
  chains, sets the texture environment to `MODULATE` only, and draws
  everything in immediate mode (`glBegin` with strips, quads and
  triangles), with alpha blending, an alpha test, culling and linear
  fog. It uses no vertex arrays, texture coordinate generation or clip
  planes. With "Hardware T&L" ticked in its launcher it hands lighting
  to OpenGL: lights 0 to 7 with positions, diffuse colours and
  quadratic attenuation, a diffuse material, `glColorMaterial` on the
  emission colour, and normals as shorts. The built-in OPENGL32 had no
  lighting at the time, so its characters then drew black (#890,
  fixed).

What that changes in the design:

- Level 1's table needed no change. The formats, the stage operations,
  the light count and the OpenGL extensions the games asked for are all
  in it.
- The W buffer stays out (see "Level 1"). Giants is the one game that
  takes it when offered. Look for z-fighting in Giants' terrain in
  phase 3, on 24-bit depth, and add a W buffer to a later level if it
  shows.
- Indexed vertex blending and tweening stay out of level 1. With the
  capabilities reporting no indexed blending in hardware, Direct3D 8
  does both on the processor, as it did on the cards of 2001, and the
  built-in Direct3D 8 now does (#872).
- The DXT decoder the card needs exists since #871
  (`src/video/dxt.rs`), with two sets of rules. The card decodes by
  S3TC's, as the cards of the time did. DirectDraw's software blits
  keep Windows 98's own, which differ (DXT5's interpolated alphas run
  the other way, and three-colour blocks occur in DXT2 to DXT5 too).
- A texture render target takes a depth buffer of its own on this
  card, or shares a larger one: `SET_RENDER_TARGET` names any colour
  surface and any depth surface in card memory, and DirectDraw's rule
  for attaching them is Windows' since #877.
- MDK2's lighting is what the card's OpenGL states are for: its lights
  map to the eight light slots, and its emission colour taken from the
  vertex colour to the material source states. Since #890 the built-in
  OPENGL32 computes OpenGL's lighting itself and hands the Voodoo's
  Direct3D device lit vertices. It keeps the lighting state
  (`opengl32/light.rs`) apart from the code that evaluates it
  (`opengl32/shade.rs`), so on this card the front end sends that state
  as commands and the card does the lighting.
- Black & White can't be traced on the built-in Windows. Its SafeDisc
  2.10 loader, like No One Lives Forever's and Max Payne's, looks for
  code of Windows 98's VMM that the built-in Windows doesn't have
  (#889). On a booted Windows 98 the same executable passes its disc
  check, so the game's needs wait for the Windows 98 driver of phase 6.
- Still open and unrelated to the card: InstallShield 6 setups (#875).
  Max Payne runs on the built-in MSVCRT since #879.

## Precedents

What each earlier project does, and what this design takes from it.

**VMware SVGA3D.** A paravirtual device whose commands mirror the
Direct3D 9 device interface: `SETRENDERSTATE`, `SETTEXTURESTATE`,
`SETTRANSFORM`, `SETMATERIAL`, `SETLIGHTDATA`, `SETLIGHTENABLED`,
`SETCLIPPLANE`, `SETRENDERTARGET`, `CLEAR`, `PRESENT`, `SHADER_DEFINE`,
`DRAW_PRIMITIVES` with a vertex declaration. Its capabilities are a
numbered list (`SVGA3D_DEVCAP_MAX_LIGHTS`, `MAX_TEXTURES`,
`MAX_CLIP_PLANES`, shader versions, one entry per surface format). The
headers are public under GPL-2.0 or MIT. Taken: the shape of the command
set and of the capability table. Not taken: its surfaces, which are
host objects the guest copies into; ToadForce keeps contents in card
memory (see "Memory").

**wined3d.** Derives a compact key from the fixed-function state
(`struct ffp_frag_settings`: per stage the colour and alpha operation,
three arguments each, texture type, projection, destination; plus fog,
alpha test, colour key, flat shading. `struct wined3d_ffp_vs_settings`:
light counts by type, material sources, blend count, fog mode, texgen
per stage). It looks the key up in a tree and generates a GLSL shader on
a miss. Matrices, lights and materials are uniforms and stay out of the
key. Bump environment mapping perturbs the next stage's coordinates with
a 2x2 matrix uniform. Its ddraw offers the T&L HAL device with
`D3DDEVCAPS_HWTRANSFORMANDLIGHT`, and runs `ProcessVertices` on the
processor. It has its own DXT1 to DXT5 decoder for hosts without S3TC.
Taken: the key-to-shader scheme, the split between key and uniforms, and
the list of what belongs in the keys. rust-dos's Direct3D is already
partly ported from Wine's, under the terms in
`builtin-windows-architecture.md`.

**DXVK.** Up to 2.x it generated one SPIR-V shader per fixed-function
key. Version 3.0 replaced that with a pair of general shaders that read
the state from a uniform buffer, with specialised variants compiled in
the background, to remove the pause when a new key first appears.
Dolphin made the same move in 2017. Taken: the per-key generator first,
because it is simpler to test against a reference, with the general
shader kept as the answer if compile pauses show up in games (see
"Shader cache").

**xemu's NV2A.** Hashes a `ShaderState` (fixed-function vertex state or
a vertex program, the register combiners, the texture modes including
the two bump environment modes and the dot product modes), keeps an LRU
cache of GLSL programs, and stores compiled program binaries on disk by
hash. It has OpenGL and Vulkan back ends and no software one. Taken: the
disk cache of program binaries.

**Dolphin.** Moved its combiner emulation from floating point to integer
shader arithmetic in 2014, because float results differed from the
console and between drivers. Taken: integer arithmetic for the stage
cascade, in the reference and in the generated shaders, so the two give
the same values from the same inputs.

**SoftGPU (VMDisp9x, VMHAL9x, Mesa9x).** Working, MIT-licensed Windows
9x drivers for virtual cards: a 16-bit display minidriver built with
Open Watcom on top of Microsoft's DIB engine, a VxD, and a 32-bit
DirectDraw and Direct3D HAL DLL that answers the DirectX 8 driver
interface, with hardware T&L, and draws through Mesa. It does not do
bump mapping or shaders. Taken: the reference for the Windows 98 driver
of phase 6, including how DirectDraw finds the 32-bit DLL
(`DCICOMMAND` with `DDGET32BITDRIVERNAME`) and how OPENGL32 finds an
installable client driver (the `OPENGL_GETINFO` escape and the
`OpenGLDrivers` registry key).

**qemu-3dfx.** Marshals Glide and OpenGL calls from guest wrapper DLLs
to the host's libraries. Direct3D reaches the host through wined3d
running in the guest on the OpenGL pass-through. Not taken: what the
guest sees is then the host's OpenGL driver, with its extension list and
its behaviour, which differs from machine to machine and can't be saved
or replayed.

**virgl and Venus.** Send Gallium state with TGSI shaders, or serialised
Vulkan. Both suit guests that have a Mesa driver stack; Windows 98 has
none, and games of 2000 to 2002 speak the fixed-function APIs, which a
state-level command set carries with far less guest code.

## Rules of the design

1. **The card is hardware.** Guest software reaches it through PCI
   configuration space, registers, card memory and the command ring.
   Rule 1 of the built-in Windows ("output goes through the emulated
   hardware") holds for it as for the Voodoo3.
2. **The card's own state is the whole state**: card memory, the ring,
   registers and the state commands set. A save state holds them and
   nothing from the host's GPU.
3. **The software renderer defines the pixels.** `--deterministic` runs
   it. The OpenGL renderer is an accelerator that has to agree with it
   within the tolerances this document states.
4. **Everything else the guest can read has one defined value**:
   capabilities, format lists, error codes, status bits, memory sizes.
   None of it depends on the host's GPU, driver or speed.
5. **A level is frozen.** A test holds each level's capability table,
   and a change to a released level fails it.
6. **Era limits where a game can see them.** Sizes and counts a game
   reads (texture size, simultaneous textures, memory) stay within what
   cards of 2000 to 2002 reported, because games size their data from
   them. Where a game can't see the difference (operations accepted,
   internal precision), the card does the complete thing.
7. **The host's GPU is the path games run on.** The card is designed
   for the OpenGL renderer's speed first, and the software renderer
   pays for it where the two pull apart. Three things follow:
   - Whatever the card defines, an OpenGL 3.2 GPU can do in its own
     pipeline. A definition that would make the renderer read a target
     back during a frame, or do on the processor what a shader can do,
     is changed until it doesn't (the dithering order of 16-bit targets
     is the example).
   - The work goes to the GPU whole: untransformed vertices, lights and
     stage state, with buffers and textures uploaded once and found
     again by their place in card memory.
   - A command is complete for the guest when `RING_PUT` returns, and
     the GPU need not have drawn it by then. The ownership tracking
     makes up the difference, reading pixels back only when the guest
     reads them.

   The software renderer is the definition, the test reference and the
   fallback. It is expected to be several times slower than a GPU, and
   too slow for some target games at their usual resolutions. Speed
   work on it comes after the OpenGL renderer's, and no feature of the
   card is held back or bent to make it faster.

## The card

`docs/toadforce-spec.md` defines what this chapter describes, as a
guest sees it: field by field, with reset values, error codes and the
arguments of each command. It is written a phase at a time, and it is
the one to follow where the two differ.

### PCI identity and resources

One PCI function, class 03h/00h (VGA-compatible display controller), in
the slot the other display adapters use (`S3_DEVICE` in `src/pci.rs`,
with a new `Target::ToadForce`).

The vendor ID is 1234h, which Bochs' and QEMU's VGA uses (as
1234h:1111h), and the device ID is 7F01h. The subsystem vendor ID is
1234h and the subsystem ID 5446h ("TF"). Windows matches a driver on
vendor and device together, so a driver for Bochs' VGA doesn't claim
this card. The pair isn't registered to the project, so a driver also
reads the `ID` register and uses the card only when it holds "TOAD".
Buying a vendor ID from PCI-SIG, or using a range inside another
vendor's ID, was considered and dropped: the first costs a yearly
membership, and the second takes space set aside for someone else's
devices.

| BAR | Size | Contents |
|---|---|---|
| 0 | 64 KB, memory | registers |
| 1 | 32, 64 or 128 MB, memory, prefetchable | card memory |
| 2 | 1 MB, memory | command ring |

Interrupt pin INTA#, line 11 by default as on the Banshee. The legacy
VGA ports and memory decode as on the `Svga` adapter, and the video BIOS
is the emulator's own with VBE, whose linear frame buffer is the start
of BAR 1. DOS programs and a Windows without the driver therefore see a
VESA card.

### Registers (BAR 0)

All are 32 bits, little-endian.

| Offset | Name | Access | Meaning |
|---|---|---|---|
| 000h | `ID` | R | 44414F54h ("TOAD") |
| 004h | `LEVEL_MAX` | R | highest feature level the card has |
| 008h | `LEVEL` | R/W | level in use; 0 after reset (3D off) |
| 00Ch | `STATUS` | R | bit 0 vertical blank, bit 1 flip pending, bit 2 error latched |
| 010h | `IRQ_STATUS` | R/W1C | bit 0 vertical blank, bit 1 flip done, bit 2 error |
| 014h | `IRQ_ENABLE` | R/W | same bits |
| 018h | `RING_PUT` | R/W | byte offset after the last command written |
| 01Ch | `RING_GET` | R | byte offset the card has consumed to |
| 020h | `ERROR` | R/WC | code of the first rejection; any write clears it |
| 024h | `ERROR_AT` | R | ring offset of that command, FFFFFFFFh for a register write |
| 028h | `MEMORY` | R | size of card memory in bytes |
| 02Ch | `RESET` | W | writing 1 resets registers and command state |
| 040h | `MODE_ENABLE` | R/W | 1: scan out the mode below instead of the VGA's |
| 044h | `MODE_WIDTH` | R/W | |
| 048h | `MODE_HEIGHT` | R/W | |
| 04Ch | `MODE_FORMAT` | R/W | P8, R5G6B5 or X8R8G8B8 |
| 050h | `MODE_PITCH` | R/W | bytes a row |
| 054h | `MODE_START` | R/W | offset of the visible surface; `FLIP` changes it |
| 058h | `MODE_REFRESH` | R/W | refresh rate in Hz |
| 060h | `FRAME` | R | vertical blanks since reset |
| 064h | `SCANLINE` | R | current scan line |
| 100h- | `CAPS[n]` | R | capability table of `LEVEL` (see "Feature levels") |
| 400h- | `PALETTE[n]` | R/W | the 256 colours of a P8 mode |

`STATUS`, `FRAME` and `SCANLINE` follow the emulated clock, so they read
the same in two deterministic runs. There is no busy bit: the card has
finished every command by the time the write to `RING_PUT` returns (see
"Command ring").

Writing `LEVEL` selects the table `CAPS` shows and the commands the
card accepts. A driver written for level 1 writes 1 and gets level 1's
behaviour on every later version of the emulator.

### Memory (BAR 1)

Card memory holds every surface. The guest's driver lays it out, as
DirectDraw's heap manager does on a real card:

- the primary surface and its back buffers,
- depth and stencil buffers,
- off-screen plain surfaces,
- textures, each mip level and cube face at an offset of its own, DXT
  formats stored as the 8 or 16 byte blocks the game wrote,
- vertex and index buffers,
- palettes.

The card reads a texture from memory when a draw samples it and draws
into the render target's memory. Guest reads and writes of card memory
always see current contents: a read after a draw returns the drawn
pixels, and a write to a texture takes effect at the next draw. No
command is needed to tell the card that memory changed. `Lock` in
DirectDraw returns a pointer into BAR 1, as it does on the Voodoo3.

The emulator implements this the way it does for the Voodoo3
(`src/voodoo/tracker.rs`), generalised: each range of card memory is
current either in memory or on the host's GPU, a guest read of a range
that is newer on the GPU reads it back first, and each 4 KB page has a
write counter that the texture and vertex buffer caches compare.

The memory size is `toadforce_memory`, 64 MB by default. The whole of it
is the video memory the driver reports.

### Command ring (BAR 2)

The guest writes commands into BAR 2 and then writes the new end offset
to `RING_PUT`. The card runs the commands before that write returns and
sets `RING_GET` equal to `RING_PUT`. The ring wraps; a command that
would cross the end is preceded by a `NOP` that fills the rest.

Each command starts with one 32-bit word: the opcode in bits 0 to 15 and
the command's length in 32-bit words, header included, in bits 16 to 31.
A command the card rejects (unknown opcode for the level, bad length, an
offset outside card memory, an undefined handle or state value) draws
nothing and changes no state. The first rejection since the last
acknowledgement sets `ERROR` and `ERROR_AT`, and the card goes on with
the next command. A length that leaves no next command to find (0, or
past what was submitted) drops the rest of the submission.

The emulator exposes the decoder to the built-in Windows as a function
(`submit(&[u32])`) that takes the same words. The built-in Direct3D
calls it without going through the bus, which removes the cost #538
measures on the Voodoo3 (12 to 15% of the emulation thread spent on
register writes). A test feeds the same command stream through the ring
and through `submit` and compares the results.

### Commands of level 1

State:

| Command | Arguments |
|---|---|
| `SET_RENDER_STATE` | pairs of (state, value) |
| `SET_STAGE_STATE` | stage, pairs of (state, value) |
| `SET_TRANSFORM` | which (world 0 to 3, view, projection, texture 0 to 7), 16 floats |
| `SET_VIEWPORT` | x, y, width, height, min z, max z |
| `SET_SCISSOR` | enable, rectangle |
| `SET_MATERIAL` | diffuse, ambient, specular, emissive, power |
| `SET_LIGHT` | slot 0 to 7, type, colours, position, direction, range, attenuation, cone, spot model |
| `ENABLE_LIGHT` | slot, on or off |
| `SET_CLIP_PLANE` | index 0 to 5, 4 floats |
| `SET_PALETTE` | palette handle, first entry, entries |

Surfaces:

| Command | Arguments |
|---|---|
| `SET_RENDER_TARGET` | colour offset, format, width, height, pitch; depth offset, format, pitch |
| `CLEAR` | flags (colour, depth, stencil), colour, z, stencil, rectangles |
| `DEFINE_TEXTURE` | handle, format, width, height, levels, flags (cube), pitch, palette handle, offset of each level and face |
| `FREE_TEXTURE` | handle |
| `SET_TEXTURE` | stage, handle or none |
| `FILL` | surface description, rectangle, value |
| `BLT` | source and destination surface descriptions and rectangles, flags (source colour key); rectangles of different sizes stretch |
| `FLIP` | offset of the new visible surface, wait for vertical blank or not |

Drawing:

| Command | Arguments |
|---|---|
| `DRAW` | primitive type, vertex format, count, vertices inline |
| `DRAW_INDEXED` | the same with 16-bit indices inline |
| `DRAW_BUFFER` | primitive type, vertex format, stride, vertex buffer offset, first, count |
| `DRAW_BUFFER_INDEXED` | the same with an index buffer offset, or indices inline |

A command's length is a 16-bit count of words, so an inline draw holds
at most 65535 words with its header and arguments. The driver splits a
larger one into several commands or draws it from a buffer.

The vertex format is a Direct3D FVF code: position as XYZ, XYZRHW
(already transformed, which bypasses transform and lighting) or XYZ with
1 to 3 blend weights; normal; point size; diffuse; specular; and up to 8
texture coordinate sets of 1 to 4 floats.

The render states and stage states are numbered and valued as Direct3D
8's are, where Direct3D has the state. Four things come from OpenGL and
have no Direct3D state, and get card states of their own:

- the pixel centre convention (Direct3D 7 and 8 put pixel centres at
  whole coordinates, OpenGL at half ones),
- which vertex of a primitive gives a flat-shaded primitive its colour
  (first in Direct3D, last in OpenGL),
- a result scale of 1, 2 or 4 on any stage operation, and the operand
  modifiers of `ARB_texture_env_combine`,
- OpenGL's spot light formula (cosine to a power), chosen for each light
  by its spot model field, and two-sided lighting.

With those, an OpenGL 1.3 driver translates to the same commands. What
OpenGL does in other coordinate conventions (depth range of -1 to 1,
bottom-left origin, clip planes in eye space) the driver folds into the
matrices it sends.

There are no state blocks, display lists or matrix stacks in the card:
the driver or the API layer keeps those and sends the resulting state.

## Feature levels

### Level 1: fixed function

Level 1 covers Direct3D 7, the fixed-function part of Direct3D 8, and
OpenGL 1.3 with the extensions listed below.

| Capability | Value |
|---|---|
| Render target formats | R5G6B5, X8R8G8B8, A8R8G8B8 |
| Depth formats | D16, D24X8, D24S8 (8 stencil bits) |
| Largest render target | 2048 x 2048 |
| Texture formats, colour | R5G6B5, X1R5G5B5, A1R5G5B5, A4R4G4B4, X8R8G8B8, A8R8G8B8, A8, L8, A8L8, P8 |
| Texture formats, compressed | DXT1, DXT2, DXT3, DXT4, DXT5 |
| Texture formats, bump | V8U8, L6V5U5, X8L8V8U8 |
| Texture size | 1 to 2048 a side, powers of two, any aspect; mip maps |
| Cube maps | yes, to 2048 |
| Volume textures | no |
| Textures at once | 4 |
| Blend stages | 8 |
| Stage operations | every `D3DTOP` from `DISABLE` to `LERP` except `PREMODULATE`, in any stage, with any argument (`TEXTURE`, `CURRENT`, `DIFFUSE`, `SPECULAR`, `TFACTOR`, `TEMP`, with `COMPLEMENT` and `ALPHAREPLICATE`), result to `CURRENT` or `TEMP` |
| Bump mapping | `BUMPENVMAP`, `BUMPENVMAPLUMINANCE`, `DOTPRODUCT3` |
| Texture coordinates | 8 sets; generated from camera-space position, normal, reflection vector, and sphere map; transforms of 1 to 4 components; projected |
| Addressing | wrap, mirror, clamp, border with a colour |
| Filtering | point, bilinear, trilinear, anisotropic to 8; LOD bias |
| Colour key | source colour key on textures, for DirectX 5 and 6 games |
| Lights | 8 active; directional, point and spot; specular; local viewer |
| Vertex blending | 1 to 3 weights (4 matrices), not indexed |
| User clip planes | 6 |
| Fog | vertex fog and table fog; linear, exponential, exponential squared; by depth or by range |
| Depth | all compare functions, bias, write mask; no W buffer |
| Stencil | all compare functions and operations, masks |
| Alpha test | all compare functions |
| Blending | all Direct3D 8 factors; add, subtract, reverse subtract, min, max |
| Colour write mask | per channel |
| Fill modes | point, wireframe, solid; flat and Gouraud shading; culling |
| Points | size to 64, point sprites, distance scaling |
| Dithering | 4x4 ordered, on 16-bit targets when the state is on, applied before the blend |
| Indices | 16-bit; 65535 primitives a draw |
| Vertex and pixel shaders | none (version 0.0) |

Reasons for the values a game can see:

- Four textures at once and 2048 texels a side are within what cards of
  2001 reported, by our recollection of the GeForce3; no capability dump
  was found to confirm the numbers (see "Unverified facts"). A game that
  loops over the count, or sizes a shadow or light map from the size,
  gets a value it was tested with. Eight textures at once would cost
  nothing to draw, and would save a game a pass that a current GPU
  doesn't notice. Cards reported eight only from late 2002, so most of
  the target games never ran with that value, and some engines size
  fixed arrays from the largest one their developers saw.
- There is no W buffer. Games used one against z-fighting in 16-bit
  depth, and only on cards that reported it; the GeForce cards, by our
  recollection, did not, so the z-buffer path is the one games were
  tested on most, and 24-bit depth removes most of the reason for it.
  Of the traced games only Giants takes a W buffer when one is offered,
  and it plays on plain Z without. A later level can add it. A frozen
  level could never drop it.
- The stage operations go beyond any one real card on purpose. A
  GeForce2 had `DOTPRODUCT3` and, by the evidence found, no
  `BUMPENVMAP`; the Matrox G400 and the Radeon had `BUMPENVMAP`. With
  generated shaders each operation costs a few lines, and a card that
  accepts all of them never sends a game down a fallback path. This also
  settles #488 and #491 for games that move to this card.
- A8R8G8B8 and the 8-bit formats are what Grom needs; DXT1 to DXT5 what
  Zanzarah needs.
- Hardware T&L with 8 lights is what Direct3D 7's T&L HAL devices
  offered and what Giants, by the reviews, checks before it turns Dot3
  on.

The Direct3D devices offered on this card are the HAL and the T&L HAL.
A game that picks the plain HAL gets transform and lighting on the
processor, by the code that does it for the Voodoo3 today
(`src/win32/ddraw/d3d/pipeline.rs`), and the card draws XYZRHW
vertices, as Direct3D 7 worked on a GeForce.

OpenGL on level 1 is version 1.3 with these extensions, each of which
NVIDIA's table lists for both the GeForce 256/2 and the GeForce3
families: `ARB_multitexture`, `ARB_texture_compression`,
`EXT_texture_compression_s3tc`, `S3_s3tc`, `ARB_texture_cube_map`,
`ARB_texture_env_add`, `ARB_texture_env_combine`,
`ARB_texture_env_dot3`, `EXT_texture_env_dot3`, `ARB_transpose_matrix`,
`EXT_fog_coord`, `EXT_secondary_color`, `EXT_separate_specular_color`,
`EXT_compiled_vertex_array`, `EXT_texture_filter_anisotropic`,
`EXT_texture_lod_bias`, `EXT_stencil_wrap`, `EXT_blend_color`,
`EXT_blend_minmax`, `EXT_blend_subtract`, `EXT_paletted_texture`,
`EXT_rescale_normal`. NVIDIA's own extensions (`NV_register_combiners`,
`NV_vertex_array_range`, `NV_texture_env_combine4`) are left out: a game
that finds them missing uses its ARB path, which every other vendor's
card took.

### Level 2: Direct3D 8.1 shaders (outline)

Not designed in detail here, and not needed by the eight target games.
It would add vertex shaders 1.1 and pixel shaders 1.1 to 1.4 as Direct3D
bytecode (`DEFINE_SHADER`, `SET_SHADER`, `SET_SHADER_CONSTANTS`), vertex
streams with declarations, 32-bit indices, volume textures, and indexed
vertex blending. On level 1, Direct3D 8 runs vertex shaders on the
processor (`src/win32/d3d8/shader.rs`), as it did on a GeForce2, and
reports no pixel shaders.

## Defined behaviour

The software renderer is the definition. This section says what it
computes, so that the definition is written down apart from the code,
and where the OpenGL renderer may differ.

### Vertices

Transform, lighting, fog factors, texture coordinate generation and
clipping are computed in IEEE single precision, in the order
`pipeline.rs` uses today, which Wine's conformance tests already check.
Screen positions are rounded to 1/256 of a pixel before rasterising.

Anything the guest can read back from vertex processing
(`ProcessVertices`, clip status, extents) is always computed by this
code on the processor, in both renderers.

### Rasterising

Triangles cover the pixels whose centres are inside them, with
Direct3D's top-left rule for centres on an edge. The pixel centre is at
whole or at half coordinates as the card state says. Lines and points
follow Direct3D's rules. Colours, fog and texture coordinates are
interpolated with perspective correction; depth is interpolated linearly
in screen space and stored as `round(z * (2^bits - 1))`.

### Texels

A texture sample is four 8-bit channels. Formats with fewer bits are
widened by bit replication. DXT blocks are decoded by one decoder with
fixed integer formulas (for the two interpolated colours of a four-colour
block, `(2*a + b + 1) / 3`), which both renderers use: the OpenGL
renderer uploads the decoded texels instead of handing the blocks to
the host's GPU, whose decoders round differently from one vendor to the
next. Textures of this period fit many times over in a current GPU's
memory when decoded.

Bilinear filtering weights have 8 fraction bits. The mip level is chosen
from the larger of the two screen-space derivatives. Anisotropic
filtering in the software renderer is trilinear filtering at the level
the smaller derivative gives; this is the one place where the reference
is deliberately simpler than the GPU.

### The stage cascade

The eight stages work on 8-bit integers per channel, 0 to 255, with a
sign and extra range only inside an operation:

- `MODULATE` is `round(a * b / 255)`; `MODULATE2X` and `4X` scale that
  and clamp.
- `ADDSIGNED` is `a + b - 128`; `ADDSMOOTH` is
  `a + b - round(a * b / 255)`.
- `DOTPRODUCT3`, with
  `s = (a.r-128)*(b.r-128) + (a.g-128)*(b.g-128) + (a.b-128)*(b.b-128)`,
  is 0 when `s` is 0 or less and `min(255, (4 * s + 127) / 255)`
  otherwise, the division rounded down, replicated to all channels.
- `BUMPENVMAP` reads signed (du, dv) from its texture, multiplies them
  by the stage's 2x2 matrix in floating point, adds the result to the
  next stage's texture coordinates, and passes `CURRENT` through.
  `BUMPENVMAPLUMINANCE` also multiplies the next stage's sample by
  `L * scale + offset`.
- Every stage result is clamped to 0 to 255.

The generated shaders compute the cascade in GLSL integers with the same
formulas, so given the same inputs they produce the same output.

### After the cascade

Specular add, fog blend, alpha test, depth and stencil tests, dithering,
blending and the write masks follow, in integers in the software
renderer. Table fog computes its factor for each pixel from depth or
from w.

On a 16-bit target the order of dithering and blending is the card's
own. With the dither state on, the 4x4 matrix is added to the fragment's
colour and the colour is reduced to the target's bits (5, 6 and 5)
before the blend. The blend then works on that colour and on the
destination's, both widened to 8 bits, and its result is rounded to the
nearest value the target holds. Without blending this is ordinary
ordered dithering. With it, blended layers show slightly more banding
than on a card that dithered after the blend. The order is chosen
because a host GPU can do it at full speed: the shader dithers, and the
GPU blends into a 565 target and rounds. Dithering after the blend would
need the blend done in the shader, with the destination read back for
each overlapping draw, which is what makes the Voodoo renderer slow on
smoke and particles.

### Where the OpenGL renderer may differ

| Source | Bound |
|---|---|
| Vertex arithmetic in a vertex shader (operation order, fused multiply-add) | positions within 1/256 pixel, so a pixel on a triangle's edge may fall on the other side |
| Interpolation of colours and coordinates | 1 in 255 on a stage input |
| Bilinear weights | 1 in 255 on a sample |
| Mip level selection, anisotropic filtering | the host GPU's; compared by eye and by a mean error over the frame |
| Blending done by the host GPU | 1 in 255 on a channel; one step of the target's on a 16-bit target |
| 16-bit targets on a host with no renderable 565 format | blended layers keep 8 bits until they are read back; several steps of the target's |

Tests compare the two renderers on fixed scenes with these bounds (see
"Testing"). Under `--deterministic` the software renderer draws and none
of this applies.

## How the emulator draws it

### One pipeline description, two back ends

A draw's state reduces to two keys, as in wined3d:

- the vertex key: transformed or not, blend count, lights by type,
  material sources, normalisation, fog mode, and for each coordinate set
  its source and transform;
- the fragment key: for each stage the colour and alpha operations,
  arguments, destination, result scale, texture type and projection;
  then fog mode, alpha test function, colour key, specular add, flat
  shading, and the target's format class.

Matrices, lights, material, factors and fog parameters are values
supplied with the draw and are in neither key.

A key is lowered once into a short list of typed operations (sample
stage 2, modulate, add signed, clamp, fog blend). Two back ends consume
that list:

- the software renderer runs it over the spans of each triangle;
- the OpenGL renderer prints it as GLSL.

Stage semantics are written once, in the lowering, and each back end
implements only the primitive operations. A new stage operation, or the
shaders of level 2, are additions to the lowering and to the two small
sets of primitives.

### The software renderer

A new module (`src/toadforce/raster/`), which draws into card memory
with the span workers of the Voodoo renderer's kind: up to four threads
on interleaved rows, so the picture doesn't depend on the thread count.
It is what `--deterministic`, the tests, the web build, and hosts
without OpenGL 3.2 use.

It is slow next to a GPU, and the design accepts that (rule 7). Each
pixel runs up to eight stages and four texture samples on the
processor, with transform and lighting for every vertex before it, on
threads the emulated machine also needs. The aim is a 640x480 game with
one or two textures playable on it. A game at 1024x768 with four
textures, or one that leans on hardware T&L for its vertex counts,
isn't expected to reach full speed, and a user with such a game needs a
host with OpenGL 3.2. What its speed turns out to be is measured in
phases 2 and 3 and written here.

It doesn't break rule 2 of the built-in Windows, which keeps a second
3D pipeline out of the Direct3D layer: this renderer is the card's, as
the Voodoo's software renderer is that card's, and Direct3D still has
one path, the HAL.

### The OpenGL renderer

On the existing context (`glow`, OpenGL 3.2 core, `src/display/gl.rs`),
shared with the window and the Voodoo renderer, and replayed on the main
thread as the Voodoo's mirror is: the card records decoded commands, and
the display runs them once a host frame and whenever something needs the
pixels.

- **Vertex work on the GPU.** Untransformed draws go to a generated
  vertex shader that transforms, blends, lights, computes fog and
  generates texture coordinates. Vertex buffers in card memory become
  host buffers, cached by offset and page write counters, so static
  geometry is uploaded once.
- **Textures** are host textures cached by handle, with the write
  counters of the pages their levels cover. A changed page re-decodes
  and re-uploads the levels on it.
- **Render targets** are host framebuffers. A target drawn on the GPU is
  marked newer there; a guest read of its memory, a `BLT` from it on the
  processor, a save state, or a switch to the software renderer reads
  the rows back first. A `Lock` for writing the whole surface discards
  the GPU's copy instead of reading it back.
- **16-bit targets** are host framebuffers in the 565 format
  (`GL_RGB565`, renderable since OpenGL 4.1 and with
  `ARB_ES2_compatibility`), so the host's blend result is rounded to
  the target's bits as the card defines. The shader does the dithering.
  The format's presence doesn't prove it works: OpenGL 4.1 left it out
  of its required formats (4.2 added it), and ANGLE replaces it with an
  8-bit format on Macs with AMD GPUs, whose drivers get packed formats
  wrong. So the renderer tests it once at start: it draws a blend with
  a known result into a 565 target, reads it back, and compares with
  the card's rounding. A host that fails, or has no such format, uses
  an 8-bit target and reduces the colours when they are read back, with
  the wider bound in the table above.
- **The visible surface** is shown from its host texture without
  passing through card memory, which is what #334 asks for on the
  Voodoo.
- **`BLT` and `FILL`** between surfaces that are current on the GPU run
  there (#502 asks the same for the Banshee).
- **Host options** that don't change what the guest reads: an internal
  resolution scale, multisampling, and forced anisotropy, like
  `voodoo_scale` and `voodoo_msaa`. A read-back from a scaled target is
  filtered down to the target's own size.

OpenGL through `glow` is kept, instead of adding wgpu or Vulkan, because
the window, the Voodoo renderer and the libretro core's frontends all
provide and expect an OpenGL context, and one context serves all of
them. The renderer sits behind the same kind of interface as
`voodoo::gpu::GpuLink`, so another back end can be added behind it
later.

### Shader cache

Programs are cached by key in memory. Compiled program binaries are
stored on disk by a hash of the generated source (`glGetProgramBinary`,
core since OpenGL 4.1, with a fallback to compiling on hosts without
it), so a game's second run compiles nothing. A game of this period sets
tens to a few hundred distinct state combinations.

If a first run still shows pauses in games, the answer is a general
shader that reads the lowered operation list from uniforms and draws
until the specialised one is compiled, which is what DXVK 3.0 and
Dolphin do. The operation list makes that shader an interpreter of a
small instruction set. It is left out until a measurement asks for it.

### Save states and determinism

`impl State` for the card reads back whatever is newer on the GPU and
saves the registers, the state the commands set (a fixed-size block:
render and stage states, matrices, lights, material, clip planes, the
texture and palette tables), the mode shown, a pending flip, the
position in the frame, card memory and the ring. Loading rebuilds the
host caches from card memory, as the Voodoo's `Resync` does. The ring
holds no commands still to run, since the card consumes them at each
`RING_PUT`, but the guest reads back what it wrote there, so its
contents are saved with the rest.

With `--deterministic` no GPU link is attached, as for the Voodoo, and
the software renderer draws. A change to the OpenGL renderer can't be
compared in deterministic runs, as AGENTS.md says of the Voodoo's.

## Guest software

### The built-in Windows

This is where the card is used first, and it needs no guest driver: the
DLLs are the emulator's.

- **Display.** GDI's desktop and DirectDraw's modes use the card's mode
  registers, at 8, 16 or 32 bits a pixel. DirectDraw surfaces in video
  memory are in BAR 1. A colour fill, and a copy of equal size between
  two of them, is a `FILL` or `BLT` of the card's; a flip is its
  `FLIP`. The driver builds the words a guest's driver would put in the
  ring and hands them to the card without the ring
  (`Bus::toadforce_submit`), so both run one decoder. Blits the card
  doesn't do as DirectDraw's own code does (stretches, which sample
  differently, mirrors, destination keys) stay with that code.
- **Direct3D 7.** `ddraw/d3d/hal.rs` is written against the Voodoo. It
  becomes one implementation of a HAL interface (capabilities, texture
  formats, texture handles, state, draws), and ToadForce is the second.
  The ToadForce HAL lists the level's formats, offers the HAL and T&L
  HAL devices, and turns state and draws into card commands. The 16-bit
  shadows of 32-bit targets (`d3d/shadow.rs`) and the copies of
  textures the Voodoo can't sample in place aren't used on it.
- **Direct3D 8** (`d3d8/`) sits on the Direct3D 7 device today and
  keeps doing so, with `D3DCAPS8` filled from the level's table and
  hardware vertex processing offered for fixed-function draws.
- **OpenGL** (`opengl32/`) is 1.1 on the Direct3D 7 device today. It
  grows to 1.3 with the level's extensions, sending the card states
  OpenGL needs that Direct3D has no state for. Its OpenGL front end
  (contexts, state, immediate mode, arrays, display lists, pixel
  transfer) is kept apart from its WGL and Win32 parts and takes its
  calls through one interface, so that a booted Windows can later feed
  the same code (see below).
- **Glide** isn't offered on this card. A game that wants Glide is
  configured with a Voodoo.

### A booted Windows 98

A booted Windows needs a driver, which the project would write and ship.
SoftGPU's drivers show that this is feasible and give MIT-licensed
references for each part:

- **The display minidriver** (16-bit, Open Watcom, on Microsoft's DIB
  engine), which sets modes through the mode registers. VMDisp9x and
  boxv9x, which it is based on, do this for Bochs' VBE registers.
- **The DirectDraw and Direct3D HAL** (a 32-bit DLL named through
  `DCICOMMAND`). The command set is close to Direct3D's own driver
  command stream, `DrawPrimitives2`: `D3DDP2OP_RENDERSTATE`,
  `TEXTURESTAGESTATE`, `SETTRANSFORM`, `SETMATERIAL`, `SETLIGHT`,
  `CREATELIGHT`, `SETCLIPPLANE`, `SETRENDERTARGET`, `CLEAR`,
  `VIEWPORTINFO`, `ZRANGE` and the draw tokens each map to one card
  command. The driver reports `D3DDEVCAPS_HWTRANSFORMANDLIGHT` and
  `D3DDEVCAPS_DRAWPRIMITIVES2EX`, the texture formats as
  `DDSURFACEDESC` entries (FourCC ones for DXT), and `D3DCAPS8` through
  `GetDriverInfo2`. Microsoft's Direct3D runtime does the API's
  validation, state blocks and managed textures.
- **OpenGL** is not offered on a booted Windows at first: OpenGL games
  run on the built-in Windows. How to add it is decided when phase 6
  reaches it, between three ways:
  - A thin installable client driver, found through the
    `OPENGL_GETINFO` escape, that writes each OpenGL call into the ring,
    with the client arrays a draw uses copied in at the draw. The card
    runs the calls through the Rust front end of the built-in OPENGL32.
    Both kinds of Windows then get one OpenGL implementation with one
    behaviour, for the least code. The card gains a second family of
    commands, defined by OpenGL 1.3 itself, and the OpenGL contexts
    become card state that save states hold. This is the preferred way.
  - An old Mesa (up to about version 7, whose drivers still received
    fixed-function state and could use hardware T&L) with a driver for
    the card. Current Mesa hands its drivers fixed function already
    turned into shaders, which level 1 can't run. The card keeps one
    command family, at the price of a second OpenGL implementation
    whose behaviour can differ from the built-in one, in an old code
    base.
  - A complete OpenGL 1.3 front end of our own in guest code: the same
    second implementation, and the most code.

The driver would be handed to the guest on a disk the emulator
generates, as `src/shared_disk.rs` does for shared folders.

This comes last because the built-in Windows reaches the target games
without it, and because the driver should be written against a command
set that the built-in Windows has already exercised in real games.

## Configuration

The keys, following the Voodoo's. `machine` and `toadforce_memory`
exist; `toadforce_level` comes with level 1, since a card with one
level has none to choose, and the renderer's keys with the renderers.

| Key | Values | Default |
|---|---|---|
| `machine` | `svga_toadforce` | |
| `toadforce_level` | `1` (later `2`): the highest level the card offers | newest |
| `toadforce_memory` | `32`, `64`, `128` | `64` |
| `toadforce_renderer` | `software`, `opengl` | `opengl` where a context exists |
| `toadforce_scale` | `1` to `4` | `1` |
| `toadforce_msaa` | `off`, `2`, `4`, `8` | `off` |
| `toadforce_anisotropy` | `off` (the game's), `2`, `4`, `8`, `16` | `off` |

## Testing

- **The reference, operation by operation.** Unit tests of each lowered
  operation and each stage operation against values computed by hand
  from the formulas above, and of the DXT decoder against hand-decoded
  blocks.
- **Wine's conformance tests.** `tests/conformance` already runs
  Wine's ddraw and d3d8 tests, with the DXT-in-video-memory, cube map
  and T&L light tests skipped on the Voodoo3
  (`tests/conformance/README.md`). They run unskipped on ToadForce.
- **Renderer agreement.** Fixed scenes (one for each stage operation,
  fog mode, light type, blend mode, texture format and filter) drawn by
  both renderers and compared within the bounds of "Where the OpenGL
  renderer may differ". Whether CI has an OpenGL context for this (Mesa's
  llvmpipe on the Linux job) has to be checked; without one these run
  locally.
- **Ring against `submit`.** One command stream through both, equal
  memory and state after.
- **Frozen levels.** A golden copy of each level's capability table and
  accepted command list.
- **Save states.** Save in the middle of a frame, load, and compare the
  finished frame with an uninterrupted run, in deterministic mode.
- **Determinism.** The existing determinism tests
  (`src/debug/determinism_tests.rs`) with a ToadForce scene.
- **Games.** Each phase ends with its target games played on the
  branch, compared with main on the Voodoo3 where main runs them, under
  the pull request rules of AGENTS.md.

## Phases

Each phase is one or a few pull requests that leave main working.

0. **Traces.** Log what the eight games read and set, on the Voodoo3
   and the built-in Windows. Done as far as the Voodoo3 lets the games
   go (see "Phase 0 traces"). The traces are repeated on the card at the
   end of phases 2, 3 and 5, and level 1 is frozen after phase 5.
1. **The card without 3D.** PCI function, registers, card memory, mode
   setting, the ring with `NOP`, `FILL`, `BLT` and `FLIP`, save states.
   The built-in Windows' GDI and DirectDraw run on it at 8, 16 and 32
   bits. Proof: 2D DirectDraw games run as on the `Svga` adapter. Done:
   the card is `src/toadforce`, its place in the machine
   `src/bus/toadforce.rs`, and the built-in Windows' driver the third
   kind of adapter in `src/win32/screen.rs` with
   `src/win32/ddraw/toadforce.rs`.
2. **The software renderer and the Direct3D 7 HAL**, for transformed
   vertices first: the lowering, the stage cascade, all texture formats
   including DXT and bump formats, depth, stencil, blending, fog. The
   HAL interface split in `ddraw/d3d`, with rule 2 of
   `builtin-windows-architecture.md` ("Direct3D runs on the emulated
   3dfx card's HAL") reworded to the emulated card's HAL, whichever
   card it is. Proof: Zanzarah's Fairy Garden with DDS textures and no
   game fix (#583, #606), and Grom past `screen.Init` (#645).
3. **Transform and lighting in the card.** Untransformed draws, lights,
   vertex blending, clip planes, texture coordinate generation, the T&L
   HAL device. Proof: Giants' terrain with bump mapping. Black & White
   joins the proof once #889 lets it run on the built-in Windows, or
   with the Windows 98 driver of phase 6.
4. **The OpenGL renderer.** Generated shaders, caches, GPU-owned
   targets, the disk cache. Proof: renderer agreement tests, and the
   phase 2 and 3 games at their full frame rate on it, measured against
   the software renderer on a four-core host as well as a larger one.
5. **Direct3D 8 and OpenGL 1.3 on the card**, with the OpenGL front end
   separated from WGL. Proof: Max Payne, MDK2, Oni.
6. **The Windows 98 driver**: display minidriver, then the DirectDraw
   and Direct3D HAL. OpenGL there is decided and done after those.
7. **Level 2**, when a game that needs shaders is picked up.

Phases 2 and 3 come before the OpenGL renderer so that the reference
exists before the renderer that is tested against it, and so that
Zanzarah and Grom, which need formats more than speed, are unblocked
early. This is the order of building. The order of importance is rule
7's: phases 2 and 3 write the lowering, the caches' keys and the
command decoding for the OpenGL renderer to use, and the games of those
phases may run below full speed until phase 4.

## Alternatives considered

**A real card at register level (GeForce 256/2, Radeon).** It would run
the vendor's own drivers on a booted Windows with no guest code of ours.
The documentation isn't there (see "The problem"), the driver would
exercise undocumented behaviour from its first frame, and the feature
set would be one card's: a GeForce2 still lacks `BUMPENVMAP`.

**The Voodoo 4 or 5 (VSA-100).** #615 names it. It is close to the
Voodoo3 the emulator has, and adds 32-bit colour, 2048 texels and DXT
and FXT1. It has no hardware T&L and no bump mapping operations, so it
answers Zanzarah and Grom and none of the T&L games. It remains a
reasonable small project of its own for Glide games in 32-bit colour,
and this proposal doesn't replace it.

**Passing API calls to the host (qemu-3dfx's way).** The least code for
the first picture. The guest then sees the host's driver, save states
would have to capture a host OpenGL context, and deterministic runs
would have nothing to draw with.

**Host-side objects, as SVGA3D's surfaces.** Textures and targets would
live only on the host and be filled by copy commands. It avoids
read-backs on `Lock`, and costs what card memory gives for nothing: a
save state that is just memory, `Lock` pointers into the card, and a
software renderer that draws where the guest looks. The write counters
and the ownership tracking recover most of its speed.

**Transform and lighting always on the processor.** The pixels would
then depend less on the host, since both renderers would get the same
screen vertices. It gives up the vertex shader path that a current GPU
runs for nothing, and puts the cost on the emulation thread for every
frame of the games this card is for. Guest-readable results stay on the
processor either way.

**Handing DXT blocks to the host GPU.** Less host memory and no decode.
Vendors' decoders round differently, so the same texture would give
different texels on different machines, and the software renderer would
have to pick one of them. Decoding once in our own code costs little for
textures of this size.

## Unverified facts

One decision is left for later, and the text says where: how OpenGL
reaches a booted Windows 98 (see "A booted Windows 98").

Checked on 2026-11-10:

- The PCI ID database (pci-ids.ucw.cz) lists no vendor 1234h and no
  device 7F01h under any vendor, so no registered device has the card's
  pair.
- `GL_RGB565` can't be assumed to work wherever it is present (see
  "The OpenGL renderer"), which is why the renderer tests it at start.

- The traces ("Phase 0 traces") found one game, Giants, that takes a W
  buffer when offered. It plays without one.

Still to check:

- Whether Giants' terrain shows z-fighting on plain Z with 24-bit
  depth, in phase 3.
- What Giants asks of a T&L device, once the card offers one, and
  what Black & White asks of any card, once it runs (#889).
- What Zanzarah does with DDS textures, and what Grom asks for past
  its initialisation, once the card gets them past #583 and #645.

Facts the research could not confirm, which the design avoids depending
on or marks as recollection:

- That the GeForce cards reported no W buffer, and that cards reported
  eight textures at once only from late 2002.

- The capability numbers of the real cards (textures at once, texture
  sizes, lights, clip planes, anisotropy). No capability dump was found.
- That `BUMPENVMAP` was absent on the GeForce 256 and 2. It agrees with
  NVIDIA listing `NV_texture_shader` for the GeForce3 family only.
- Which Direct3D operations Giants uses for its bump mapping, beyond
  reviewers calling it Dot3 and tying it to T&L.
- What Oni and Max Payne do once the compressed and 32-bit formats
  they ask about are there, and whether MDK2 asks for more than OpenGL
  1.1 on a card that lists more.
- How vertex buffers reach a Direct3D 7 T&L driver on Windows 9x
  (execute-buffer surfaces and their callbacks), and whether the DIB
  engine tells a minidriver when it touches the frame buffer. The card
  doesn't need to be told (see "Memory"), so neither blocks the design.
- The licence of the Windows 98 and 2000 DDK samples. They are reading
  material at most; the reusable references are the MIT-licensed
  SoftGPU drivers.

## Licensing

The card's definition and its emulation are new code under the
project's GPL-2.0-or-later. Wine's fixed-function shader generator
(LGPL-2.1 or later) can be ported under the terms the project already
uses for Wine's Direct3D. VMware's SVGA3D headers (GPL-2.0 or MIT) are a
model for the command and capability lists; no code is needed from
them. xemu's shader generators are a reference to read; its licence
was not checked for this document, and has to be before any of its code
is ported. A Windows 98 driver based on VMDisp9x and VMHAL9x (MIT) is
compatible with the project's licence. Nothing from Microsoft's DDKs is
copied.

## References

Wine and DXVK:

- `dlls/wined3d/glsl_shader.c`, fixed-function shader generation and
  the two shader caches: https://github.com/wine-mirror/wine/blob/master/dlls/wined3d/glsl_shader.c
- `dlls/wined3d/wined3d_private.h`, `struct ffp_frag_settings` and
  `struct wined3d_ffp_vs_settings`: https://github.com/wine-mirror/wine/blob/master/dlls/wined3d/wined3d_private.h
- `dlls/wined3d/utils.c`, the key builders and the DXT decoder:
  https://github.com/wine-mirror/wine/blob/master/dlls/wined3d/utils.c
- `dlls/wined3d/ffp_hlsl.c`, the newer HLSL generator:
  https://github.com/wine-mirror/wine/blob/master/dlls/wined3d/ffp_hlsl.c
- `dlls/ddraw/ddraw.c` and `vertexbuffer.c`, the T&L HAL device and
  `ProcessVertices`: https://github.com/wine-mirror/wine/tree/master/dlls/ddraw
- DXVK 2.4's per-key fixed-function compiler:
  https://github.com/doitsujin/dxvk/blob/v2.4/src/d3d9/d3d9_fixed_function.cpp
- DXVK releases (D8VK merged in 2.4; general shaders in 3.0):
  https://github.com/doitsujin/dxvk/releases

Paravirtual and emulated cards:

- SVGA3D commands: https://git.kernel.org/pub/scm/linux/kernel/git/torvalds/linux.git/plain/drivers/gpu/drm/vmwgfx/device_include/svga3d_cmd.h
- SVGA3D capabilities: https://github.com/torvalds/linux/blob/master/drivers/gpu/drm/vmwgfx/device_include/svga3d_devcaps.h
- SoftGPU: https://github.com/JHRobotics/softgpu
- VMDisp9x: https://github.com/JHRobotics/vmdisp9x
- VMHAL9x: https://github.com/JHRobotics/vmhal9x
- qemu-3dfx: https://github.com/kjliew/qemu-3dfx/wiki/QEMU-3D-APIs-Pass-Through
- virgl: https://docs.mesa3d.org/drivers/virgl/
- xemu's shader generators: https://github.com/xemu-project/xemu/tree/master/hw/xbox/nv2a/pgraph/glsl
- xemu's shader cache: https://github.com/xemu-project/xemu/blob/master/hw/xbox/nv2a/pgraph/gl/shaders.c
- NV2A combiners: https://xboxdevwiki.net/NV2A/Pixel_Combiner
- envytools' register database: https://github.com/envytools/envytools/tree/master/rnndb/graph
- 86Box's video devices: https://github.com/86Box/86Box/tree/master/src/video
- `ARB_ES2_compatibility`, which adds `GL_RGB565` (issue 12 on
  OpenGL 4.1 and 4.2): https://registry.khronos.org/OpenGL/extensions/ARB/ARB_ES2_compatibility.txt
- ANGLE's replacement of packed formats on Macs with AMD GPUs:
  https://trac.webkit.org/r271062
- Dolphin on integer combiner arithmetic:
  https://dolphin-emu.org/blog/2014/03/15/pixel-processing-problems/

Windows driver interfaces:

- Windows 95 DDK, display drivers: https://techshelps.github.io/MSDN/WIN95DDK/HTML/S27C2.HTM
- Windows 9x video minidrivers: https://www.os2museum.com/wp/windows-9x-video-minidriver-hd/
- Hardware transform and lighting in a DirectX 7 driver:
  https://learn.microsoft.com/en-us/previous-versions/windows/drivers/display/hardware-transform-and-lighting
- `D3DHAL_DP2OPERATION`: https://learn.microsoft.com/en-us/windows-hardware/drivers/ddi/d3dhal/ne-d3dhal-_d3dhal_dp2operation
- `D3DHAL_GLOBALDRIVERDATA`: https://learn.microsoft.com/en-us/windows-hardware/drivers/ddi/d3dhal/ns-d3dhal-_d3dhal_globaldriverdata
- DirectX 8 capabilities through `GetDriverInfo2`:
  https://learn.microsoft.com/en-us/previous-versions/windows/drivers/display/reporting-directx-8-0-style-direct3d-capabilities

Cards and games:

- NVIDIA's table of OpenGL extension support (pages 4 to 6):
  https://developer.download.nvidia.com/opengl/specs/nv30specs.pdf
- Giants and Dot3: https://www.tomshardware.com/reviews/radeon-8500,385-5.html
  and https://www.beyond3d.com/content/reviews/13/3
- Max Payne's requirements: https://legacy.3drealms.com/news/2001/06/max_payne_specs.html
- Max Payne's use of T&L: https://www.computerbase.de/artikel/grafikkarten/asus-v8440-td-und-v8460-ultra-td-test.74/seite-14

In this repository: #615 (a card with DXTn), #583 and #606 (Zanzarah),
#645 (Grom), #593 and #613 (the closed game patches), #538 (the cost of
register writes), #334 and #502 (GPU-owned frame buffers and 2D),
#488 and #491 (stage operations on the Voodoo3 HAL).
