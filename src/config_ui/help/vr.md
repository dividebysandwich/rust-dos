# VR {#vr}

The DOS picture on a screen in a 3D scene: in the window, with a camera
you fly around, or in a VR headset (SteamVR, Monado) through OpenXR.

Hold **Ctrl+Shift** and move the mouse to look around in the window's
scene; drag with the left button to slide, use the wheel to move ahead and
back, **Q** and **E** to go down and up, and **Home** to go back to the
start or center the headset's view.

# 3D scene {#vr-mode}

Where the picture is shown:

- **off**: it fills the window, as usual.
- **3d in 2d window**: renders the 3d scene on your physical 2d monitor.
- **VR headset**: render in a connected VR headset, desktop shows left eye
view. Without a headset, the scene shows in the window.

Takes effect at once; `--vr` and `--vr-desktop` choose it on the command
line. On Linux, Rust-DOS asks the headset's runtime as it starts how it
draws (**Headset graphics**): turned on later, a headset that needs
another kind of window keeps the setting with **F2** and starts again.

# Scene {#vr-scene}

The 3D scene the screen is in: a glTF file (`.glb` or `.gltf`) exported
from Blender, whose mesh named `screen` shows the picture. **Enter** opens
the list of scenes: the built-in test room, those downloaded from
rust-dos.com, and a file of your own. **Delete** goes back to the test
room.

Meshes named `led_power`, `led_turbo`, `led_hdd` and `led_floppy` light
up as the PC's lights do, and empties named `speaker_left` and
`speaker_right` are where the sound comes from.

# Choose a scene {#vr-scenes}

**On this computer**: the test room and the scenes downloaded. **Enter**
chooses one, **Delete** deletes a downloaded one. *Browse for a scene
file* picks a `.glb` or `.gltf` of your own.

**On rust-dos.com**: *Show the scenes rust-dos.com has* asks the website
for its list; nothing is asked before. **Enter** on a scene downloads it
(its size is beside it), checks every file, and chooses it. *update*
means a newer one is there.

Scenes are kept in `vr-scenes` in Rust-DOS's folder. The scene chosen
shows at once, in the window and the headset; **F2** keeps it.

# Picture on the screen {#vr-screen-fit}

How the DOS picture fills the scene's screen:

- **defined by scene**: the scene's choice (its screen's
  `rustdos_screen_fit` property); without one, the picture keeps its
  shape.
- **fit**: the picture keeps its proportions, with black bars
  where the screen is wider or taller than it.
- **stretch to fill**: the picture covers the whole screen, as a CRT's
  width and height knobs turned up would.

# Lighting {#vr-quality}

How much of the scene's lighting is worked out:

- **auto**: high, or medium on a standalone headset's own graphics chip.
- **high**: soft shadows, the screen lighting the room in twelve patches
  of the picture's colours, the light bouncing around the room, and
  corners and gaps darkened (ambient occlusion).
- **medium**: the same with four patches and shadows a little less soft.
- **low**: hard shadows, the screen's light in one colour, and no
  bounced light or ambient occlusion, for slow graphics chips.

The bounced light is worked out when the scene loads, the first time in
a second or so, and kept in `vr-cache` in Rust-DOS's folder for the next.
It takes effect at once.

# Ambient occlusion {#vr-ambient-occlusion}

Corners and gaps darkened, as they get less of the light from all
around. It takes the graphics chip a pass of its own.

- **auto**: as the lighting has it, but off on a standalone headset.
- **on** or **off**, whatever the lighting.

# Antialiasing {#vr-msaa}

How many samples each pixel of the scene takes, for smooth edges: **4x**,
**2x** or **off**. **auto** is 4x, or 2x on a standalone headset. Each
sample more costs the graphics chip a little more.

# Light from the screen {#vr-screen-glow}

How brightly the screen lights the room, in percent of what the scene
says (its `rustdos_screen_glow` property). 0% turns its light off. A
bright picture lights the keyboard and the desk, a dark one leaves them
in the dark, and the room takes on the colours of the picture's parts.

# Headset controllers {#vr-controllers}

What a VR headset's controllers do:

- **laser mouse and gamepad**: the hand you last pulled the trigger with
  points a laser; where it meets the screen is the mouse, the trigger its
  left button and the grip its right. The sticks and the other buttons are
  the game port's joystick (or the game's gamepad mapping).
