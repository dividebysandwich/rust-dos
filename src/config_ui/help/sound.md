# Sound Blaster {#sb-type}

The sound card nearly every DOS game supports. **SB16** works for almost
everything; pick **SB Pro 2** or **SB 2.0** for an older game that
doesn't recognise the SB16, or **none** to remove the card.

**SB AWE32** is an SB16 with Creative's EMU8000 wavetable synthesizer,
which games with an *AWE32* music option (Doom, Rise of the Triad, many
Miles and HMI games) play their music on. It needs the AWE32
ROM for its instruments.

In a game's setup program, choose *Sound Blaster* (or *Sound Blaster 16*)
for both sound effects and music, and keep the settings it detects.

# Sound Blaster port, IRQ and DMA {#sb-ports}

Where the Sound Blaster sits in the PC. A game's setup asks for these:
give it the same values as here, **220**, IRQ **7**, DMA **1** and HDMA
**5** unless you changed them.

Leave them as they are unless a game only works with other values.
Left and Right pick a field, Enter lists its values.

# AWE32 ROM {#awe32-rom}

The AWE32's 1 MB sample ROM, `awe32.raw`, with the General MIDI
instruments its music plays with. It is Creative's and doesn't come with
Rust-DOS. Enter picks the file, Del goes back to **default**, which looks
in Rust-DOS's configuration directory and the working directory.

Without it the card still works: games that load their own instruments
(Impulse Tracker, Cubic Player) play, but the ROM's instruments are
silent. *Download the AWE32 ROM...* fetches the copy the libretro PCem
core keeps, checks it, and puts it in the configuration directory.

# AWE32 RAM {#awe32-ram}

The sample memory on the card for instruments programs load: **512 KB**
as the AWE32 came, up to 28 MB as it could be fitted with.

# FM synthesizer {#opl}

The chip that plays the music of Sound Blaster and AdLib games.
**OPL3** plays both kinds of music; **OPL2** is the older chip of the
AdLib and first Sound Blasters, for the few games that sound wrong on an
OPL3.

# Gravis Ultrasound {#gus}

A second sound card with wavetable music, which many demos and some games
sound best with (Epic Pinball, Jazz Jackrabbit, Star Control 2). Choose
*Gravis Ultrasound* in their setup. **on** does no harm to games that
don't use it.

# Ultrasound port, IRQ and DMA {#gus-ports}

Where the Ultrasound sits in the PC: port **240**, IRQ **5**, DMA **3**
unless you changed them. Give a game's setup the same values. Left and
Right pick a field, Enter lists its values.

# Ultrasound software drive {#gus-drive}

The drive letter of the Ultrasound's instrument sounds (patches), which
are built into Rust-DOS. **X:** is fine unless you need that letter for
something else. **none** leaves it out, for patches of your own.

# ULTRADIR {#ultradir}

The DOS folder with the Ultrasound's patches, which games look in. The
default is on the *Software drive* and just works. Only change it if you
put your own patches somewhere, with the *Software drive* set to **none**.

# MIDI synthesizer {#midi}

What plays the music of games set up for *General MIDI*, *Roland* or
*MPU-401*:

- **auto**: the *SoundFont* if you picked one, else the built-in
  Ultrasound instruments. Works out of the box.
- **SoundFont**: a SoundFont file (`.sf2`) of your choice: the best
  General MIDI sound.
- **MT-32 (munt)**: an emulated Roland MT-32, for Sierra, LucasArts and
  Origin games of 1988-1992, which sound best on one. Needs the MT-32 ROMs.
- **host MIDI port**: a synthesizer or MIDI device of your computer.

In the game's setup, pick *General MIDI* (or *Roland MT-32* with the
MT-32) at port **330**.

# SoundFont {#soundfont}

A General MIDI SoundFont file (`.sf2`), used for MIDI music. Free ones
such as *GeneralUser GS* or *FluidR3_GM* sound great with DOS games.
Enter picks the file, Del goes back to none.

# MT-32 ROMs {#mt32-roms}

The folder with the Roland MT-32's (or CM-32L's) ROM files, which the
MT-32 needs and which don't come with Rust-DOS. Any file names work.
Enter picks the folder; **default** looks in the usual places, such as
DOSBox's `mt32-roms`.

# MT-32 model {#mt32-model}

Which ROMs to play with. **auto** takes the CM-32L's if there, else the
MT-32's. A few early games only sound right on the **MT-32**.

# MIDI port {#midi-port}

The MIDI port of your computer that *host MIDI port* sends music to: a
real synthesizer, or a program such as FluidSynth or munt.

# Parallel port DAC {#lpt-dac}

A simple sound device on the printer port, which some games of the late
1980s and early 1990s play speech and sound effects through:

- **Disney Sound Source**: pick it where a game offers it.
- **Covox Speech Thing**: for games that offer a *Covox* or *LPT DAC*.

Most games sound better on the Sound Blaster; leave it at **none**
unless a game has nothing better.

# Tandy/PCjr sound {#tandy-sound}

The three-voice sound chip of the Tandy 1000 and PCjr. **auto** has it
with those video cards. **on** adds it to any machine, for games that
offer *Tandy* sound with VGA graphics.

# Hard disk noise {#hard-disk-noise}

Plays the sounds of a hard disk: **seek-only** the heads moving on each
access, **on** also its spinning and humming. For the feel of an old PC;
best with a *Hard disk speed* below **maximum**.

# Floppy disk noise {#floppy-disk-noise}

Plays the sounds of a floppy drive: **seek-only** the heads moving,
**on** also its motor. Best with a *Floppy disk speed* below **maximum**.
