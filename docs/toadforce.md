# ToadForce: a synthetic 3D card for games of 2000 to 2002

This document says why the card exists, how it is defined, how the
emulator draws it and what the guest software does with it. What a guest
sees of the card is specified in `toadforce-spec.md`. Facts taken from
other projects have their source in "References".

## The problem

The Voodoo3, the newest real 3D card rust-dos emulates, has the real
card's limits: 16-bit colour and depth, 256x256 textures, two texture
units, no compressed textures, no hardware transform and lighting, and a
short list of texture stage operations. Games from 2000 on were written
for the GeForce 256, GeForce2, Radeon and their successors, and many
either require what those cards added or take untested paths without
it: Zanzarah reads its DDS textures only when a DXTn format is listed,
Grom asserts when no texture format has 8 bits a channel, and Giants:
Citizen Kabuto draws its terrain wrong without bump mapping.

Emulating a real card of that class at register level is not practical
today. 86Box has no merged NV3, TNT or GeForce device. envytools' prose
for the NV10 and NV20 3D engines is a stub of a few hundred bytes each;
what exists is `rnndb`'s method names and bit fields, which name the
registers and say nothing of the pipeline's arithmetic. xemu's NV2A is
the closest working model of a GeForce-class pipeline, and it is driven
by the Xbox's own software, with no Windows driver in the picture. A
register-level GeForce would also have to run NVIDIA's Windows 98
driver, which uses every corner of the chip.

## The card in short

ToadForce is a PCI display adapter that never existed. rust-dos defines
it: its PCI identity, registers, memory layout, command set,
capabilities, and the arithmetic of its pixel pipeline. Its feature set
is that of the cards of 2000 to 2002, taken as a union: hardware
transform and lighting, four textures at once, DXT1 to DXT5, 32-bit
colour, 24-bit depth with stencil, cube maps, and every Direct3D 7 and 8
texture stage operation, including the three bump mapping ones.

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
   same way as on that card.
3. Games are played on the host's GPU. The OpenGL renderer draws the
   commands with shaders generated from the pipeline state, as wined3d,
   DXVK and xemu do, and the card is shaped so that the GPU can do the
   transform, the lighting and the pixels with nothing read back in a
   frame. A software renderer defines what the card draws, in integer
   arithmetic where the pixel pipeline allows it, and the OpenGL
   renderer is tested against it within stated tolerances. It also draws
   where there is no GPU to use, slowly.
4. The capabilities are a fixed table for each feature level. A level
   never changes once released. New features make a new level, which the
   guest's driver has to ask for.
5. The built-in Windows drives the card. The command set is shaped so
   that a driver for a booted Windows 98 is a thin translation of
   Direct3D's driver interface (see "A booted Windows 98").

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
memory (see "Memory (BAR 1)").

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
Dolphin made the same move in 2017. Taken: the per-key generator,
because it is simpler to test against a reference.

**Dolphin.** Moved its combiner emulation from floating point to integer
shader arithmetic in 2014, because float results differed from the
console and between drivers. Taken: integer arithmetic for the stage
cascade, in the reference and in the generated shaders, so the two give
the same values from the same inputs.

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
5. **A released level is frozen.** A test holds each released level's
   capability table and command list, and a change to one fails it.
   Level 0 is released. Level 1 is provisional while the emulator's own
   drivers are its only users.
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
   - The work can go to the GPU whole: untransformed vertices, lights
     and stage state, with buffers and textures uploaded once and found
     again by their place in card memory. The pixels do. The vertex work
     is the card's on the processor.
   - A command is complete for the guest when `RING_PUT` returns, and
     the GPU need not have drawn it by then. The ownership tracking
     makes up the difference, reading pixels back only when the guest
     reads them.

   The software renderer is the definition, the test reference and the
   fallback. It is several times slower than a GPU, and too slow for
   some games at their usual resolutions. Speed work on it comes
   after the OpenGL renderer's, and no feature of the card is held back
   or bent to make it faster.

## The card

`docs/toadforce-spec.md` defines what this chapter describes, as a guest
sees it: field by field, with reset values, error codes and the
arguments of each command. It is the one to follow where the two differ.

### PCI identity and resources

One PCI function, class 03h/00h (VGA-compatible display controller), in
the slot the other display adapters use (`S3_DEVICE` in `src/pci.rs`,
with `Target::ToadForce`).

The vendor ID is 1234h, which Bochs' and QEMU's VGA uses (as
1234h:1111h), and the device ID is 7F01h. The subsystem vendor ID is
1234h and the subsystem ID 5446h ("TF"). Windows matches a driver on
vendor and device together, so a driver for Bochs' VGA doesn't claim
this card. The PCI ID database (pci-ids.ucw.cz) lists no vendor 1234h
and no device 7F01h under any vendor, so no registered device has the
pair. It isn't registered to the project either, so a driver also reads
the `ID` register and uses the card only when it holds "TOAD". Buying a
vendor ID from PCI-SIG, or using a range inside another vendor's ID, was
considered and dropped: the first costs a yearly membership, and the
second takes space set aside for someone else's devices.

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
| 024h | `ERROR_AT` | R | ring offset of that command, FFFFFFFFh for a register write, FFFFFFFEh for a command the emulator's drivers handed over directly |
| 028h | `MEMORY` | R | size of card memory in bytes |
| 02Ch | `RESET` | W | a write with bit 0 set resets registers and command state |
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

`STATUS`, `IRQ_STATUS`, `MODE_START`, `FRAME` and `SCANLINE` follow the
emulated clock, so they read the same in two deterministic runs. There
is no busy bit: the card has finished every command by the time the
write to `RING_PUT` returns (see "Command ring (BAR 2)").

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

The emulator implements this as it does for the Voodoo3
(`src/voodoo/tracker.rs`), generalised: each surface is current either
in card memory or on the host's GPU, a guest read of one that is newer
on the GPU is answered from the GPU's pixels, and each 4 KB page has a
write counter (`gpu::Pages`) that the host's copies of textures and
surfaces compare (see "Between the card and the host renderer").

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