- **laser mouse**: pointing and clicking only.
- **gamepad**: no laser; both hands are the joystick.

The menu button opens and closes the settings window, which the laser
clicks in; held for a second, it centers the view.

The grip button also picks up what is on the desk: the running game's
manual, whose pages turn with that hand's thumbstick (A/X picks the next
manual, if there are several), and the joystick. Hold the joystick by its
stick and the stick follows your hand as the game port's joystick, the
trigger and A/X its buttons; hold it by its base to move it. Let go and
it settles on what is below.

Type on the keyboard on the desk with the controllers' tips: push a tip
down into a key, and bring it up over the keys again for the next one.
The laser and the trigger press a key for a moment (the key it points
at lights up); Shift, Ctrl or Alt pressed so stays down for the next
key. Take the keyboard by its case to
move it. It is hidden while **Show the real keyboard** shows the real
one.

# Center the view {#vr-center}

Puts your eyes where the scene's seat is (its `spawn`), facing the
screen, from where your head is now. Sit as you play and press **Enter**.

The view centers itself when the headset first shows the scene; this,
**Ctrl+Shift+Home** and the controllers' menu button held for a second
center it again.

# Scene scale and seat {#vr-seat}

Fine-tune where you sit in the scene, in small steps with **Left** and
**Right**, or type a number with **Enter**. **Delete** puts one back.

- **Scene scale**: how big the room looks, 50 to 200%. Above 100% it
  is bigger and you smaller.
- **Seat to the right**, **higher**, **closer to the screen**: moves you
  from the scene's seat, in cm (-100 to 100).
- **Seat turned to the left**: turns you, in degrees (-180 to 180).

They take effect at once, and are kept with the other settings.

# Show the real keyboard {#vr-keyboard}

Lets the headset's cameras show your real keyboard (and your hands on
it) through the scene, in a window on the desk with soft edges. It needs
a headset whose cameras the program can show: the Steam Frame running
Rust-DOS on the headset itself, Varjo's XR headsets, or a Quest through
Link. Elsewhere the headset says it can't, and nothing changes.

- **auto**: on the desk below the screen, in front of your seat. With
  the hands tracked (controllers put down), the window moves to where your
  fingers press keys, and is kept for the next time.
- **marked**: where you marked it with a controller.
- **off**: the scene only.

**Mark the keyboard with a controller** shows the room through the scene.
Put the controller's tip on one front corner of the keyboard, hold the
trigger and move to the other front corner, let go; then touch the back
edge and pull the trigger. The grip button cancels. What you mark is kept
for the next time, and **Show the real keyboard** becomes **marked**.

Turning it on or off starts the headset again.

# Time of day {#vr-time}

The time of day in the scene: where the sun and the moon are, how light
the sky is, the stars at night. Scenes with windows show it through them.

- **Time of day**: **now** follows the computer's clock; a time starts
  the scene's day there.
- **Time goes**: how fast the scene's day goes on: as the clock (real
  time), faster (up to an hour a second), or **stands still**.
- **Latitude**: how far north (or south) of the equator the scene is,
  which sets how high the sun climbs. The sun rises at six and sets at
  six, as at the equinox.

They take effect at once.

# Weather {#vr-weather}

What the sky does outside: **clear**, **rain**, **thunderstorm** or
**snow**. Rain runs down the windows, and clouds dim the sun.

- **Rain and thunder volume**: how loud the rain on the windows and
  the thunder are, 0 to 100%. The rain sounds from the windows.
- **Lightning**: flashes in a thunderstorm, which light up the room,
  with thunder a few seconds after.

# Headset resolution {#vr-resolution}

How sharp the headset's picture is: the size of each eye's image in
percent of what the headset's runtime recommends, 30 to 150%. Lower it
when the headset stutters; raise it for a sharper picture on a fast
graphics card. **Left** and **Right** step in tens; **Enter** types a
number; **Delete** puts back **auto**. The headset starts again with it at
once.

**auto** draws the size the runtime recommends; on a standalone headset's
own graphics chip, as much of it (from half up) as there is time for at
the headset's rate, a little less when it is busy and more when it isn't.

# Foveated rendering {#vr-foveation}

Draws the edges of the view coarser than where your eyes look, which
follows your gaze, so the graphics chip has more time for the rest. It
uses Valve's own foveation, which only the Steam Frame has. It does
nothing elsewhere.

