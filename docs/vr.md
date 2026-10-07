# Building rooms for the 3D scene and VR

Rust-DOS can show the DOS picture on a screen inside a 3D room, in its
window or in a VR headset (see [3D scene and
VR](../CONFIGURATION.md#3d-scene-and-vr)). The built-in test room is a
floating screen and a small PC tower under a sunset sky. This page explains
how to make your own room in Blender: the screen the picture goes on, the
PC's lights, where the sound comes from and where you start. It also covers
exporting the room and loading it.

A room is an ordinary **glTF 2.0** file, the format Blender exports with
File > Export > glTF 2.0. Rust-DOS finds the special parts of it by their
**names** (or by custom properties, if you'd rather keep your own names).
Nothing else needs setting up: any mesh, material or light that isn't
special is simply drawn.

## Quick start

1. Model your room in Blender. One unit is one metre.
2. Add a plane (or any surface) for the monitor's glass, name the object
   `screen`, and unwrap its UVs so the picture is upright (see [The
   screen](#the-screen)).
3. Add an empty named `spawn` where your eyes should be, facing the screen
   (see [Where you start](#where-you-start)).
4. Export as **glTF Binary (.glb)** with **+Y Up** and **Custom
   Properties** ticked.
5. Run `rust-dos --vr-desktop --vr-scene myroom.glb` and look around (hold
   Ctrl+Shift and move the mouse).

## Starting from the test room

[`assets/vr/test-room.blend`](../assets/vr/test-room.blend) is the built-in
test room as a Blender scene, ready to change. It has the screen, the
`spawn`, the two speakers, the tower with its four lights, the materials,
and a `README` text block with the export settings.
[`assets/vr/test-room.glb`](../assets/vr/test-room.glb) is its export.
Loaded with `--vr-scene`, it looks the same as the built-in room, apart
from the floor's grid: here it is a texture, which fades a little sooner in
the distance than the built-in room's.

In the `.blend`, the **Viewport only** collection (a sun lamp in the
built-in sunset's direction, a camera at the spawn, a blue world) is there
to make Blender's viewport look like the room. Export with **Punctual
Lights off** to have Rust-DOS light the room with its own sunset, as the
built-in room is. With them on, the sun lamp lights it instead, with less
of the sky's light.

`assets/vr/make_test_room.py` builds both files from scratch, from the same
numbers as the built-in room:

```sh
blender --background --factory-startup --python assets/vr/make_test_room.py
```

## The names Rust-DOS looks for

| Object in Blender | Kind | What it does |
|---|---|---|
| `screen` | Mesh | Shows the DOS picture. Required. |
| `spawn` | Empty (or a camera) | Where your eyes start, and which way they face. |
| `speaker_left`, `speaker_right` | Empty | Where the left and right sound channels come from. |
| `led_power`, `led_turbo`, `led_hdd`, `led_floppy` | Mesh | The PC's front-panel lights. |

Names are not case-sensitive. LED and speaker names may carry Blender's
copy suffix (`led_hdd.001` works). **`screen` and `spawn` must be exact**:
`screen.001` is not the screen, so rename an object you duplicated.

Instead of names you can use **custom properties** on the object (Object
Properties > Custom Properties):

| Property | Value | Same as naming it |
|---|---|---|
| `rustdos_screen` | `1` or `True` (a boolean) | `screen` |
| `rustdos_led` | `power`, `turbo`, `hdd` or `floppy` (a string) | `led_…` |

Custom properties only reach Rust-DOS if **Include > Custom Properties** is
ticked in the export dialog.

## The screen

The screen is the surface the DOS picture is drawn on. It is required: a
file without one is rejected, and the test room is shown instead.

- **Any shape works:** a flat plane, a slightly bulging CRT glass, even
  something odd. The picture follows the surface's UV map.
- **UVs decide where the picture goes.** Unwrap the screen so its UVs fill
  the whole 0 to 1 square, with the picture upright as an image texture
  would show it. A quick check is to give it an image texture of any picture
  in Blender. If that picture looks right on the screen, the DOS picture
  will too. Only the first UV map is used.
- **Proportions:** the picture keeps its own shape and is letterboxed (black
  bars) to fit the screen's. Most DOS pictures are 4:3, so a 4:3 screen
  (say 40 × 30 cm) fills edge to edge. The screen's shape is measured from
  its size and its UVs, so stretched UVs make it look wider or narrower than
  it is.
- **Filling a screen that isn't 4:3:** many monitor models' glass is a
  little taller or wider than the picture. To stretch the picture over the
  whole screen instead, as a CRT's width and height knobs would, give the
  screen object (or its mesh) the custom property `rustdos_screen_fit` with
  the text `stretch`. Players can override it with the `screen_fit` setting
  (the VR page's *Picture on the screen*).
- **Its own material is ignored:** the screen always shows the picture,
  lit by nothing (it glows). It is visible from both sides. Put the monitor
  case, bezel and so on in **separate objects**.
- **Children count too:** everything parented under the `screen` object is
  screen as well. Don't parent the monitor case to the screen; parent both
  to an empty instead if you want them to move together.
- **The mouse and the laser** point at the screen through its triangles,
  so a curved screen gets accurate pointing.
- **Glow:** the screen lights what is in front of it (the desk, the
  keyboard, the insides of the bezel) as a CRT does in a dark room, as a
  rectangle of light in patches of the picture's colours: a blue sky at the
  top of the picture lights the ceiling blue. Surfaces behind the screen's
  front side aren't lit by it, and things in front of it cast soft
  shadows. A scene custom property `rustdos_screen_glow` (a number, 5 by
  default) sets how bright the light is; players can change it with the
  `screen_glow` setting.

The CRT look chosen in the settings (scanlines, aperture grille, shadow
mask) is drawn onto the screen, but without the tube's curve. If you want
a curved tube, model the curve.

## Where you start

An **empty named `spawn`** is where your eyes are and which way you face
when the room loads.

- An empty with no rotation looks along **Blender's +Y** (into the screen
  when you view the scene from the Front view, numpad 1). Rotate it about Z
  to turn.
- In the window, the camera also takes the empty's up or down tilt. In a
  headset only the turn counts: your real head decides the tilt.
- Put it at **eye height**. For a seated room, about 1.1 to 1.2 m above the
  floor, half a metre or so in front of the desk's edge. In a headset, the
  spawn is where your head is when the headset's view was last centered:
  when it first shows the scene, and with Ctrl+Shift+Home, the
  controller's menu button held for a second, or **Center the view where
  you sit now** on the settings window's VR page. Players fine-tune it
  there too, in cm and degrees, and the scene's scale (`seat_right`,
  `seat_up`, `seat_forward`, `seat_turn`, `scene_scale`), so a spawn a few
  centimetres off is easy to live with, but get it close.
- **Without a `spawn`**, the first camera in the scene is used. Without
  either, you start 1.2 m above the origin looking along +Y.

The scene has no collision or walking. The window's camera flies anywhere,
and in a headset you move by moving.

## Speakers

With spatial audio on (the default), the left channel comes from one point
and the right channel from another. Turning your head moves the sound
around the room, and stepping closer makes it louder.

- Add two **empties** named `speaker_left` and `speaker_right`, where the
  sound should come from: the monitor's speakers, a pair of desk speakers,
  the PC case.
- Without them, the sound comes from the **left and right edges of the
  screen**. You can also place just one; the other then stays at the
  screen's edge.
- From the `spawn`, the sound is exactly the plain stereo it is without the
  scene. Everything is relative to that, so place the spawn and the
  speakers the way you'd sit in front of them.

## The PC's lights

Meshes named `led_power`, `led_turbo`, `led_hdd` and `led_floppy` (or with
a `rustdos_led` property) light up like the emulated PC's:

| Light | Lit while |
|---|---|
| `led_power` | Always, while Rust-DOS runs |
| `led_turbo` | The CPU runs faster than 1000 instructions a millisecond (above XT speed) |
| `led_hdd` | A hard disk's files or sectors are read or written |
| `led_floppy` | A floppy disk's are |

How they look:

- **Lit**, the light glows in its material's **Emission colour**. If the
  material has no emission, it glows in its Base Color instead.
- **Dark**, the emission is off and you see the base colour, lit by the room
  like anything else. Give LEDs a dark, tinted base colour (dark red for the
  HDD light) and a bright emission colour.
- Each light gets its own copy of its material. You can share one material
  between all four LEDs and they still switch on and off separately.
- Keep LEDs small (a few millimetres) and slightly in front of the panel, so
  they don't flicker against it.

## Materials

Rust-DOS draws a simple version of Blender's Principled BSDF:

- **Base Color** and its image texture.
- **Emission** colour, strength and image texture (glowing parts, lamps,
  the power light). A strength above 1 makes it brighter, up to a soft
  white-out.
- **Alpha**: Blend (see-through glass, smoke) and Clip / Alpha Mask
  (leaves, grilles).
- **Backface Culling** off (double-sided) or on.
- **Unlit** materials (glTF's KHR_materials_unlit) show their colour as it
  is, ignoring the lights. This is the way to bring in **baked lighting**:
  bake the room's lighting into its textures in Blender, then export the
  materials as unlit. The Blender manual's glTF 2.0 page explains which
  shader setups export as unlit.

Not used: metallic and roughness, normal maps, occlusion, clearcoat and the
other extensions, texture transforms, vertex colours, a second UV map. A
surface ignores what it can't use and is still drawn.

Textures must be **PNG or JPEG**, which is what Blender exports.

## Lights and sky

- **Lights:** point, spot and sun lights are used, up to **8** of them.
  Area lights aren't (glTF has none). Blender's export dialog has a
  **Lighting Mode**: pick **Unitless**, which keeps intensities close to
  Blender's numbers. "Standard" turns watts into much larger photometric
  values, and the room comes out blown out. If the room is too bright or too
  dark overall, add a **scene** custom property `rustdos_exposure` (Scene
  Properties > Custom Properties), a number. 1 is the default, 0.5 is half
  as bright.
- **No lights:** the room is lit by a low orange evening sun and a dim blue
  sky light, as the test room is. With lights of its own, the room still
  gets a little of the sky's light.
- **Sky:** the dark blue sunset sky surrounds the room, and distant things
  fade into it. For a **closed room**, or one with its own skybox mesh,
  turn it off with a scene custom property `rustdos_sky` set to `0` or
  `False`. Outside the room it is then night-dark.
- **Shadows:** the lights cast shadows: through a window, a spot or sun
  light makes a window-shaped patch with the frame's shadow in it. Up to
  **4** lights cast shadows, the first in the file. To keep a light from
  casting them (a fill light, say), give the light object the custom
  property `rustdos_shadow` set to `0` or `False`; for none at all, the
  scene custom property `rustdos_shadows`. Glass and other see-through
  (Blend) materials, and black glowing surfaces (a sky backdrop outside the
  window), cast none.
- **Bounced light:** light falling on a surface lights the rest of the room
  in its colour, twice over: the sunlit patch on the wall warms the room,
  the screen's light reaches the walls beside it. Rust-DOS works it out when
  the room loads, by looking around from points on a grid about 40 cm
  apart over the room, and keeps it for the next start. Make sure the room
  is closed where it should be dark: light comes in through gaps in the
  walls. The scene custom property `rustdos_gi` set to `0` turns it off,
  for scenes with their lighting baked in.
- **Ambient occlusion:** corners, the gap under a monitor, the floor around
  a chair's legs get less of the light from all around (the room's fill
  and the bounced light), as they do in a real room. It is worked out from
  what each view sees, so it needs nothing from the scene. The scene
  custom property `rustdos_ao` set to `0` turns it off, for scenes with it
  baked into their textures.
- Fake light (an emissive "sunbeam" patch on a wall) isn't needed any more
  for light the scene's lights already give; it would double it.

## Exporting from Blender

File > Export > glTF 2.0, with:

| Setting | Value | Why |
|---|---|---|
| Format | **glTF Binary (.glb)** | One file. glTF Separate (`.gltf` + `.bin` + textures) works too; keep the files together. |
| Include > Limit to | Selected / Visible Objects, as you like | Anything exported is drawn. |
| Include > Custom Properties | **On** | For `rustdos_…` properties on objects and the scene. |
| Include > Cameras, Punctual Lights | On | For a camera as the start, and the lights. |
| Transform > +Y Up | **On** (the default) | Rust-DOS expects glTF's Y-up. |
| Mesh > Apply Modifiers | **On** | Rust-DOS sees only the final mesh. |
| Mesh > UVs, Normals | On | UVs for the screen and textures. Normals for lighting; without them, smooth normals are made. |
| Lighting > Lighting Mode | **Unitless** | See [Lights and sky](#lights-and-sky). |
| Animation | Off | Animations aren't played; objects stay as they are in the current frame. |

Other things to keep in mind:

- **Scale:** one Blender unit is one metre, and real sizes matter. The
  room is seen at real size in a headset, and the speakers' distance
  changes the sound's loudness. Apply scale (Ctrl+A > Scale) on objects you
  scaled a lot. The export handles rotation and scale either way, but
  applied scale keeps normals and UV-based sizes predictable.
- **Triangles only:** faces and quads are fine (the export triangulates
  them), but loose edges, points and curves that aren't converted to mesh
  aren't drawn.
- **Polygon count:** the headset draws the room twice per frame at 90 Hz
  or more. Tens of thousands of triangles are fine; millions aren't.
- **Texture sizes:** 2048 × 2048 or less per texture is plenty. Textures are
  mipmapped and filtered, so they don't shimmer when seen from afar.
- **Skinned or morphing meshes** are drawn in their rest shape.
- If the file uses **Draco mesh compression** or other compression
  extensions, Rust-DOS can't read it. Leave compression off.

## Loading the room into Rust-DOS

Pick the file in any of these ways:

- **Settings window:** Ctrl+F12, the **VR** tab. Set *3D scene* to *3d in 2d
  window* or *VR headset* (that takes effect at the next start). Enter on
  *Scene* opens the list of scenes: pick *Browse for a scene file* and
  your file. It shows at once, and F2 keeps it. Delete on *Scene* goes
  back to the test room.
- **Configuration file:** in `rust-dos.conf` (relative paths are from the
  file's folder):

  ```ini
  [vr]
  mode=desktop        ; or headset
  scene=rooms/myroom.glb
  ```

- **Command line**, for this run only:

  ```sh
  rust-dos --vr-desktop --vr-scene rooms/myroom.glb   # in the window
  rust-dos --vr --vr-scene rooms/myroom.glb           # in a VR headset
  ```

  `--vr-scene` on its own also means `--vr-desktop`.

A room that can't be loaded (a missing file, a picture that isn't PNG or
JPEG, no screen) is replaced by the test room. The reason is printed in the
terminal and written to the [log file](../README.md#log-file)
(`Rust-DOS.log`), on a line starting with `[VR]`.

## Sharing a room

The settings window's scene list offers the rooms listed in
[`vr/scenes.json`](https://rust-dos.com/vr/scenes.json) on rust-dos.com,
for players to download with Enter. To have yours listed, put its files
where they can be downloaded over HTTPS at a fixed address (a GitHub
repository's files at a commit, or a release), and ask for an entry in the
website's repository, `rust-dos-site`:

```json
{
  "id": "myroom",
  "name": "My room",
  "version": "1",
  "author": "you",
  "description": "What the player sees, in a sentence or two.",
  "license": "CC BY 4.0",
  "homepage": "https://github.com/you/myroom",
  "requires": "1.4.0",
  "scene": "myroom.glb",
  "files": [
    { "path": "myroom.glb", "url": "https://...", "size": 1234567, "sha256": "..." }
  ]
}
```

- `id` is the folder the room is downloaded into: lowercase letters,
  digits, `-` and `_`.
- `scene` is the file Rust-DOS loads. A `.gltf` lists its `.bin` and
  pictures in `files` too, with their paths beside it; a credits file can
  come along the same way.
- `size` (in bytes) and `sha256` are checked for every file, so a room
  that doesn't match isn't kept.
- A new `version` shows players an update.
- `requires` is the oldest Rust-DOS the room works with.

## Checking a room

Work in the window first. It's quicker than putting on a headset:

```sh
rust-dos --vr-desktop --vr-scene myroom.glb
```

- Hold **Ctrl+Shift** and move the mouse to look around. Drag with the left
  button to slide, use the wheel to move forward and back, press **Q** and
  **E** to go down and up, and **Home** to return to the spawn.
- **Is the picture upright and the right way round?** If it is mirrored or
  upside down, fix the screen's UVs (flip them in the UV editor).
- **Does the mouse land under the pointer?** Move the host mouse over the
  screen at the DOS prompt with a mouse program running. If it is off,
  the UVs are stretched or don't fill 0 to 1.
- **The lights:** `copy` a large file at the DOS prompt (the HDD light
  flickers), or slow the CPU down with Ctrl+F11 until the turbo light goes
  out.
- **The sound:** play something with sound, turn the camera away from the
  screen, and listen for the sound moving to one side.
- **Too dark or too bright:** adjust `rustdos_exposure`, the lights'
  strengths, or switch materials to unlit with baked lighting.

Then try it in the headset with `--vr`. In a headset, also check the scale:
a desk that looks fine in the window can feel like a doll's house or a
giant's table at real size.