The built-in Windows' drivers hand the card the same words without the
ring, through `Bus::toadforce_submit`, which saves a write through the
bus for every word. The commands wait in a queue and run together
before the next write to the card's registers, ring or memory, when the
queue is full, or `QUEUE_NS` (5 ms of emulated time) after the first of
them ("The queue" in "The built-in Windows" lists the other points).
The queue runs through the card's decoder (`ToadForce::submit`), and a
command rejected there sets `ERROR_AT` to FFFFFFFEh. A test feeds the
same command stream through the ring and through `submit` and compares
the results.

### Commands of level 1

Level 1 has level 0's commands (`NOP`, `FILL`, `BLT` and `FLIP`, see
"Level 0: display and 2D") and the ones below.

State:

| Command | Arguments |
|---|---|
| `SET_RENDER_STATE` | pairs of (state, value) |
| `SET_STAGE_STATE` | stage, pairs of (state, value) |
| `SET_TRANSFORM` | which (world 0 to 3, view, projection, texture 0 to 7), 16 floats |
| `SET_VIEWPORT` | x, y, width, height, min z, max z |
| `SET_SCISSOR` | enable, rectangle |
| `SET_MATERIAL` | diffuse, ambient, specular, emissive, power |
| `SET_LIGHT` | slot 0 to 7, type, colours, position, direction, range, falloff, attenuation, cone, spot model |
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
| `DRAW_BUFFER` | primitive type, vertex format, stride, offset of the first vertex, count |
| `DRAW_BUFFER_INDEXED` | the same with an index buffer offset or indices inline, and an index count |

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

### Level 0: display and 2D

Level 0 is what the card has after reset, with `LEVEL` at 0: the mode
registers and four commands, `NOP`, `FILL`, `BLT` and `FLIP`, on
surfaces of 8, 16 and 32 bits a pixel up to 4096 a side. Its capability
table has five entries: that size, the formats `FILL`, `BLT` and a mode
take, and the range of `MODE_REFRESH`. The built-in Windows' GDI and
DirectDraw run on it. It is released and frozen: `caps.rs` and
`command.rs` hold copies of its table and its commands.

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
  2001 reported, by our recollection of the GeForce3. No capability dump
  was found to confirm these numbers, or the counts of lights, clip
  planes and anisotropy.
  A game that loops over the count, or sizes a shadow or light map from
  the size, gets a value it was tested with. Eight textures at once
  would cost nothing to draw, and would save a game a pass that a
  current GPU doesn't notice. Cards reported eight only from late 2002,
  by our recollection, so most games of the period never ran with that
  value, and some engines size fixed arrays from the largest one their
  developers saw.
- There is no W buffer. Games used one against z-fighting in 16-bit
  depth, and only on cards that reported it; the GeForce cards, by our
  recollection, did not, so the z-buffer path is the one games were
  tested on most, and 24-bit depth removes most of the reason for it.
  Giants: Citizen Kabuto, which takes the Voodoo3's W buffer, plays on
  plain Z without one. A frozen level could never drop it, so the level
  leaves out what its games don't need.
- The stage operations go beyond any one real card on purpose. A
  GeForce2 had `DOTPRODUCT3` and, by the evidence found, no `BUMPENVMAP`
  (NVIDIA lists `NV_texture_shader` for the GeForce3 family only); the
  Matrox G400 and the Radeon had `BUMPENVMAP`. With generated shaders
  each operation costs a few lines, and a card that accepts all of them
  never sends a game down a fallback path.
- The texture formats are those games of the period ask for: 32-bit
  with alpha, the 8-bit ones, and DXT1 to DXT5. A game that also asks
  for one the level lacks (Max Payne asks about R8G8B8, Q8W8V8U8,
  W11V11U10 and D32) takes one it has.
- Hardware T&L with 8 lights is what Direct3D 7's T&L HAL devices
  offered. Some games turn features on only for a T&L device: Giants:
  Citizen Kabuto makes the T&L HAL device and draws its bump mapping
  there with `DOTPRODUCT3`.
- Vertex blending is by weights, without indices. With the capabilities
  reporting no indexed blending, Direct3D 8 does it and tweening on the
  processor, as it did on the cards of 2001; Max Payne draws its
  characters that way, with 28 world matrices.

The Direct3D devices offered on this card are the HAL and the T&L HAL.
The T&L HAL's description has `D3DDEVCAPS_HWTRANSFORMANDLIGHT` and the
card's 8 lights, and the two draw the same way. On a GeForce, Direct3D 7
transformed and lit the plain HAL device's vertices on the processor and
the card drew XYZRHW vertices. Here the card transforms and lights the
untransformed vertices of either device, and the pipeline
(`src/win32/ddraw/d3d/pipeline.rs`) those of the draws the card can't.
The two are separate code (see "Vertices").