- **auto**: medium on a Steam Frame.
- **low**, **medium** or **high**: how much coarser the edges get.
- **off**: all of the view is drawn sharp.

While it is on, all of the eyes' images are drawn (**auto** headset
resolution doesn't adapt). It takes effect at the next start.

# Headset refresh rate {#vr-refresh}

How many times a second the headset shows a picture, where its runtime
lets programs choose: the nearest it offers to the one picked. **auto**
leaves the runtime's own, or on a standalone headset takes the lowest from
72 Hz, which leaves the graphics chip the most time for each picture.

# Headset graphics {#vr-graphics}

How the headset's pictures get to its OpenXR runtime:

- **automatic**: whatever the runtime takes, OpenGL first.
- **OpenGL (GLX/WGL)**: as SteamVR takes it. On Linux it needs X11
  (XWayland on a Wayland desktop).
- **OpenGL through EGL**: as Monado takes it, under Wayland too.
- **Vulkan bridge**: for a runtime that takes Vulkan only. The scene is
  still drawn with OpenGL, then copied with Vulkan; both must run on the
  same graphics card.

Takes effect when Rust-DOS starts again (keep it with **F2**);
`--vr-graphics` chooses it on the command line. `vr.log` in Rust-DOS's
folder says which was used, and `--vr-probe` writes what the headset's
runtime offers to `vr-probe.log`.

# Sound from the screen {#vr-spatial-audio}

The sound comes from the screen's left and right sides (or the scene's
speakers), as if from the monitor: turn your head or the camera and it
moves; come closer and it gets louder. From where the scene starts, it
sounds as it does without the scene. It adds no delay.

# VFX1 headset {#vfx1}

Puts a Forte VFX1, a VR headset from 1995, on the desk of the scene, and
its card and driver in the PC for games that know it: Descent, Descent 2,
System Shock, MechWarrior 2 and Quake (through VFX1QK) among them.

Pick the headset up with the grip button and bring it to your face,
whichever way round: it goes on, and you see what the game draws for each
eye, with your head turning the game's view. To take it off, grip at your
head and pull it away; put it down on the desk or throw it anywhere. On
the floor, out of reach, it is back on the desk.

- **Card port**: where the card is, which `VIPPORT` tells games (set for
  them as `VIPPORT=260 I5`). The card also answers at 300h, where some
  games look for it.
- **Swap the eyes**: for a game that draws them the other way round.

Turning it on or off, or another port, takes effect at the DOS prompt.
Start the game with its VFX1 switches, as `DESCENT -vfx -vfxtrak`.

# VFX1 stereo {#vfx1-stereo}

How the eyes' pictures are taken from the screen:

- **auto**: as the game tells the headset, or else, while it reads the
  head tracker, **VFX1 pages** on a 640x480 screen and **interlaced
  lines** on one of 400 lines (Comanche 3); the whole screen for both
  eyes for its menus.
- **VFX1 pages**: the VFX1's own stereo, which most of its games draw:
  the left eye on the left half of the screen, the right on the right,
  each page on every other line.
- **interlaced lines**: even lines the left eye, odd lines the right, as
  i-glasses! and CyberMaxx games and Quake's `lcd_x` draw.
- **side by side**: the left half of the screen the left eye.
- **off**: the whole screen for both eyes.

# VFX1 picture size {#vfx1-view-size}

How wide the picture looks, in degrees. The VFX1's was 36°, like a
120 inch screen seen from 11 feet; more makes it fill more of the view.

# Head turns steer {#vfx1-steer}

For games that turn by how far the head moved since their last frame,
as Descent turns the ship: makes the movement larger as the game draws
more than 30 frames a second, so the ship turns as fast as it did then.
Leave it off for games where the head just looks around.

# Shutter glasses' 3D BIOS {#vfx1-stereo-bios}

Answers games as the 3D BIOS of shutter glasses did (Kasan's 3DMax, the
LCDBIOS of SimulEyes and others), so that a game set up for them draws
its eyes on alternate lines, which the VFX1 shows with **Stereo** on
**auto**. Some draw more than the VFX1's own stereo: Descent 2 with
`VR_type=6` and `VR_resolution=4` in DESCENT.CFG draws 800x300 for each
eye, where its VFX1 setting draws 320x200. Without the VFX1's head
tracker, though: the game reads it only in its VFX1 setting.
