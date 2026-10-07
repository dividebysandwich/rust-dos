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

Takes effect the next time Rust-DOS starts, or with `--vr` and
`--vr-desktop` on the command line.

# Scene {#vr-scene}

The 3D scene the screen is in: a glTF file (`.glb` or `.gltf`) exported
from Blender, whose mesh named `screen` shows the picture. **Delete**
goes back to the built-in test room.

Meshes named `led_power`, `led_turbo`, `led_hdd` and `led_floppy` light
up as the PC's lights do, and empties named `speaker_left` and
`speaker_right` are where the sound comes from.

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

- **high**: soft shadows, the screen lighting the room in twelve patches
  of the picture's colours, the light bouncing around the room, and
  corners and gaps darkened (ambient occlusion).
- **medium**: the same with four patches and shadows a little less soft.
- **low**: hard shadows, the screen's light in one colour, and no
  bounced light or ambient occlusion, for slow graphics chips.

The bounced light is worked out when the scene loads, the first time in
a second or so, and kept in `vr-cache` in Rust-DOS's folder for the next.
It takes effect at the next start.

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

# Sound from the screen {#vr-spatial-audio}

The sound comes from the screen's left and right sides (or the scene's
speakers), as if from the monitor: turn your head or the camera and it
moves; come closer and it gets louder. From where the scene starts, it
sounds as it does without the scene. It adds no delay.