For OpenGL the level is shaped as version 1.3 with these extensions,
each of which NVIDIA's table lists for both the GeForce 256/2 and the
GeForce3 families: `ARB_multitexture`, `ARB_texture_compression`,
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
card took. The card has the state behind each of them. The built-in
OPENGL32 offers OpenGL 1.1 on it with fewer extensions (see "The built-in
Windows").

## Limitations

What the card, its drivers in the built-in Windows and its renderers
don't do, by what a program or a user notices. An entry that another
section covers names that section.

### Direct3D

- **Shaders.** The card has no vertex or pixel shaders. Direct3D 8 runs
  vertex shaders 1.1 on the processor (`src/win32/d3d8/shader.rs`), as
  it did on a GeForce2, on a device made with software or mixed vertex
  processing. The adapter's capabilities and a device with hardware
  vertex processing alone report none. `CreatePixelShader` fails.
- **Points.** Direct3D draws points one pixel wide (Direct3D 8's
  `MaxPointSize` is 1.0): the HAL leaves the card's point size states
  at their reset values, so the point sizes, point sprites and distance
  scaling of the level 1 table aren't reached through Direct3D.
- **Direct3D 8.** `CreateVolumeTexture` fails, and the only
  multisample type is none. Vertices blended by index and tweened ones
  are worked out on the processor, on a device with software vertex
  processing.
- **W buffer.** The card has none. A program that turns one on
  (`D3DZB_USEW`) draws with the z-buffer.
- **Gamma ramp.** The card has none. DirectDraw's gamma control and
  Direct3D 8's `SetGammaRamp` keep the ramp and give it back, and the
  picture stays as it was: a brightness slider or a fade that a game
  makes with the ramp shows nothing.
- **Render targets and depth buffers.** Direct3D draws into 5-6-5 and
  32-bit surfaces in video memory, and so not in a 256-colour mode or
  into a 5-5-5 surface. A depth buffer has 16 bits, or 24 in a 32-bit
  pixel with or without 8 bits of stencil: none has 16 bits and a
  stencil, or 3 bytes a pixel. Direct3D 7's device descriptions list
  depths of 16 and 32 bits, without `DDBD_24`.
- **Textures.** A compressed texture asked for in DirectDraw's video
  memory is refused with `DDERR_NOTEXTUREHW`; one made in system memory
  or without a memory flag draws. A cube map made without all six faces
  draws as no texture. Direct3D 7's device descriptions report four
  sets of texture coordinates in `dwFVFCaps`, and the card takes eight.

### OpenGL

- **Version and extensions.** The built-in OPENGL32 reports version
  1.1.0 on the card, with `GL_ARB_multitexture`, `GL_EXT_bgra`,
  `GL_EXT_compiled_vertex_array`, `GL_EXT_packed_pixels`,
  `GL_EXT_texture_env_add` and `WGL_EXT_swap_control`. It draws with two
  texture units and 2D textures up to 2048 a side, and takes OpenGL
  1.2's packed pixel types, which its version and extensions don't
  name. Compressed textures, cube maps, the combine and dot product
  texture environments and anisotropic filtering aren't reached through
  it.
- **Buffers.** A pixel format has the desktop's 16 or 32 bits of colour
  and a 16-bit depth buffer, and no stencil, alpha or accumulation
  buffer. `ChoosePixelFormat` finds no format for a request with alpha
  bits. The stencil calls do nothing and the stencil test passes for
  every pixel, so what a program masks with the stencil buffer is drawn
  unmasked.
- **Functions that do nothing.** Display lists, `glDrawPixels`,
  `glBitmap` and the raster position, `glCopyPixels`, texture
  coordinate generation (`glTexGen`), user clip planes, 1D textures,
  stipples, `glLogicOp`, colour index mode, the accumulation buffer,
  evaluators, selection and feedback log their first call and do
  nothing else. The calls between `glNewList` and `glEndList` run at
  once, and `glCallList` draws nothing.
- **Reading back.** `glReadPixels` of `GL_DEPTH_COMPONENT` and
  `glGetTexImage` leave the program's buffer as it was. A proxy texture
  is refused with `GL_INVALID_ENUM`, and `glGetTexLevelParameter`
  answers for the bound texture whatever the target.

### Where the card runs

- **The built-in Windows only.** The drivers are the emulator's own
  DLLs. A booted Windows 95 or 98 has no driver for the card and sees a
  VESA card without 3D, as DOS programs do ("A booted Windows 98"). A
  game on a Windows image needs a display adapter that has a driver
  there, such as `svga_voodoo3`.
- **Glide.** The card has none. A game that draws with Glide is
  configured with a Voodoo card.
- **GDI.** GDI's drawing on the desktop writes card memory on the
  processor, and so do DirectDraw's stretched and mirrored blits and
  those with a destination colour key ("The built-in Windows").

### Where the picture can differ

- **Between the two renderers.** The OpenGL renderer's pixels are those
  of the software renderer, which are the card's, within the bounds of
  "Where the OpenGL renderer may differ". Anisotropic filtering differs
  the most. `toadforce_renderer=software` gives the card's own pixels
  on every host.
- **No picture options.** The OpenGL renderer draws at the card's
  resolution, without multisampling, with the anisotropy the game sets.
  There is no counterpart of `voodoo_scale` or `voodoo_msaa`.
- **A batch the host can't draw.** When the host has no complete
  framebuffer for a target, the batch being drawn is lost, and the
  software renderer draws from the next one on. Only a host whose
  driver lacks a format OpenGL 3.2 requires gets there.
- **Direct3D's rules.** On a 16-bit target the card dithers before the
  blend ("After the cascade"), so blended layers band slightly more
  than on a card that dithers after it. Direct3D's two cubic
  magnification filters are the linear one. Vertex fog goes by the
  camera-space z under any projection, and the reflection vector and
  the sphere map take the direction from the eye to the vertex whatever
  `LOCALVIEWER` says; neither is checked against a real Direct3D.

### What costs speed

- **Vertex work on the processor**, with both renderers. The card
  transforms, lights, clips and culls every vertex on the emulator's
  thread, and works out a draw's setup (matrix products, lights in
  camera space) again for every draw. The OpenGL front end transforms
  and lights its vertices on the processor too.
- **The software renderer** is too slow for some games at their usual
  resolutions ("The software renderer"). It draws wherever the OpenGL
  renderer doesn't: a host without desktop OpenGL 3.2, a window drawn
  by SDL's renderer, the web build, and a run with `--deterministic`.
- **Declined states.** A draw in a state the OpenGL renderer declines
  is drawn in software, with its target and depth buffer read back
  before and uploaded again after ("The OpenGL renderer" lists the
  states).
- **Read-backs.** With the OpenGL renderer, card memory gets a
  surface's pixels from the GPU when the processor needs them: for a
  stretched, mirrored or destination-keyed blit of a surface the host
  drew, a z-buffer filled with DirectDraw's `Blt` (`DDBLT_DEPTHFILL`),
  a keyed copy of more than 4096 runs of pixels or from a source the
  host drew, the picture of a 256-colour mode or of a mode shown
  doubled, once a frame, and a program's polls of port 3DAh between
  the host's draws into the surface shown. `/api/status` counts them
  by cause.
- **Programs compiled in the frame.** The OpenGL renderer compiles a
  state's program on the emulator's thread when the state is first
  drawn, and keeps it in memory only, so a game pauses briefly at each
  new state combination, in every run ("Shader cache").
- **Copies made again.** The host knows that card memory changed by
  4 KiB page, so a write to one small surface makes the host's copies
  of the others on its page stale, and they are uploaded again. The
  Direct3D HAL converts a palettized texture to 8-8-8-8, and again
  whenever its palette changes.

## Defined behaviour

The software renderer is the definition. This section says what it
computes, so that the definition is written down apart from the code,
and where the OpenGL renderer may differ.

### Vertices

The card's own code defines transform, lighting, fog factors, texture
coordinate generation and clipping: `raster::geometry`, `raster::light`
and `raster::math`, on the processor for both renderers. It computes in
IEEE single precision, with these exceptions, so that its results are
the same on every host:

