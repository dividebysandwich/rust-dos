# The VFX1 headset: how many pixels an eye gets

The Forte VFX1's two screens had 263x230 dots each. Rust-DOS shows the
games' own pictures, not the headset's dots, so an eye sees all a game
draws for it. This page is about how much that is in the games most
asked about, and whether they can be made to draw more for each eye
while keeping their stereo and the head tracker.

How to set up the headset and the games is in CONFIGURATION.md, under
"The VFX1 headset" and `[vfx1]`.

## In short

| Game | Stereo it draws | Each eye | Head tracker | More is possible? |
|---|---|---|---|---|
| Descent | VFX1 pages, 640x480 | 320x240 (160x120 doubled unless asked) | Yes | Only by patching the game |
| Descent 2 | VFX1: lines, 320x400 | 320x200 | Yes | **Yes: 800x300**, without the head tracker |
| Comanche 3 | Lines, 320x400 | 320x200 | Yes (Alt+F12) | No |
| Duke Nukem 3D | None of its own | - | Not by itself | Only by patching the game |

All four were checked with their released source code where there is
some (Descent, Descent 2, Duke Nukem 3D's game code), and Descent,
Descent 2 and Comanche 3 by running them in Rust-DOS with
`RUST_DOS_VFX1_DUMP`, which writes the eyes' pictures out.

## Descent

Descent's VFX1 mode is fixed in its code: `-vfx` calls
`game_init_render_buffers(SM_640x480V, 320, 240, 0, VR_AREA_DET, 0)`
(MAIN/INFERNO.C), a 640x480 VESA screen with a 320x240 picture for each
eye, the left eye's in the left half. Each frame it draws both eyes and
copies them onto every other line of the screen, the even lines one
frame and the odd lines the next (`game_render_frame_stereo_vfx`,
MAIN/GAME.C), then has the headset show the lines just drawn.

Out of the box it draws even less: `VR_low_res` starts at 3, which draws
160x120 for each eye and doubles it both ways. Shift+F6 steps it down to
0, the whole 320x240. That is the most Descent draws for the VFX1.

The other headsets Descent knew (i-glasses!, CyberMaxx, Kasan's 3DMax)
draw 320x200 or less for each eye, interlaced. Drawing more would mean
changing the numbers in DESCENT.EXE, a patch of the game, which is out of
what Rust-DOS does.

## Descent 2

Descent 2 sets up its headset in DESCENT.CFG (`VR_type`,
`VR_resolution`, `VR_tracking`), which its SETUP writes. Version 1.2,
whose source was released, and GOG's copy draw:

| `VR_type` | Headset | Screen | Each eye |
|---|---|---|---|
| 1 | VFX1 | 320x400, lines | 320x200 |
| 2, 3 | CyberMaxx, CyberMaxx 2 | 320x200 or 320x400, lines | 320x100 or 320x200 |
| 4 | i-glasses! | 320x400, lines | 320x200 |
| 5 | Kasan's 3DMax | 320x200 or 320x400, lines | 320x100 or 320x200 |
| 6, 7, 8 | LCDBIOS, CyberShades, SimulEyes | `VR_resolution` 0 to 4 | up to 800x300 |

The shutter glasses of types 6 to 8 go through a "3D BIOS" on INT 10h,
and `VR_resolution=3` draws 640x480 (640x240 for each eye), `4` draws
800x600 (800x300 for each eye), in a VESA mode, interlaced. Its VFX1
setting stays at 320x200 (the code says "headset can't display 640x480").

Rust-DOS answers as that 3D BIOS (`[vfx1] stereo_bios`, on by default)
and shows its interlaced lines in the VFX1. With `VR_type=6` and
`VR_resolution=4`, an eye gets 800x300, 3.75 times the pixels of the
VFX1 setting. In that setting Descent 2 doesn't read the VFX1's head
tracker, though (run with `VR_tracking=1`, `/api/status` shows no reads):
the VFX1 is only a display then, and the ship is flown with the
controllers or the keyboard. To have both, Descent 2 would have to be
patched to start the tracker in the shutter glasses' setting.

## Comanche 3

Comanche 3's Video menu (Esc in flight) has **Stereoscopic**, which
draws the two eyes on alternate lines of a 320x400 screen, and Alt+F12
turns on the head tracking, which reads the VFX1's tracker. The game
doesn't tell the VFX1's driver that it draws stereo; with `stereo=auto`,
Rust-DOS shows a screen of 400 lines as interlaced lines while the game
reads the head tracker. The forte-vfx1-vr project hadn't had it working.

Its other video modes, up to 640x480, are flat pictures: stereo is
320x200 for each eye and no more.

## Duke Nukem 3D

Duke Nukem 3D draws no stereo of its own. Its Build engine had stereo for
shutter glasses from mid-1996 on (`setstereo`, for SimulEyes and Nuvision's
3D-Spex, in VESA modes; Ken Silverman's notes in BUILD2.TXT), but Duke Nukem
3D's game code never turns it on. Head tracking would come through its
"external" controller, a driver program of its own, which Rust-DOS doesn't
have. No DOS copy was at hand to try it.

The only way to stereo at its high resolutions would be to change the
game: draw every other frame from a little to the left and a little to
the right, by moving the player's view in its memory. That is a patch of
each version of the game, and out of what Rust-DOS does.

## Stereo from the 3D cards

Games that draw through an emulated 3D card (the 3dfx Voodoo, S3 ViRGE,
Rendition Vérité) hand Rust-DOS their triangles with their depth, and
Rust-DOS draws them again at the host's resolution. It could draw each
frame twice, once for each eye, moving each triangle's corners sideways
by how far away they are, as NVIDIA's 3D Vision did for Direct3D games:
stereo at the host's resolution for any such game, whether it had stereo
or not.

None of the games above draws through a 3D card in the copies at hand:
Descent 2 had separate 3dfx, ViRGE and Vérité releases, and whether they
kept the VFX1's head tracker isn't known. Without a game that has both,
it would be stereo without the head tracker. It hasn't been done.
