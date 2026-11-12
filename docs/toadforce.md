# ToadForce: a synthetic 3D card for games of 2000 to 2002

Status: being built. "Phases" says what is done; the card as built is
specified in `toadforce-spec.md`. The document covers the research, the
card's definition, how the emulator draws it, what the guest software
needs, and the order of the work. Facts taken from other projects have
their source in "References"; what could not be confirmed is listed
under "Unverified facts".

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

The first eight decide what level 1 contains. The table says what each is
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
| Nocturne | 1999 | Direct3D 6 interfaces, from its DirectX 7 renderer | `DDCAPS_3D` in the driver's caps (#982); A8R8G8B8 textures and a 32-bit target with a Z32 z-buffer, which it picks where a card offers them | traced (see "Nocturne on the card") |

Nocturne joined the list on 2026-11-12, after level 1 was fixed. It
needs nothing of the card that the eight didn't, and is on the list
for its frame rate and for what it asks of the built-in Windows (see
"Nocturne on the card").

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

Surfaces:

| Command | Arguments |
|---|---|
| `SET_RENDER_TARGET` | colour offset, format, width, height, pitch; depth offset, format, pitch |
| `CLEAR` | flags (colour, depth, stencil), colour, z, stencil, rectangles |
| `DEFINE_TEXTURE` | handle, format, width, height, levels, flags (cube, colour key), palette offset, colour key, offset and pitch of each level and face |
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

A palette is 256 words of card memory, which a `P8` texture names by
their offset. It needs no command and no handle: the guest writes it as
it writes texels, and the card reads it when a draw samples the
texture.

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
The T&L HAL's description has `D3DDEVCAPS_HWTRANSFORMANDLIGHT` and the
card's 8 lights, and the two draw the same way. On a GeForce, Direct3D 7
transformed and lit the plain HAL device's vertices on the processor and
the card drew XYZRHW vertices. Here the card transforms and lights the
untransformed vertices of either device, and the pipeline
(`src/win32/ddraw/d3d/pipeline.rs`) those of the draws the card can't,
with the same picture.

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

