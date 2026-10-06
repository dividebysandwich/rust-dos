# VR {#vr}

The DOS picture on a screen in a 3D scene: in the window, with a camera
you fly around, or in a VR headset (SteamVR, Monado) through OpenXR.

Hold **Ctrl+Shift** and move the mouse to look around in the window's
scene; drag with the left button to slide, use the wheel to move ahead and
back, **Q** and **E** to go down and up, and **Home** to go back to the
start or centre the headset's view.

# 3D scene {#vr-mode}

Where the picture is shown:

- **off**: it fills the window, as usual.
- **in the window**: on a screen in the 3D scene, seen through a camera.
- **in a VR headset**: in the headset, with the left eye's view in the
  window. Without a headset, the scene shows in the window.

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

- **as the scene says**: the scene's choice (its screen's
  `rustdos_screen_fit` property); without one, the picture keeps its
  shape.
- **keep its shape**: the picture keeps its proportions, with black bars
  where the screen is wider or taller than it.
- **stretch to fill**: the picture covers the whole screen, as a CRT's
  width and height knobs turned up would.

# Headset controllers {#vr-controllers}

What a VR headset's controllers do:

- **laser mouse and gamepad**: the hand you last pulled the trigger with
  points a laser; where it meets the screen is the mouse, the trigger its
  left button and the grip its right. The sticks and the other buttons are
  the game port's joystick (or the game's gamepad mapping).
- **laser mouse**: pointing and clicking only.
- **gamepad**: no laser; both hands are the joystick.

The menu button opens and closes the settings window, which the laser
clicks in; held for a second, it centres the view.

# Sound from the screen {#vr-spatial-audio}

The sound comes from the screen's left and right sides (or the scene's
speakers), as if from the monitor: turn your head or the camera and it
moves; come closer and it gets louder. From where the scene starts, it
sounds as it does without the scene. It adds no delay.