- cosines, e to a power and the powers of specular highlights and spot
  falloffs come from its own series in the four exactly rounded
  operations (`math.rs`), and not from the host's C library;
- a triangle's winding (`geometry::clockwise`) and a depth's value in
  the buffer (`level1::depth_value`) are worked out in double
  precision;
- a vertex blended between world matrices is blended in camera space,
  each matrix's world and view product applied and the results
  weighted.

Of Direct3D's pipeline in the built-in Windows
(`src/win32/ddraw/d3d/pipeline.rs`) it shares only the function that
makes texture coordinates (`geometry::coordinates`). Screen positions
are rounded to 1/256 of a pixel before rasterising.

What a program reads back from vertex processing (`ProcessVertices`,
clip status, extents) is Direct3D's pipeline's: the HAL leaves those
draws to it, and the card never computes them.

### Rasterising

Triangles cover the pixels whose centres are inside them, with
Direct3D's top-left rule for centres on an edge. The pixel centre is at
whole or at half coordinates as the card state says. A line has the
pixel whose centre is nearest to it in each column it crosses (each row
for a steep one), and a point is a square of its size covered as two
triangles are: the specification's "Drawing" has the rules. Colours, fog
and texture coordinates are interpolated with perspective correction;
depth is interpolated linearly in screen space and stored as
`round(z * (2^bits - 1))`.

### Texels

A texture sample is four 8-bit channels. Unsigned fields with fewer bits
are widened by bit replication, and signed ones (the du and dv of
`L6V5U5`) by a shift, so that they keep their share of the signed range.
DXT blocks are decoded by one decoder with fixed integer formulas (for
the two interpolated colours of a four-colour block,
`(2*a + b + 1) / 3`), which both renderers use: the OpenGL renderer
uploads the decoded texels instead of handing the blocks to the host's
GPU, whose decoders round differently from one vendor to the next.
Textures of this period fit many times over in a current GPU's memory
when decoded.