Not designed in detail here, and not needed by the nine target games.
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
| Interpolation of colours and coordinates | 1 in 255 on a stage input. An operation that multiplies its result passes the step on multiplied: 8 after `DOTPRODUCT3` and `MODULATE2X` (Giants' ground, measured) |
| A sample of a 2D texture | 1 in 255, and 2 with a projected texture. The shader takes the card's steps in the card's whole numbers (`shader::SAMPLING`): the level of detail, the levels and the filter, the coordinates in 1/256 of a texel, the weights of 8 bits. What is left is a coordinate or a level of detail that the host's interpolation puts on the other side of one of those steps: a weight one of 256 off, or the next level where the level of detail is within 1/4096 of the change. Coordinates the card has exactly (a texel to a pixel from whole coordinates) give the same texels |
| A sample of a cube map | 1 in 255 on a smooth texture, 3 where neighbouring texels differ by most of a channel's range. The host's own filter samples it: it rounds a weight to its 8 bits where the card cuts it off, works out the face's coordinates in its own arithmetic, and picks the mip level |
| Bump offsets from a linearly filtered bump texture | as the weights, except between texels whose offsets differ in sign: both renderers blend the signed bytes as unsigned ones, so there a weight that is one off can turn 127 into -128 |
| Anisotropic filtering, where the host has it | the host GPU's, which reads more texels than the card's stand-in (trilinear at the level of the shorter side); compared by eye and by a mean error. Measured: a mean of 1.5 and at most 29 on the test's square going into the distance; in Giants 12% of the frame's pixels, 0.2% of the frame by more than 8 (the ground's normal map through `DOTPRODUCT3`) |
| The alpha test and the colour key after a filtered sample | a pixel whose alpha is within the sample's bound of the reference is drawn by one renderer and not by the other. Measured: 0 to 6 pixels a frame of Zanzarah's leaves |
| Depth | 1 step of a 16-bit buffer, 4 steps of a 24-bit one: the host interpolates depth in single precision and rounds it by itself. A depth half-way between two steps goes either way, and so does a test between two surfaces closer than the bound: Giants' marks on the ground, 2 steps of 24 bits in front of it, differ by more than 8 on 0.06% of the frame's pixels, where one renderer alone drew the mark |
| Blending done by the host GPU | 1 in 255 on a channel; one step of the target's on a 16-bit target |
| 16-bit targets on a host with no renderable 565 format, or one that doesn't round a blend to the nearest value | blended layers keep 8 bits until they are read back; several steps of the target's |

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
  the target's bits as the card defines. The shader does the dithering,
  and rounds a colour that is written without a blend to the target's
  bits itself, so such a colour doesn't hang on the host's rounding.
  The format's presence doesn't prove it works: OpenGL 4.1 left it out
  of its required formats (4.2 added it), ANGLE replaces it with an
  8-bit format on Macs with AMD GPUs, whose drivers get packed formats
  wrong, and a GeForce RTX 4080 cuts a blend's result off where the
  card rounds it. So the renderer tests it once at start: it fills a
  565 target and reads it back, writes colours each side of where the
  bits round, and draws blends whose results are clear of the middle
  between two values, and compares what it reads back with the card's
  rounding. A host that fails, or has no such format, uses an 8-bit
  target and reduces the colours when they are read back, with the
  wider bound in the table above. Such a target holds a colour written
  without a blend as the card's bits widened, so only blended layers
  are wider than the card's.
- **The visible surface** is shown from its host texture without
  passing through card memory, which is what #334 asks for on the
  Voodoo.
- **`BLT` and `FILL`** between surfaces that are current on the GPU run
  there (#502 asks the same for the Banshee).
- **Host options** that don't change what the guest reads: an internal
  resolution scale, multisampling, and forced anisotropy, like
  `voodoo_scale` and `voodoo_msaa`. A read-back from a scaled target is
  filtered down to the target's own size.

Step 4b builds the renderer behind `gpu::Host` (`src/toadforce/gl.rs`),
on desktop OpenGL 3.2, and step 4c attaches it to the card of a
running machine (see "The window's side" below). It
draws the screen vertices the card hands it, so the vertex work stays
on the processor, and it has:

- the targets, with their depth and stencil buffers, filled from card
  memory and read back in the card's formats. Row 0 of a surface is
  row 0 of its host texture, which makes the host's window coordinates
  the card's and its fill rule the card's top-left rule;
- the textures of every format, decoded by the software renderer's
  decoders, with palettes, mip levels and cube maps. A texture with a
  colour key has a second host texture that marks the keyed texels;
- the card's sampling of a 2D texture, in the fragment shader
  (`shader::SAMPLING`): the level of detail from how the coordinates
  change over a pixel, the levels and the filter by `Sampler::levels`,
  the level-of-detail bias and `MAXMIPLEVEL`, the coordinates in 1/256
  of a texel, the address modes and the border colour, and the four
  texels of a level blended by weights of 8 bits, each read with
  `texelFetch`. The colour key is looked up in the level the sample
  reads first. The host's own filter samples a cube map, and a texture
  with anisotropy where the host has that filter;
- a fragment shader printed from the draw's steps, with every stage
  operation, the bump offsets and luminance, projected and cube
  coordinates, the specular add, the three fog modes by depth and by
  w, the alpha test and the dither, kept in memory by the steps;
- the depth and stencil tests, the blend, the write masks and the clip
  rectangle as the host's fixed state. The depth bias is taken from the
  depth in the vertex shader, since the unit of the host's polygon
  offset isn't a step of a 16-bit buffer on every host;
- `CLEAR` on a buffer the host has (`Host::clear`). A depth that is
  neither the nearest nor the farthest is written as pixels, since the
  host's clear converts only those two for certain; with the stencil
  kept, such a clear is left to the processor.

It declines these states, which the software renderer draws:

- bump offsets on a cube map: they move the coordinates on the face the
  direction picked, and the host's cube sampler takes a direction;
- with the host's own filter (an anisotropic sample), what that filter
  does another way than the card: `MAXMIPLEVEL` above 0 with different
  filters for magnifying and minifying (the card changes filter by the
  pixel's size in texels of level 0, the host by the level it is
  limited to); a linear filter for magnifying with the nearest texel
  for minifying, with mip levels (OpenGL magnifies up to a level of
  detail of 0.5 there, and the card only up to 0); a colour key on a
  texture blended between mip levels (the card compares the texel of
  the lower level, the host's lookup the closer one); one texture on
  two stages with different sampling states, or sampled by the host's
  filter on one and read by the shader on the other, since the states
  are the host texture's. The shader's own sampling draws all four;
- `SRCALPHASAT` as the destination's blend factor, which OpenGL takes
  from 3.3 on.

A colour key on a texture of a block format is no state to decline:
`DEFINE_TEXTURE` refuses it. A state whose program the host doesn't
compile is declined from its first draw on, since the program is
compiled when the host is asked whether it takes the state.

The agreement tests (`toadforce::gl::tests`, which need a desktop OpenGL
context) draw 332 scenes in software and through the host and compare
card memory. On a GeForce RTX 4080: untextured triangles, every stage
operation at each result scale, four stages with four textures, the
alpha test, fog, the specular add, the stencil operations, the write
masks and the blend factors and operations on a 32-bit target are the
same byte for byte. So is the nearest texel of every format, address
mode and cube face, the linear filter on a 2D texture of every format
and address mode, every way of taking the mip levels on squares of 1,
2, 4 and 8 texels to a pixel, a texture drawn a texel to a pixel from
whole coordinates, and a filtered texture into a dithered 5-6-5
target. A square going into the distance is within 1 with each mip
filter, a projected texture within 2, a cube map with the linear
filter within 3, and offsets from a linearly filtered bump texture
within 1. Anisotropic filtering differs from the software renderer's
by a mean of 1.5 over that square. Depth is within 1 step
of 16 bits and 4 of 24. That host's 565 format fails the test at
start, so its 16-bit targets were compared in 8 bits a channel alone,
and were within one step of the target's with up to five blended
layers. A scene for each declined state checks that the software
renderer draws it.

Not built yet: the vertex work on the GPU, the picture from the host's
texture, `BLT` and `FILL` on the host, the host options, and the
program binaries on disk (see "Shader cache").

OpenGL through `glow` is kept, instead of adding wgpu or Vulkan, because
the window, the Voodoo renderer and the libretro core's frontends all
provide and expect an OpenGL context, and one context serves all of
them. The renderer sits behind the same kind of interface as
`voodoo::gpu::GpuLink`, so another back end can be added behind it
later.

### Between the card and the host renderer

The card and the display run on one thread, as the Voodoo's renderers
do, so the card calls the host renderer directly. `toadforce::gpu` has
the interface (`Host`), which the display attaches to the card when it
has an OpenGL 3.2 context and the run isn't deterministic.

- **What the host gets.** The card's renderer queues the primitives of
  a run of commands as it does for the software renderer, and hands
  the host the queue (`Batch`) at the points where the software
  renderer would draw it: each entry is the lowered state
  (`DrawState`) and its triangles as screen vertices, after the
  card's transform, lighting, clipping and culling. A triangle the
  card culls, or one with no area, never reaches the host, so the
  host has no winding to agree on. The clip rectangle stays in the
  state for the host to cut the pixels by. The coordinates of texture
  slot `n` are in set `n` of every vertex, whichever set of the
  program's vertex they came from, and a flat-shaded triangle has a
  fourth vertex, the one that gives it its colours. The vertex work
  moves to the GPU later, if a measurement asks for it.
- **A draw the host doesn't take.** The host is asked for each state
  (`Host::takes`). A draw it declines (a state it has no shader for
  yet) is drawn by the software renderer, and so are lines, points,
  triangles drawn as outlines or corners, and a draw that samples the
  surface it writes: the target and the textures are read back first,
  and the target is uploaded again before the host's next draw into
  it. The host renderer can so be built up state by state, and a rare
  state stays right at the cost of speed. The queue holds draws of
  one renderer at a time, and a draw of the other empties it first,
  which keeps the card's order on a target.
- **Who has the newer pixels.** A surface the host drew into is ahead
  on the host, and card memory is stale there. Before anything reads
  or writes that memory on the processor the card reads the surface
  back (`ToadForce::settle` with the range): the program's writes of
  the frame buffer (in the bus), the drivers' reads and writes
  (`win32::screen`), the card's own `FILL` and `BLT` where they run
  on the processor, a `CLEAR` of part of a buffer the host has no
  current copy of, a texture or a vertex buffer that overlaps it
  and isn't an image of it, and the picture the screen shows when
  the window can't show the host's copy (`Bus::sync_display`).
  Running the queued commands and reading back are separate: the
  queue is run at every such point as before, and a surface is read
  back only when its own bytes are asked for.
- **The program's own reads.** A read of the frame buffer has the bus
  shared (`Bus::read_8`), and can't read anything back. The bytes are
  there before the program gets to them instead. The built-in drivers
  read a surface back when they give the program its address, at a
  lock (`win32::screen::expose`). A guest's own driver has no such
  call, so the card reads everything back when a write of `RING_PUT`
  returns, as the specification promises: such a driver gets the
  host's speed only once the card shows the picture from the host's
  texture and the driver's reads are known some other way. A read of
  bytes that are ahead all the same is a driver that handed out an
  address without `expose`; the card counts those for its status
  (`stale_reads`).
- **A saved state** is written from a shared card too. It keeps card
  memory with every surface read back into a copy
  (`ToadForce::settled`), and the card and the host stay as they are.
  After a load the host drops its copies (`Host::forget`). A
  debugger's look at card memory (`win32::screen::peek`) reads it as
  it is.
- **What the processor wrote.** The card counts the writes to each
  page of its memory (`Pages`), 4 KiB each: the program's, the
  drivers' and its own. The host keeps a stamp of the pages a copy
  was made from, of a texture or of a surface it drew into, and makes
  the copy again from card memory when a page of it was written
  since. A read-back writes card memory what the host's copy holds
  and isn't counted, and neither are the host's draws, so a host that
  draws into a target drops its other copies of the target's bytes:
  the same surface kept as a texture, or another surface over the
  same memory, such as a depth buffer of another size. Without a host
  nothing is counted.
- **What stays on the host.** A `CLEAR` of a whole surface makes the
  host's copy the newer one whatever card memory holds, so a stale
  or missing copy costs nothing. A texture whose image is a surface
  the host has ahead (a level of a texture, a face of a cube map, in
  `R5G6B5`, `X8R8G8B8` or `A8R8G8B8` without a colour key:
  `gpu::image_of`) is filled from that copy by a shader, which
  reduces a 16-bit surface kept in 8 bits a channel as a read-back
  would. `FILL`, and `BLT` between surfaces of one colour format
  with no stretch and no key, run on the host when a surface of
  theirs is ahead (`Host::fill`, `Host::blt`). A surface inside a
  larger copy of the same bytes (one depth buffer under targets of
  several sizes) uses that copy. Everything else is read back as
  before: a keyed texture, a texture that overlaps a surface
  without being an image of it, a stretch.
- **The picture shown** comes from the host's copy (`Host::shows`,
  see "The window's side").

### The window's side

The window's OpenGL screen (`display::gl::GlScreen`) owns the one
`ToadForceGl` of its context, made the first time a card needs it. A
card gets a link to it (`HostLink`, a `gpu::Host` that passes every
call on), through `Bus::toadforce_set_host`, which runs the queued
commands and reads everything back before the renderer changes.
`Display::run_toadforce` does this once a frame from the main loop:

- A machine with a ToadForce and no link gets one when
  `toadforce_renderer` is `auto` or `opengl`, the window has desktop
  OpenGL 3.2 and the run isn't deterministic. A new machine's card
  gets the same renderer, which first drops every copy it has.
- With `software`, in a deterministic run, or in a window drawn by
  SDL's renderer, a card that has a link loses it, and its memory
  holds every pixel from then on. `opengl` says once in the log why
  OpenGL doesn't draw; `auto` says nothing.
- The renderer ends with the window, before the context does. A card
  that lives longer keeps a link that takes no draw, so it draws in
  software. The emulator's own order ends the machine first.
- A loaded state keeps the link, and the renderer drops its copies
  (`Bus::toadforce_after_load`).

The renderer shares the context with the window's own drawing and
leaves OpenGL's reset state after every call (`ToadForceGl::leave`),
as the 3dfx renderer does, so neither has to know of the other. A
machine has one display adapter, so a ToadForce and a Banshee or
Voodoo3 never meet. A Voodoo Graphics or Voodoo2 board beside a
ToadForce is possible in the configuration; each renderer then
draws between the other's calls, from the reset state.

The window shows the host's copy of the surface on the screen
(`Display::show_toadforce`, `ToadForceGl::composite`) while the copy
has every pixel of it: it is ahead, or what card memory holds. The
window's cursors and messages go over it, and nothing is read back.
The picture comes from card memory, read back once a frame
(`Bus::toadforce_show`), in every other case:

- something wants the frame in memory: a screenshot, a recording, the
  debugger's picture, the settings window or an overlay, a phosphor
  monitor, a deterministic run;
- the processor wrote the surface since the host had it, as GDI and
  DirectDraw's blits on the processor do, until the host draws into
  it again;
- a mode with a palette, or one shown doubled.

The surface shown is the one the card shows as of the commands it has
run, with the queued ones left queued. The one latched at the last
retrace would be wrong here: a program that flipped is already
drawing its next frame into it.

`/api/status` has the renderer's name and what it costs under
`video.toadforce` (`docs/llm-debugging.md`): the read-backs by what
asked for them, the declined draws, and the renderer's own counts.
`RUST_DOS_TOADFORCE_GL_DETERMINISTIC=1` keeps OpenGL drawing in a
deterministic run, for comparing the two renderers' frames at equal
emulated times. It is a tool for that comparison only: the frames of
such a run depend on the host's GPU.

### What the first games showed (2026-11-12)

Deterministic runs of one build with each renderer, frames compared
at equal emulated times. The host is an RTX 4080, which fails the
5-6-5 test, so 16-bit targets are drawn with 8 bits a channel. No
draw was declined and no read was stale in any of them.

| Game and scene | Pixels that differ | By more than 8 in 255 | Mean difference |
|---|---|---|---|
| Giants, first mission, 800x600, 32 bits | 16% (13% by 1 or 2) | 0.28% | 0.35 |
| Zanzarah, Fairy Garden, 640x480, 16 bits | 0.6% to 1.1% (by one step of the screen's, but for 0 to 6 pixels a frame) | 0.09% to 0.17% | 0.04 to 0.07 |
| Oni, training room, 640x480, 16 bits | 1.0% to 1.4% (by one step, but for 4 to 22 pixels a frame) | 0.25% to 0.35% | 0.08 to 0.11 |

A step of red or blue on a 16-bit screen is 8 or 9 in 255, so the
third column of the two 16-bit games counts single steps. The
pictures are right by eye in all three, and menus and loading screens
are identical.

Each difference was traced to its draws by recording a frame of each
game and drawing it through both renderers (`replay_a_capture`, see
"Testing"), with a state turned off on both sides to see what hangs
on it:

- **Giants.** The ground's normal map and its colour texture have an
  anisotropy of 8, which the host's GPU filters in its own way. With
  the anisotropy set to 1 on both sides 3.9% of the frame's pixels
  differ, 2.3% by more than 1 and 0.06% by more than 8. Nine in ten of
  those over 1 are the ground again: its diffuse colour, one of 255
  off, goes through `DOTPRODUCT3` and `MODULATE2X` and comes out up to
  8 off. Nearly all of the rest, and those over 8, are marks drawn on
  the ground with a depth bias of 2 steps of 24 bits, less than the
  depth bound: a mark's pixel is drawn by one renderer and not by the
  other.
- **Zanzarah.** With the blend turned off on both sides 0.05% to 0.1%
  of the pixels differ. The rest is the host's blend into a 16-bit
  target that it keeps in 8 bits a channel. The few pixels further
  off, by up to 198, belong to alpha-tested leaves.
- **Oni.** 97% of the pixels that differ by a step or more after their
  draw are drawn by three blended states, into a 16-bit target as in
  Zanzarah.

With the host's own filter on 2D textures in place of the shader's
sampling, the same frames differ on 27% of the pixels in Giants, 10%
to 15% in Zanzarah (1.5% to 2.4% by more than 8, up to 239) and 6% in
Oni (0.7% to 0.8% by more than 8): the host picks the mip level and
rounds the weights by itself, and each sample one of 255 off tips a
pixel of a dithered 16-bit target to the next step. The shader's
sampling costs these games no frames on that host: Giants draws 25.8
frames a second with it and 25.6 with the host's filter, Oni 20.6 and
20.0.

Read-backs for each frame the game shows, and what asks for them:

| Game | Read-backs a frame | What asks |
|---|---|---|
| Giants | 5 | the screen (1); a target a later draw samples (3: the sea's and the sky's 256x256 and 64x64 targets); `CLEAR` of a 64x64 target the host declined (1) |
| Zanzarah | 2.7 | `CLEAR` (1.4), a sampled target (0.5), the DirectDraw driver's own reads and writes (0.3), the screen (0.4) |
| Oni | 1 | the OpenGL front end's `SwapBuffers`, which copies the back buffer to the screen on the processor |

Step 4d did the first four of the list below, and `BLT` on the host.
Read-backs for each frame the game shows after it, in a window that
nothing reads the picture from:

| Game | Read-backs a frame | What asks |
|---|---|---|
| Giants | 0 | |
| Zanzarah | 0.4 | the DirectDraw driver's reads and writes of a surface the card drew into |
| Oni | 1 | `SwapBuffers`, as before |

Oni's `SwapBuffers` copies a 16-bit back buffer to a 32-bit desktop,
and `BLT` converts no formats, so the copy stays on the processor
(see "Frame rates against the target" for what is left).

What the first runs said to do, in the order of what they cost:

- **A target that is sampled.** The host reads the target back and
  decodes it again as a texture. A texture whose one level is a
  surface the host has, in a format it samples as it is, can be the
  host's copy itself.
- **`CLEAR` declined over a shared page.** `Pages` counts by 4 KiB
  page, and Giants' 64x64 targets lie 16 KiB apart off the page
  boundaries, so a processor write to one makes its neighbours'
  copies stale, the host declines their `CLEAR`, and the processor's
  `CLEAR` writes again. A `CLEAR` of a whole surface with every bit
  needs no current copy. Stamps by byte range would end the false
  sharing for draws too.
- **One depth buffer under targets of several sizes.** Giants draws
  its small targets with the screen's depth buffer, which the host
  holds as surfaces of different sizes over the same bytes, so each
  change of target drops and reads back the other.
- **The screen.** Step 4d's present from the host's texture.
- **`SwapBuffers` and the drivers' blits.** `BLT` on the host, and
  the OpenGL front end showing its back buffer with it.

It is built in these steps, each a pull request: the interface with
the tracking and a host that draws in software, to test the protocol
(4a); the OpenGL host with its targets, textures, generated shaders
and the agreement tests, and `CLEAR` on the host (4b); the display's
side, the configuration and the status (4c); the picture from the
host's texture, and clears, copies and render to texture kept on the
host (4d). The host options (a scale, multisampling) come after.

### Shader cache

Programs are cached by key in memory: the draw's steps without what is
the host's fixed state, and which stages the host's own filter
samples, so two draws that differ in the depth test, the blend, the
masks or a texture's sampling states share a program. Compiled program
binaries are to be stored on disk by a hash of the generated source
(`glGetProgramBinary`, core since OpenGL 4.1, with a fallback to
compiling on hosts without it), so a game's second run compiles
nothing. A game of this period sets tens to a few hundred distinct
state combinations. The store on disk isn't built yet: it waits for a
game on the host renderer to show what the first run's compiles cost.

If a first run still shows pauses in games, the answer is a general
shader that reads the lowered operation list from uniforms and draws
until the specialised one is compiled, which is what DXVK 3.0 and
Dolphin do. The operation list makes that shader an interpreter of a
small instruction set. It is left out until a measurement asks for it.

### Save states and determinism

A saved state has whatever is newer on the GPU read back into its copy
of card memory ("Between the card and the host renderer"), and `impl
State` for the card saves the registers, the state the commands set (a
fixed-size block: render and stage states, matrices, lights, material,
clip planes, the texture and palette tables), the mode shown, a
pending flip, the position in the frame and the ring. Loading rebuilds
the host caches from card memory, as the Voodoo's `Resync` does. The ring
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
- **The queue.** The words handed over that way wait in a queue and
  run together, in their order. A game's draws are small, and one by
  itself is a few rows on one thread; a queue of them goes to the
  renderer's threads at once. The queue runs before anything can tell
  it was there: before a write to the card's registers or ring, before
  the program or the emulator writes card memory, before the emulator
  reads it (`win32::screen`), with a flip or a mode set, and before a
  call that lets the program read it or shows a window
  (`screen::api_settles`: all of DirectDraw and Direct3D 8 but the
  Direct3D devices' methods and those of what they draw from, with a
  scene's end and Direct3D 8's present among those that settle; GDI;
  USER; WGL; and OpenGL's pixel transfers). It also runs when it is
  full, and 5 ms of emulated time after its first command, so a
  program that reads video memory it kept a pointer to sees the picture
  a frame's fraction late at worst, as it would beside a real card that
  draws behind the driver's back. The queue is part of the card's saved
  state. A guest's driver, with its ring, isn't queued.
- **Direct3D 7.** `ddraw/d3d/hal` is the HAL interface (capabilities,
  formats, state, draws, clears), with the Voodoo's implementation
  (`hal/voodoo.rs`) and the ToadForce's (`hal/toadforce.rs`) behind it.
  The ToadForce HAL lists the level's formats and turns state and draws
  into card commands. A draw of vertices that aren't in screen space
  goes to the card with the device's matrices, lights and material, and
  the card transforms and lights it; the pipeline keeps blended
  vertices and the draws whose clip status a program reads back.
  Direct3D 7 lists the T&L HAL device beside the HAL one on the card,
  and a program draws on either the same way: Giants picks the T&L HAL
  and hands the card untransformed vertices, where on the HAL device
  it has `ProcessVertices` transform them. The 16-bit shadows of 32-bit
  targets (`d3d/shadow.rs`) aren't used on the card. A texture surface
  in video memory in one of the card's formats is sampled where it is.
  The HAL copies into card memory only what the card can't read there:
  textures in system memory, palettized ones, and sizes that are no
  power of two.
- **Direct3D 8** (`d3d8/`) sits on the Direct3D 7 device today and
  keeps doing so, with `D3DCAPS8` filled from the level's table and
  hardware vertex processing offered for fixed-function draws.
- **OpenGL** (`opengl32/`) is 1.1 on the Direct3D 7 device, and so draws
  on this card through the Direct3D HAL, with the pixel formats and
  extensions it has on a Voodoo3 (Oni plays on it). It grows to 1.3 with
  the level's extensions, sending the card states OpenGL needs that
  Direct3D has no state for. Its OpenGL front end (contexts, state,
  immediate mode, arrays, display lists, pixel transfer) is kept apart
  from its WGL and Win32 parts and takes its calls through one interface,
  so that a booted Windows can later feed the same code (see below).
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
- **A game's draws through both renderers.** `POST
  /api/toadforce/capture` records what the card is given
  (`toadforce::capture`), and the ignored test `replay_a_capture`
  draws the recording in software and through the host
  (`toadforce::gl::replay`): the differences of each target, and draw
  by draw, with the host given the software's pixels before each
  draw, which states the pixels that differ belong to
  (`docs/llm-debugging.md`).
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
   game fix (#583, #606), and Grom past `screen.Init` (#645). Done:
   the state, surface and draw commands are in `src/toadforce/level1`,
   the renderer in `src/toadforce/raster`, and the HAL in
   `src/win32/ddraw/d3d/hal/toadforce.rs`. Machines offer level 1,
   which stays provisional until it is frozen.
3. **Transform and lighting in the card.** Untransformed draws, lights,
   vertex blending, clip planes, texture coordinate generation, the T&L
   HAL device. Proof: Giants' terrain with bump mapping. Black & White
   joins the proof once #889 lets it run on the built-in Windows, or
   with the Windows 98 driver of phase 6. In the card: a draw of
   vertices that aren't in screen space goes through
   `raster/geometry.rs` (transform, texture coordinates, fog),
   `raster/light.rs` and `raster/clip.rs`, as "Transform and lighting"
   in the specification's chapter 9 defines.
   Done but for two parts: Direct3D 7's untransformed draws go to the
   card, the T&L HAL device is listed, the pipeline makes texture
   coordinates as the card does for the draws it keeps, and the HAL
   has the card's four textures. DirectDraw makes cube maps on the
   card, which Direct3D 7 samples and draws into. Giants makes the
   T&L HAL device and turns on object and landscape bump mapping and
   environment mapping, for which it draws the sky and the sea into
   the six faces of a 256-texel cube map; nothing at the first
   mission's start samples it yet. Blended vertices reach the
   card with Direct3D 8 in phase 5 or a Direct3D 7 game that sets the
   state (#959).
4. **The OpenGL renderer.** Generated shaders, caches, GPU-owned
   targets, the disk cache. Proof: renderer agreement tests, and the
   phase 2 and 3 games at their full frame rate on it, measured against
   the software renderer on a four-core host as well as a larger one.
5. **Direct3D 8 and OpenGL 1.3 on the card**, with the OpenGL front end
   separated from WGL. Proof: Max Payne, MDK2, Oni. Started: Direct3D 8
   makes its device on the card, with hardware or mixed vertex
   processing, the HAL's texture formats and a D24S8 depth buffer, and
   a DirectDraw z-buffer keeps its stencil bits. The four games draw
   their first levels on the card (see "Phase 5 on the card"). Left:
   OpenGL 1.3's extensions, cube textures in Direct3D 8, and the OpenGL
   front end's vertices transformed and lit by the card.
6. **Performance.** Optimisation passes over the working
   implementation of phases 1 to 5. The target: every game of the
   primary list (the eight of phase 0 and Nocturne) holds a stable 30
   frames a second, with some to spare, measured on a four-core host
   as well as a larger one.

Postponed, with no place in the order (decided 2026-11-12):

- **Level 2**, until work starts on a game that needs shaders. None
  of the nine games of the primary list does.
- **The Windows 98 driver** (display minidriver, then the DirectDraw
  and Direct3D HAL, then OpenGL): the card serves the built-in
  Windows only for now. Black & White, which the driver was to
  unblock, then waits for #889 alone.

### Phase 5 on the card (2026-11-12)

The four games of phase 5 on `svga_toadforce`, on the built-in
Windows, each from its launcher to its first level. "Before" is the
branch of phase 4. The frame rates are rough: other emulators ran
beside each run.

| Game | Before | Now, with the OpenGL renderer |
|---|---|---|
| Max Payne (Direct3D 8) | No adapter: `GetDeviceCaps` fails with `D3DERR_NOTAVAILABLE` | The tutorial level with "D3D Hardware T&L": every draw transformed by the card, no read-back, about 30 frames a second |
| No One Lives Forever (Direct3D 7) | Menus without their backgrounds and text, the first cutscene a blue band | The menus and the first mission's cutscenes; no frame rate taken |
| MDK2 (OpenGL) | The first level (run with the launcher's T&L box clear) | The same, with the box clear and ticked; about 28 frames a second ticked |
| Oni (OpenGL) | The training room | The same |

No draw was declined by the host renderer in any of them, and the card
rejected no command.

What the games asked of the card, where it differs from the Voodoo3's
traces of phase 0:

- **Max Payne** lists the adapter and offers "D3D Software T&L" and "D3D
  Hardware T&L". The second makes a device with
  `D3DCREATE_MIXED_VERTEXPROCESSING`. It checks A8R8G8B8, V8U8, L6V5U5
  and DXT1, 3 and 5 as textures and D24S8 and D24X8 as depth formats,
  takes D24S8, and clears colour, depth and stencil in one `Clear`.
  Formats it checks and the card has none of: R8G8B8, Q8W8V8U8,
  W11V11U10 and D32. With software vertex processing it draws its
  characters with the blended vertex formats 0x1118 and 0x1116, which
  the layer blends on the processor as on the Voodoo3 (3% of the draws).
  With mixed processing it set neither in the tutorial, and the card
  transformed every draw.
- **No One Lives Forever** takes the 32-bit z-buffer format with 8
  bits of stencil from `EnumZBufferFormats` and clears depth and
  stencil together each frame. DirectDraw dropped the stencil bits
  of the format, so every such `Clear` failed and cleared nothing.
  The `BltFast` of compressed surfaces that failed on the Voodoo3
  doesn't come up: the textures are in the card's own formats. The
  driver reads the target 1.7 times a frame in the menus and 0.4
  times over the whole run.
- **MDK2** with the T&L box ticked lights through OpenGL
  (`glLightfv`, `glMaterialfv`, `glColorMaterial` with `GL_EMISSION`,
  `glNormal3sv`). The front end lights and transforms on the
  processor, so the card sees screen vertices: 160 draws a frame of
  3.5 primitives each. Its attract-mode demo shows the gauges alone
  with the box ticked, on the Voodoo3 as well; not looked into.
- **Oni** is as in phase 4.

What is left of phase 5, with a guess of its size:

- **OpenGL 1.3 on the card** (large). The front end reports 1.1 with
  the Voodoo3 ICD's four extensions. The card has what 1.3 adds
  (compressed and cube textures, the combine, add and dot3
  environments, the border colour, four units), and none of the four
  games asks for it. To write: the entry points and the lowering of
  each, and the version and extension strings by card.
- **The OpenGL front end's vertices on the card** (large, and what
  the separation from WGL is for). The front end runs Direct3D's
  pipeline on the processor for every `glEnd`, and puts its whole
  state on a Direct3D device before each. With the matrices, lights
  and materials sent as level 1 state, the card transforms and lights
  as it does for Direct3D, and equal state is sent once. This is the
  first item of phase 6 for Oni too.
- **`SwapBuffers` without the read-back** (medium): one read of the
  target a frame in MDK2 and Oni. A `BLT` or `FLIP` of the context's
  back buffer on the card. Oni's `glReadPixels` belongs here too.
- **Keyed copies the host leaves to the processor** (small, #981): a
  keyed copy with more than 4096 runs, or one whose source is ahead
  on the host, is copied by the processor after a read-back. A draw
  that discards the key would take both.

The second step of phase 5 added:

- **Cube textures in Direct3D 8**: `CreateCubeTexture` and
  `IDirect3DCubeTexture8` over DirectDraw's cube maps, with
  `CheckDeviceFormat` answering for cube textures and the caps bits
  set on a card that samples them.
- **Blended vertices on the card** (#959): a draw from Direct3D 7 or
  8 that blends by weights without indices goes to the card, which
  weighs the position and the normal through its world matrices.
  Direct3D 7 gained `D3DRENDERSTATE_VERTEXBLEND`, which it lacked on
  every card. Indexed blending and tweening stay on the processor:
  the card has neither.
- **Copies from system memory**: the driver writes the source
  rectangle into card memory and the card copies it with `BLT`, so
  the host needn't read the target back for the processor to copy
  into it.
- **Keyed copies on the host**: the host reads the source from card
  memory and writes the runs of pixels the key lets through, up to
  4096 runs a copy. A copy with more goes back to the card.

No One Lives Forever needed the last two. Its menus drew a
full-screen `Blt` from a system-memory surface each frame and keyed
blits for the cursor and the marks, and each made the host read the
target back: 9324 read-backs from the menus through the first
cutscene into the first mission, 3 with these (one each for the
driver, a lock and a shown frame). It now plays its first mission. It
takes the plain HAL device and transforms by itself: about 1900
`DrawPrimitive` fans of screen vertices a frame, which the host
draws in batches of equal state. The emulated processor never halts
there, so its frame rate is the game's own work on the processor.
Zanzarah's driver read-backs on its route to 160 s went from 1208 to
none, with the same pictures.

### What the phase 5 traces showed

Traces of the HAL's commands in each game, taken to find what the
card lacks:

- **Giants** makes the T&L HAL device at 800x600 in X8R8G8B8 with a
  32-bit z-buffer with stencil. It uses `DOTPRODUCT3`, `MODULATE4X`
  and `ADDSMOOTH`, texture coordinates from the camera-space position
  and from the reflection vector, three directional lights, and
  render targets of 256 texels and cube maps.
- **Zanzarah** makes the T&L HAL device and copies about 34 keyed
  rectangles a frame from system memory to the back buffer, the case
  the copies above cover.
- **Grom** runs to 300 s without input with every draw transformed
  by the card. Its trace hasn't been read.
- **Max Payne** blends no vertex in the tutorial, lights with up to
  6 point and spot lights, and uses two stages.
- **MDK2** uses lights 0 to 5 and `glNormal3sv`, in immediate mode
  only.
- **Oni** calls `glReadPixels` 3501 times in a run where the driver
  read the target 2957 times, so not all of those reads come from
  `SwapBuffers`. Which do is open.

None of them needs level 2.

### Nocturne on the card (2026-11-12)

Nocturne (Terminal Reality, 1999) runs from its two discs: its own
installer under the built-in Windows, then `NOCTURNE.EXE` with disc 1
in the drive. On `svga_toadforce` it reaches play in the first room of
Volume 1 with both of the card's renderers.

- **Finding the card.** The game ships a renderer DLL for each API
  (`TRID3D` for DirectX 5, `TRIDX6`, `TRIDX7`, `TRI3DFX` for Glide)
  and starts on its software renderer; Options > Graphic options turns
  3D hardware on. Each DLL lists a DirectDraw device only when
  `DDCAPS_3D` is set in the driver's caps, which a ToadForce's lacked
  (#982, fixed with this entry). With it the screen offers DirectX 5, 6
  and 7 on "ToadForce", and no Glide. DirectX 7 is the one to use. On a
  Voodoo3 the game's card table picks Glide and 16 bits instead.
- **What it asks.** The DirectX 7 renderer makes `IDirect3D3`'s HAL
  device on a 640x480 X8R8G8B8 flipping chain with a Z32 z-buffer (the
  card lists Z16, Z32S8 and Z32), and A8R8G8B8 textures of 64 to 256
  texels with mip levels, loaded by `Blt` from system memory. It
  transforms and lights by itself: every draw is
  `DrawIndexedPrimitive` of `XYZRHW | DIFFUSE | SPECULAR | TEX1`
  triangle lists, about 44 a frame with 1400 triangles, one stage of
  `MODULATE`, linear filters with a linear mip filter, vertex fog in
  the specular colour's alpha, and for the lit effects `SRCALPHA` and
  `ONE` blending with an alpha test.
- **On the host.** No draw is declined and the card rejects no
  command. Two things a frame make the OpenGL renderer read back
  (#983): the game locks the whole back buffer, twice, and copies a
  z-buffer it keeps into the device's with `Blt`.
- **Frame rate, rough** (the card's flips over 10 seconds, with other
  work on the machine): about 20 a second with OpenGL and 25 in
  software in that room, and about 18 on a Voodoo3 through Glide. The
  read-backs of #983 are what puts OpenGL behind software here.
- **Outside the card.** The opening movie doesn't play and the game
  waits on a black screen until Escape (#980): MCI's `play` lacks the
  `window` and `fullscreen` flags of a video device.

### Frame rates against the target

Measured in real time with `cycles=max`, alone on the development
machine, sound on a muted device. Each is one run of 20 seconds, so a
rough figure.

| Game and scene | ToadForce, software | ToadForce, OpenGL | Voodoo3, software | Voodoo3, OpenGL |
|---|---|---|---|---|
| Giants, first mission, 800x600 (2026-11-12, after phase 3) | 14 | | 21 | 17 |
| Oni, training room, 640x480 (2026-11-11, with #944) | 15 | | 23 | 29 |
| Giants, the same (2026-11-12, step 4c) | 15 | 25 | | |
| Oni, the same (2026-11-12, step 4c) | 15 | 20 | | |
| Giants, the same (2026-11-12, step 4d, 4c measured again beside it: 23) | | 28 | | |
| Oni, the same (2026-11-12, step 4d, 4c beside it: 18) | | 19.5 | | |
| Nocturne, first room of Volume 1, 640x480, 32 bits | not measured | not measured | | |

With OpenGL at step 4c the emulator's thread is 86% busy in Giants
and 82% in Oni, as it was in software: it still transforms and clips
every vertex, hands the host a draw call for each of the game's
(Oni's are a few triangles each, 1.65 million of them to the training
room), and waits for the GPU at each read-back listed under "What the
first games showed".

Step 4d took Giants' five read-backs a frame away, which were 18% of
the thread's busy time. Where the thread's busy time goes after it
(`/api/profile`, zones in `docs/profiling.md`):

| Zone | Giants | Oni |
|---|---|---|
| `cpu.jit`, the game's code | 33% | 29% |
| `hle.win32`, the built-in Windows outside the zones below | 27% | 23% |
| `toadforce`, the card: decoding, transform, lighting, clipping | 20% | 4% |
| `toadforce.host`, the OpenGL renderer's calls | 10% | 3% |
| `toadforce.readback` | 0% | 5% |
| `hle.d3d.bind`, the card's state brought up to the device's | 1% | 16% |
| `hle.d3d.draw` and `hle.opengl` | 1% | 8% |

What is left for phase 6, largest first, with what each is worth:

- **Oni: the Direct3D layer's work for each `glEnd`** (`hle.d3d.bind`,
  `hle.d3d.draw`, `hle.opengl`: 24% of the busy time, for 97 draws a
  frame of a few triangles each). The OpenGL front end sets its
  whole state on its Direct3D device before every draw, and the HAL
  compares all of it again. Draws with equal state gathered in the
  front end into one draw would take most of it.
- **Giants: transform and lighting on the processor** (`toadforce`,
  20%). Moving the vertex work to the GPU, which the interface
  between the card and the host was kept open for.
- **`hle.win32` outside Direct3D** (23% to 27%): not looked into yet.
  A profile by API call comes first.
- **The host's own calls** (`toadforce.host`, 3% to 10%): state and
  uniforms are set for every run, 290 a frame in Giants. Skipping
  what didn't change is worth a part of the 10%.
- **Oni's `SwapBuffers`** (5%): a back buffer in the desktop's format
  when the desktop has 32 bits, as a real driver gives a windowed
  context, with `BLT` to the window. It changes Oni's pictures, as
  the target then isn't dithered.
- **Zanzarah's driver reads** (0.4 read-backs a frame): which
  DirectDraw calls ask, and whether a `BLT` with a colour key on
  the host covers them.

Giants stays under 30 on every card, the Voodoo3 drawn by the host's
GPU included, with the emulated machine at full speed and the
emulator's thread 84% to 98% busy. What limits it is the work on that
thread: the game's own code and the Direct3D layer's. A faster
renderer alone doesn't bring it to the target, so phase 6 starts with
a profile of that thread. Oni gains from the renderer: the Voodoo3 is
at 29 with OpenGL and at 23 without.

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
- What Black & White asks of any card, once it runs (#889).
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
