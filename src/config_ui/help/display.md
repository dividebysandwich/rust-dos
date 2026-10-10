# Window scale {#scale}

How many times bigger than the DOS picture the window is. A game at
320x200 shows in a 640x400 window at **2x**, and 960x600 at **3x**.

Pick the biggest that fits your screen. For a full screen, use
*Fullscreen* instead.

> The CRT shaders look best at **3x** or more.

# Fullscreen {#fullscreen}

Fills the whole screen with the picture, keeping its shape, with black
bars at the sides where needed.

**Alt+Enter** switches between fullscreen and a window at any time.

# Fixed aspect ratio {#aspect}

Choose the shape to show the picture in: **none** keeps its native pixel
ratio, while **4:3**, **5:4**, **16:10** and **16:9** expand it to fit
that display ratio without cropping. DOS games drawn for CRT monitors
usually look right at **4:3**.

Choose **none** for perfectly square, sharp pixels.

# Variable refresh rate (VRR) {#vrr}

VGA games run at **70 Hz**, and some modes at 60 Hz or other rates. A
60 Hz display drops one frame in seven of 70, which makes scrolling
judder. Turn this **on** with a G-Sync or FreeSync display, and each
frame shows when it is due: the display runs at the game's own rate.

VRR must be on for the display and for windows: in the graphics
driver's settings, or with *adaptive sync* on in the desktop. Leave this
**off** with a fixed-rate display.

# Scaling filter {#filter}

How the picture is blown up to the window's size:

- **nearest**: sharp, blocky pixels.
- **linear**: smooth, slightly soft pixels.

With a fixed aspect ratio, or at an odd window size, **linear**
avoids uneven pixel sizes. It has no effect while a CRT shader is on.

# CRT shader {#shader}

Makes the picture look like an old monitor:

- **none**: the plain picture.
- **scanlines**: the dark lines between the rows of a VGA monitor.
- **aperture grille**: scanlines with red, green and blue phosphor stripes.
- **CRT**: a curved glass tube with rounded corners and a glow.

Purely a matter of taste; games run the same either way. They look best
at a *Window scale* of **3x** or more, or in fullscreen.

# CRT curvature {#crt-curvature}

How far the **CRT** shader bends the screen: **0%** is flat, **100%** a
strongly curved tube. Left and Right change it.

# CRT glow {#crt-glow}

How much light glows around the bright parts of the picture with the
**CRT** shader: **0%** is none, **100%** the most.

# Monochrome monitor {#monochrome}

Shows the picture on a one-colour monitor: **white**, **amber** or
**green**, as on many PCs of the 1980s. **off** is a colour monitor.

With a VGA or EGA video card, games that support a monochrome screen see
one too (from the next DOS prompt on) and pick their best graphics for it.

# CGA composite colour {#composite}

Many 1980s games (King's Quest, Ultima, Sierra's AGI games) were made
for a CGA card on a TV or composite monitor, which turned fine patterns
of pixels into extra colours.

- **auto**: shows those colours when a game asks for them. Best for most.
- **on**: always shows them in graphics modes.
- **off**: the plain 4-colour RGB picture.

Only matters with *Video card* set to **CGA**. If a game has a
"composite" or "RGB" choice at its start, pick composite.

# CGA revision {#composite-era}

Which CGA card makes the composite colours. **old** (IBM's first CGA) is
what the classic games were drawn for; **new** gives slightly different
colours.