Bilinear filtering weights have 8 fraction bits. The mip level is chosen
from the larger of the two screen-space derivatives. With anisotropic
filtering the software renderer takes the level from the smaller
derivative instead, or from the larger divided by the anisotropy where
that is more, and samples it with the stage's mip filter: trilinear
filtering with a linear mip filter, one level with a nearest one, and
the first level without mip maps. This is the one place where the
reference is deliberately simpler than the GPU.

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
- `BUMPENVMAP` reads signed (du, dv) from its texture as bytes
  divided by 128, multiplies them by the stage's 2x2 matrix in floating
  point, adds the result to the next stage's texture coordinates, and
  passes `CURRENT` through. `BUMPENVMAPLUMINANCE` also multiplies the
  next stage's sample by `L / 255 * scale + offset`, limited to 0 to
  1.
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
| Vertex arithmetic on the host: the vertex shader's conversion of the card's screen positions, rounded to 1/256 of a pixel, to the host's coordinates, and its operation order and fused multiply-add once it transforms and lights | positions within 1/256 pixel, so a pixel on a triangle's edge may fall on the other side |
| Interpolation of colours and coordinates | 1 in 255 on a stage input. An operation that multiplies its result passes the step on multiplied: 8 after `DOTPRODUCT3` and `MODULATE2X` (Giants' ground, measured) |
| A sample of a 2D texture | 1 in 255, and 2 with a projected texture. The shader takes the card's steps in the card's whole numbers (`shader::SAMPLING`): the level of detail, the levels and the filter, the coordinates in 1/256 of a texel, the weights of 8 bits. What is left is a coordinate or a level of detail that the host's interpolation puts on the other side of one of those steps: a weight one of 256 off, or the next level where the level of detail is within 1/4096 of the change. Coordinates the card has exactly (a texel to a pixel from whole coordinates) give the same texels |
| A sample of a cube map | 1 in 255 on a smooth texture, 3 where neighbouring texels differ by most of a channel's range. The host's own filter samples it: it rounds a weight to its 8 bits where the card cuts it off, works out the face's coordinates in its own arithmetic, and picks the mip level |
| Bump offsets from a linearly filtered bump texture | as the weights, except between texels whose offsets differ in sign: both renderers blend the signed bytes as unsigned ones, so there a weight that is one off can turn 127 into -128 |
| Anisotropic filtering, where the host has it | the host GPU's, which reads more texels than the card's stand-in (trilinear at the level of the shorter side); compared by eye and by a mean error. Measured: a mean of 1.5 and at most 29 on the test's square going into the distance; in Giants 12% of the frame's pixels, 0.2% of the frame by more than 8 (the ground's normal map through `DOTPRODUCT3`) |
| The alpha test and the colour key after a filtered sample | a pixel whose alpha is within the sample's bound of the reference is drawn by one renderer and not by the other. Measured: 0 to 6 pixels a frame of Zanzarah's leaves |
| Depth | 1 step of a 16-bit buffer, 4 steps of a 24-bit one: the host interpolates depth in single precision and rounds it by itself. A depth half-way between two steps goes either way, and so does a test between two surfaces closer than the bound: Giants' marks on the ground, 2 steps of 24 bits in front of it, differ by more than 8 on 0.06% of the frame's pixels, where one renderer alone drew the mark |
| Blending done by the host GPU | 1 in 255 on a channel; one step of the target's on a 16-bit target |
| Lines and points | the same pixels, with the same colours where no texture is sampled: the card works out both. Depth and samples are within the bounds above (measured on lines: samples 1 in 255, depth 1 step of 16 bits and 2 of 24). `ZBIAS` moves a line's depth as a triangle's on both. A pixel of a line within half a pixel of where the line's 1 / w falls to 0, or past it, gets its own values at every corner of its square, so a mip filter picks level 0 for it and fog by w takes w as 1 where the centre has no positive 1 / w |
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

The fragment key is built (`raster::pipeline`, the steps). The vertex
key is the design for a vertex shader and isn't built: the card's
vertex work runs on the processor for both renderers, as code of the
software renderer (`raster::geometry`, `raster::light`).

A key is lowered once into a short list of typed operations (sample
stage 2, modulate, add signed, clamp, fog blend). Two back ends consume
that list:

- the software renderer runs it over the spans of each triangle;
- the OpenGL renderer prints it as GLSL.

Stage semantics are written once, in the lowering, and each back end
implements only the primitive operations. A new stage operation is an
addition to the lowering and to the two small sets of primitives.

### The software renderer

A module of its own (`src/toadforce/raster/`), which draws into card
memory with span workers of the Voodoo renderer's kind: up to four
threads on interleaved rows, so the picture doesn't depend on the thread
count. It is what `--deterministic`, the tests, the web build, and hosts
without OpenGL 3.2 use.

It is slow next to a GPU, and the design accepts that (rule 7). Each
pixel runs up to eight stages and four texture samples on the processor,
with transform and lighting for every vertex before it, on threads the
emulated machine also needs. The aim is a 640x480 game with one or two
textures playable on it. A game at 1024x768 with four textures, or one
that leans on hardware T&L for its vertex counts, doesn't reach full
speed, and a user with such a game needs a host with OpenGL 3.2.

It doesn't break rule 2 of the built-in Windows, which keeps a second
3D pipeline out of the Direct3D layer: this renderer is the card's, as
the Voodoo's software renderer is that card's, and Direct3D still has
one path, the HAL.

### The OpenGL renderer

On the existing context (`glow`, OpenGL 3.2 core, `src/display/gl.rs`),
shared with the window and the Voodoo renderer. The card and the display
run on one thread, and the card hands the renderer its queued draws
directly, at the points where the software renderer would draw them
(see "Between the card and the host renderer").

- **Vertex work** is the card's, on the processor: the host gets screen
  vertices after the card's transform, lighting, clipping and culling,
  and keeps no copies of the vertex buffers in card memory. The card is
  shaped so that a generated vertex shader could do the work instead.
- **Textures** are host textures decoded once and kept by their
  description (format, size, levels and the offset of each image), with
  the write counters of the pages their images cover. A write to any of
  those pages decodes and uploads every image again.
- **Render targets** are host framebuffers. A target drawn on the GPU is
  marked newer there. The card's own work on it on the processor (a
  declined draw, a `BLT` or `FILL` the host doesn't take) and a switch to
  the software renderer read the rows back into card memory first. A
  save state and the program's reads of it read nothing back into card
  memory, and the program's writes only where they write parts of
  pixels or more than 1024 rectangles (see "Between the card and the
  host renderer").
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
  passing through card memory.
- **`BLT` and `FILL`** between surfaces that are current on the GPU run
  there.
- **Host options** that don't change what the guest reads (an internal
  resolution scale, multisampling, forced anisotropy, like
  `voodoo_scale` and `voodoo_msaa`) fit the design, since a read-back
  can be filtered down to the target's own size. The renderer has none.

The renderer is behind `gpu::Host` (`src/toadforce/gl.rs`), on desktop
OpenGL 3.2, and the window attaches it to the card of a running machine
(see "The window's side" below). It draws the screen vertices the card
hands it, and it has:

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
  with anisotropy where the host has that filter. The shader samples
  because the host's filter picks the mip level and rounds the weights
  by itself, and each sample one of 255 off tips a pixel of a dithered
  16-bit target to the next step: with the host's filter on 2D textures,
  frames of Giants differed from the software renderer's on 27% of their
  pixels and frames of Zanzarah on 10% to 15%, against 16% and about 1%
  with the shader's, at the same frame rate;
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
  kept, such a clear is left to the processor;
- points, lines, and triangles drawn as their corners or their edges
  (`FILLMODE`), as triangles the card builds. OpenGL leaves it to the
  GPU which pixels a `GL_LINES` line or a `GL_POINTS` point has, so
  the host draws neither. The card works out a line's pixels by the
  rule of "Rasterising" (`Prim::line`) and hands the host a square
  for each run of them in a row, from pixel edge to pixel edge
  (`Prim::squares`). A triangle covers the pixel centres inside it,
  and a centre on the diagonal the square's two triangles share goes
  to one of them by the top-left rule, so the squares cover the
  line's pixels and no other on any host. The corners carry the
  line's depth, 1 / w and coordinates there, and what the host
  interpolates to a pixel's centre is the line's value at it, with
  the coordinates' change from pixel to pixel for the level of
  detail. The colours are worked out by the card for each pixel, with
  its rounding: a line whose colours change has a square for each run
  of pixels of the same colours. A point is the two triangles of its
  square, with the limits of its size applied by the card.

It declines these states, which the software renderer draws:

- bump offsets on a cube map: they move the coordinates on the face the
  direction picked, and the host's cube sampler takes a direction;
- with the host's own filter (a cube map, and an anisotropic sample
  where the host has the filter: `shader::native`), what that filter
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
context) draw 422 scenes in software and through the host and compare
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
layers. Lines into every octant, of less than a pixel and of no
length, with either place of the pixel centres, with and without the
last pixel, shaded and flat, cut by a viewport and by the scissor
rectangle, blended, alpha-tested and fogged have the same pixels
with the same colours, and so do points of every size at and between
the pixel centres, and triangles drawn as their corners. So do
lines in the plane of a surface with a `ZBIAS` of 0, 1 and 16.
Textured lines, lines whose w grows 16 times along them with each
mip filter, and triangles drawn as their edges are within 1, and a
line's depth within 1 step of 16 bits and 2 of 24. The tests assert
these figures and no looser ones. A scene for each declined state
checks that the software renderer draws it.

They are run by hand on a host with a GPU.

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

- **What the host gets.** The card's renderer queues the primitives of a
  run of commands as it does for the software renderer, and hands the
  host the queue (`Batch`) at the points where the software renderer
  would draw it: each entry is the lowered state (`DrawState`) and its
  triangles as screen vertices, after the card's transform, lighting,
  clipping and culling. A triangle the card culls, or one with no area,
  never reaches the host, so the host has no winding to agree on. A
  point comes as the two triangles of its square and a line as a square
  for each run of its pixels in a row, so the host needs no rule of its
  own for either. The clip rectangle stays in the state for the host to
  cut the pixels by. A vertex is queued as the host's vertex buffer
  takes it (`gpu::Vertex`, 88 bytes: the position, the two colours and
  four sets of coordinates), so a run's triangles go there in one copy.
  The coordinates of texture slot `n` are in set `n` of every vertex,
  whichever set of the program's vertex they came from, and the three
  vertices of a flat-shaded triangle all have the colours of the one
  that gives it them.
- **A draw the host doesn't take.** The host is asked for each state
  (`Host::declines`). A draw it declines (the states listed under "The
  OpenGL renderer") is drawn by the software renderer, and so is a draw
  that samples the surface it writes: the target and the textures are
  read back first, and the target is uploaded again before the host's
  next draw into it. A rare state stays right this way and costs speed:
  a declined draw needs a read-back of the target and its depth buffer
  and an upload after. Grom draws 12 lines a frame, and while the host
  declined lines it ran slower with OpenGL than in software, which is
  why the host takes points and lines as triangles the card builds. The
  queue holds draws of one renderer at a time, and a draw of the other
  empties it first, which keeps the card's order on a target.
- **Who has the newer pixels.** A surface the host drew into is ahead
  on the host, and card memory is stale there. Before the emulator's
  own code reads that memory on the processor the card reads the
  surface back (`ToadForce::settle` with the range): the drivers'
  reads (`win32::screen::read`), the card's own `FILL` and `BLT`
  where they run on the processor, a `CLEAR` of part of a buffer the
  host has no current copy of, a texture or a vertex buffer that
  overlaps it and isn't an image of it, and the picture the screen
  shows when the window can't show the host's copy
  (`Bus::sync_display`). Running the queued commands and reading back
  are separate: the queue is run at every such point as before, and a
  surface is read back only when its own bytes are asked for.
- **What the program and the drivers write.** A write into a surface
  that is ahead reads nothing back. The card notes each byte written
  (`Gpu::writing`, a bit for each byte of the surface), and card
  memory has those bytes from then on while the host has the others.
  Before the host next uses its copies (a draw, `CLEAR`, `FILL`,
  `BLT`, the picture shown) the pixels written go into the host's
  copy as rectangles of whole pixels (`Host::write`), rows with equal
  runs joined into one. A game that locks its back buffer every frame
  to write a picture into it, as Nocturne does with its backgrounds,
  so costs one copy of that picture to the GPU. A surface written in
  parts of pixels, or in more than 1024 rectangles, is read back
  around what was written and copied to the host again.
- **What the program reads.** Handing out a surface's address, at a
  lock, reads nothing back either (`win32::screen::expose` only runs the
  queue). A read of the frame buffer has the bus shared (`Bus::read_8`),
  so it can't change card memory: where the host has newer pixels the
  bus answers from them (`Gpu::read`). The card fetches the surface's
  pixels from the host once, into a buffer of its own beside card
  memory, and answers every read from it until the host's copy changes;
  bytes the processor wrote since come from card memory. The status
  counts each fetch as a read-back for `lock`. A program that only
  writes what it locked never pays for one. A guest's own driver hands
  out no addresses the card knows of, so the card reads everything back
  when a write of `RING_PUT` returns, as the specification promises:
  such a driver gets the host's speed only once its reads are known some
  other way.
- **A saved state** is written from a shared card too. It keeps card
  memory with every surface read back into a copy, and what the
  processor wrote of those surfaces over it
  (`ToadForce::settled`), and the card and the host stay as they are.
  After a load the host drops its copies (`Host::forget`). A
  debugger's look at card memory (`win32::screen::peek`) reads it as
  it is.
- **What the processor wrote.** The card counts the writes to each page
  of its memory (`Pages`), 4 KiB each: the program's, the drivers' and
  its own. The host keeps a stamp of the pages a copy was made from, of
  a texture or of a surface it drew into, and makes the copy again from
  card memory when a page of it was written since. A read-back writes
  card memory what the host's copy holds and isn't counted, and neither
  are the host's draws, so a host that draws into a target drops its
  other copies of the target's bytes: the same surface kept as a
  texture, or another surface over the same memory, such as a depth
  buffer of another size. Without a host nothing is counted. A page is
  coarser than a small surface: Giants' 64x64 targets lie 16 KiB apart
  off the page boundaries, so a write to one makes its neighbours'
  copies stale.
- **What stays on the host.** A `CLEAR` of a whole surface makes the
  host's copy the newer one whatever card memory holds, so a stale or
  missing copy costs nothing. Games clear and draw small targets every
  frame and then sample them (Giants its sea and sky, Zanzarah its
  heroine's shadow), which is what the rest of this point keeps off the
  processor. A texture whose image is a surface the host has ahead (a
  level of a texture, a face of a cube map, in `R5G6B5`, `X8R8G8B8` or
  `A8R8G8B8` without a colour key: `gpu::image_of`) is filled from that
  copy by a shader, which reduces a 16-bit surface kept in 8 bits a
  channel as a read-back would. `FILL`, and `BLT` with no stretch and no
  key, run on the host when a surface of theirs is ahead (`Host::fill`,
  `Host::blt`). `BLT` copies bits, and the formats it names only say how
  large a pixel is, so the host looks its copies up by the bytes the
  command names (`gpu::alias`) and copies what they hold: colour to
  colour, or depth to depth of one layout. DirectDraw's `Blt` between
  two z-buffers names both as a colour format of their size and is such
  a copy. A depth source card memory holds gets a host copy of its own,
  made again only after a write to its pages, so a game that puts a kept
  scene's depths back every frame (Nocturne, behind its drawn
  backgrounds) has them copied on the GPU. A surface inside a larger
  copy of the same bytes (one depth buffer under targets of several
  sizes) uses that copy. Everything else is read back as before: a keyed
  texture, a texture that overlaps a surface without being an image of
  it, a stretch, a copy between colour and depth.
- **A window's back buffer.** The OpenGL front end's and Direct3D 8's
  windowed presents copy the back buffer to the screen with `BLT`
  when it has the screen's format and isn't stretched
  (`ddraw::toadforce::present`), so the host copies its own pixels.
  The front end makes its colour buffer in the desktop's format for
  that: X8R8G8B8 on a 32-bit desktop, 5-6-5 on a 16-bit one, as a
  real driver's windowed pixel formats are of the desktop's depth.
  Other presents (a stretch, a 256-colour desktop) read the back
  buffer and write the screen on the processor.
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
- the processor wrote the surface while the host's copy wasn't
  ahead, as GDI and DirectDraw's blits on the processor do on a
  desktop the host never drew, until the host draws into it again
  (what is written into a surface that is ahead goes to the host's
  copy first);
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

### Shader cache

Programs are cached by key in memory: the draw's steps without what is
the host's fixed state, and which stages the host's own filter samples,
so two draws that differ in the depth test, the blend, the masks or a
texture's sampling states share a program. A game of this period sets
tens to a few hundred distinct state combinations, and each run compiles
them again.

### Save states and determinism

A saved state has whatever is newer on the GPU read back into its copy
of card memory ("Between the card and the host renderer"), and `impl
State` for the card saves its PCI configuration, the registers, the
state the commands set (a fixed-size block: render and stage states,
matrices, lights, material, clip planes, the texture and palette
tables), the mode shown, a pending flip, the position in the frame,
what the VGA's side of the display was when the card's mode took the
screen (`Legacy`), the ring, and the built-in drivers' queue with the
time its first command came. Loading rebuilds the host caches from card
memory, as the Voodoo's `Resync` does. The ring holds no commands still
to run, since the card consumes them at each `RING_PUT`, but the guest
reads back what it wrote there, so its contents are saved with the
rest.

With `--deterministic` no GPU link is attached, as for the Voodoo, and
the software renderer draws, so a change to the OpenGL renderer isn't
run there, as AGENTS.md says of the Voodoo's. To compare the two
renderers' frames at equal emulated times,
`RUST_DOS_TOADFORCE_GL_DETERMINISTIC=1` keeps OpenGL drawing in a
deterministic run (see "The window's side"); such a run's frames depend
on the host's GPU.

## Guest software

### The built-in Windows

This is where the card is used, and it needs no guest driver: the DLLs
are the emulator's.

- **Display.** GDI's desktop and DirectDraw's modes use the card's mode
  registers, at 8, 16 or 32 bits a pixel. DirectDraw surfaces in video
  memory are in BAR 1. A colour fill, and a copy of equal size between
  two of them, is a `FILL` or `BLT` of the card's; a flip is its
  `FLIP`. The driver builds the words a guest's driver would put in the
  ring and hands them to the card without the ring
  (`Bus::toadforce_submit`), so both run one decoder. Blits the card
  doesn't do as DirectDraw's own code does (stretches, which sample
  differently, mirrors, destination keys) stay with that code.
- **The frame rate cap.** The card's presents are the built-in
  Windows': DirectDraw's flips (Direct3D 8's full-screen `Present`
  among them) and the windowed presents of `SwapBuffers` and Direct3D
  8. `fps_cap` holds them back there (`win32::pace`), before the card
  gets its `FLIP` or `BLT`, as it does on every other card; the card
  has no cap of its own (CONFIGURATION.md's "Frame rate cap"). Blits to
  the primary surface aren't held, and a `FLIP` written to the ring by
  a guest's driver wouldn't be either.
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
  the card transforms and lights it, vertices blended by weights
  included. The pipeline keeps the blending the card lacks (by index,
  and tweening) and the draws whose clip status a program reads back.
  Direct3D 7 lists the T&L HAL device beside the HAL one on the card,
  and a program draws on either the same way: Giants picks the T&L HAL
  and hands the card untransformed vertices, where on the HAL device it
  has `ProcessVertices` transform them. The 16-bit shadows of 32-bit
  targets (`d3d/shadow.rs`) aren't used on the card. A texture surface
  in video memory in one of the card's formats is sampled where it is.
  The HAL copies into card memory only what the card can't read there:
  textures in system memory, palettized ones, and sizes that are no
  power of two.
- **State kept between draws.** Most draws follow one with the same
  states, textures and target, and the card's state is then right
  already. What a draw is made from carries a stamp
  (`ddraw/stamp.rs`): a number that no two values share, and that
  changes when the value is borrowed to be written. A Direct3D device
  keeps its states, textures, transforms, lights and target in one
  stamped value (`d3d::device::Drawn`), and DirectDraw its objects in
  another, so a write anywhere in them gives a new stamp with no call
  for the writer to remember. The setters write only a value that
  differs. The HAL keeps its last bind with the two stamps
  (`hal/toadforce.rs`, `Kept`), and a draw that finds both unchanged
  sends the card nothing and looks nothing up. Two things change
  without a stamp and are compared by value: the pixels of a texture
  the HAL keeps a copy of (the surface's uniqueness value), and the
  parts of the pipeline's setup a bind reads. The card's state and the
  kept bind are held together (`told::Told`), so that taking the
  state to change it, as a clear does, drops the bind. Debug builds,
  which the tests are, bind again for every kept draw and stop if the
  bind sends a word or gives another result. `/api/win32/ddraw` counts
  the binds and those kept.
- **Screen vertices go from the program's memory to the queue.** A
  draw of transformed vertices has nothing for the pipeline to work
  out unless a triangle needs cutting, a point or a line needs its
  pixels, or a flat-shaded triangle its colours. `hal::draw_screen`
  (`hal/toadforce/direct.rs`) writes such a draw into the card's
  queue as one `DRAW_INDEXED` triangle list: the vertices read from
  the program's memory into the command's words, in one copy where
  the program's layout is the card's (`D3DFVF_TLVERTEX` with one
  stage) and gathered word by word otherwise, and the indices of the
  triangles the cull mode keeps, a strip's and a fan's too. It
  declines points, lines, the wireframe and point fill modes, flat
  shading, a clip status the program asked for, a draw with a vertex
  outside the guard band or, while the device clips, outside the
  depth range, a draw too large for one command, and vertices in
  memory that isn't plain RAM: those go through the pipeline, as
  every draw of vertices to transform does. A test draws 8000
  made-up draws both ways and compares the triangles the card gets,
  word for word.
  `/api/win32/ddraw` counts the draws of each way (`screen_draws`).
- **Direct3D 8** (`d3d8/`) sits on the Direct3D 7 device, with
  `D3DCAPS8` filled from the HAL's device description
  (`hal::description`, whose sizes and counts are the level's
  constants in `toadforce::caps`) and bits of its own (`d3d8/caps.rs`),
  hardware and mixed vertex processing offered for fixed-function
  draws, the HAL's texture formats, cube textures over DirectDraw's
  cube maps, and a D24S8 depth buffer. Vertex shaders run on the
  processor.
- **OpenGL** (`opengl32/`) is 1.1 on the Direct3D 7 device, and so draws
  on this card through the Direct3D HAL, with the extensions it has on a
  Voodoo3. It lights and transforms by itself and hands the device lit
  vertices, and it offers none of what OpenGL 1.3 adds, although the
  card has the states for it. Its OpenGL front end (contexts, state,
  immediate mode, arrays, pixel transfer) is kept apart from its WGL
  and Win32 parts and takes its calls through one interface, so that a
  booted Windows can feed the same code (see below).
- **OpenGL's draws wait for each other.** A game that draws a
  triangle or a quad for each `glBegin` and `glEnd` would pay
  Direct3D's work for a draw every time. The primitives of draws that
  follow each other with the same state wait instead
  (`opengl32::draw::Waiting`) and go to Direct3D as one draw, in the
  order the program gave them. They hold what they are drawn with by
  value: the device's state, and vertices the lights and texture
  matrices were already applied to. A context's state as Direct3D's
  is made once for each stamp of what its draws are made with, and
  two are the same when every bit is. The draws that wait are drawn
  when a draw with another state or of another context comes, when
  they have 8192 vertices, and before any call of a function that
  isn't known to leave the buffers and the textures' images alone
  (`opengl32::keeps_waiting`). Those known are the functions that
  give vertices and draw them and the ones that set a context's
  state; `glClear`, `glReadPixels`, `glFlush`, `glFinish`, texture
  uploads and deletes, every `glGet` but `glGetError`, WGL with
  `SwapBuffers`, and every function of every other DLL draw them
  first. A function missing from the list costs a draw and no
  picture. Only the calls of the drawing process draw them: another
  process can't name what they draw with, and its call would draw
  them as its own. They go with their process when it ends. They are
  part of the saved state, so a state saved between two calls draws
  the same frame after it is loaded.
- **Glide** isn't offered on this card. A game that wants Glide is
  configured with a Voodoo.

### A booted Windows 98

No driver for the card exists for a booted Windows 95 or 98, which
sees a VESA card without 3D. The card is shaped so that one can be a
thin translation of Direct3D's driver interface. SoftGPU's drivers,
under the MIT licence, do each part of such a driver for other virtual
hardware:

- **The display minidriver** (16-bit, Open Watcom, on Microsoft's DIB
  engine) sets modes through the adapter's mode registers. VMDisp9x
  and boxv9x, on which SoftGPU's is based, do this for Bochs' VBE
  registers; the card's are its `MODE` registers.
- **The DirectDraw and Direct3D HAL** is a 32-bit DLL named through
  `DCICOMMAND`. The card's command set is close to Direct3D's own
  driver command stream, `DrawPrimitives2`: `D3DDP2OP_RENDERSTATE`,
  `TEXTURESTAGESTATE`, `SETTRANSFORM`, `SETMATERIAL`, `SETLIGHT`,
  `CREATELIGHT`, `SETCLIPPLANE`, `SETRENDERTARGET`, `CLEAR`,
  `VIEWPORTINFO`, `ZRANGE` and the draw tokens each map to one card
  command. A HAL reports `D3DDEVCAPS_HWTRANSFORMANDLIGHT` and
  `D3DDEVCAPS_DRAWPRIMITIVES2EX`, the texture formats as `DDSURFACEDESC`
  entries (FourCC ones for DXT), and `D3DCAPS8` through
  `GetDriverInfo2`. Microsoft's Direct3D runtime does the API's
  validation, state blocks and managed textures.
- **OpenGL** on a booted Windows takes an installable client driver.
  The card has no command for OpenGL calls, and level 1 runs no
  shaders, which current Mesa's drivers are handed fixed function
  turned into. The built-in OPENGL32's front end takes its calls
  through one interface, apart from its WGL and Win32 parts.

A guest driver can be handed to the guest on a disk the emulator
generates, as `src/shared_disk.rs` does for shared folders.

## Configuration

The keys follow the Voodoo's, and CONFIGURATION.md describes them for
users.

| Key | Values | Default |
|---|---|---|
| `machine` | `svga_toadforce` | |
| `toadforce_memory` | `32`, `64`, `128` | `64` |
| `toadforce_renderer` | `auto`, `opengl`, `software` | `auto`: OpenGL where the window has a desktop OpenGL 3.2 context |

The card has one level of 3D, so no key chooses one.

## Testing

- **The reference, operation by operation.** Unit tests of each lowered
  operation and each stage operation against values computed by hand
  from the formulas above, and of the DXT decoder against hand-decoded
  blocks.
- **Wine's conformance tests.** `tests/conformance` runs Wine's ddraw
  and d3d8 tests with a Voodoo3, where the tests of DXT textures in
  video memory, cube maps and T&L lights skip. They don't run on a
  ToadForce.
- **Renderer agreement.** Fixed scenes (one for each stage operation,
  fog mode, blend mode, texture format and filter) drawn by both
  renderers and compared within the bounds of "Where the OpenGL renderer
  may differ" (`toadforce::gl::tests`). They need a desktop OpenGL
  context and are run by hand; CI doesn't run them.
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
- **Frozen levels.** A golden copy of each released level's capability
  table and accepted command list (`caps.rs`, `command.rs`).
- **Save states.** A card saved after a draw and loaded draws the next
  one into the same memory as the card that went on
  (`raster::tests`), and a machine's loaded state has the card as it
  was (`tests/video/toadforce_tests.rs`).
- **Determinism.** The software renderer draws the same pixels with any
  number of workers (`raster::tests`). The determinism tests
  (`src/debug/determinism_tests.rs`) have no machine with a ToadForce.

## Licensing

The card's definition and its emulation are new code under the project's
GPL-2.0-or-later. Wine's fixed-function shader generator (LGPL-2.1 or
later) can be ported under the terms the project already uses for Wine's
Direct3D. VMware's SVGA3D headers (GPL-2.0 or MIT) are a model for the
command and capability lists; no code is needed from them. A Windows 98
driver based on VMDisp9x and VMHAL9x (MIT) is compatible with the
project's licence. Nothing from Microsoft's DDKs is copied: the licence
of the Windows 98 and 2000 DDK samples wasn't established, so they are
reading material at most.

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

Cards:

- NVIDIA's table of OpenGL extension support (pages 4 to 6):
  https://developer.download.nvidia.com/opengl/specs/nv30specs.pdf
