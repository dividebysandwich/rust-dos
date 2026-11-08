# Copy-protected games

Rust-DOS runs a game's copy protection as it is, and the protection
checks the disc image as it would check the disc in a real drive. That
works with images of discs you own: a full rip of your original CD, made
with a tool that keeps what the protection checks. Rust-DOS doesn't
patch or skip a protection, and this page says only which images keep a
disc as it is.

## Do this

1. Find the game's protection in the table below, and the image it
   needs.
2. Make that image of your disc with one of the tools in [Dumping a
   disc](#dumping-a-disc). If you don't know the protection, make the
   most complete image: BlindWrite or Alcohol 120% with subchannel data
   and data position measurement (DPM), which keeps all that the
   protections in the table check.
3. Mount the image as the game's CD (see [Mounting the
   image](#mounting-the-image)), with every file the dump made beside it.
4. Start the game. If the check fails, see [When the check
   fails](#when-the-check-fails).

## Which protections run

| Protection | Built-in Windows | Booted Windows 95 or 98 image | The image it needs |
|---|---|---|---|
| SafeDisc 1 | The check passes (Discworld Noir) | Not tried | Whole 2352-byte sectors: BIN and CUE, CHD, CloneCD, or BlindWrite or Alcohol 120% in raw mode |
| SafeDisc 2 | Passes (Oni) | Passes (Anachronox, SafeDisc 2.30) | The same as SafeDisc 1 |
| SecuROM 4.8 | The check passes (Grom, SecuROM 4.84) | Not tried | BlindWrite or Alcohol 120% with the subchannel and DPM data |
| Other SecuROM versions, StarForce, LaserLock, CD-Cops and the rest | Not tried | Not tried | |
| Code wheels, manual lookups | Yes | Yes | The manual or wheel as a PDF or picture (see [Manuals and code wheels](#manuals-and-code-wheels)) |

"The check passes" means the game gets past its disc check and stops
later for another reason, which has an issue of its own:

- **Discworld Noir** needs a KERNEL32 function the built-in Windows
  doesn't have yet
  ([#812](https://github.com/dividebysandwich/rust-dos/issues/812)).
- **Grom** then asks Direct3D for 32-bit textures, which a Voodoo3
  doesn't have, on a real one either: the game's minimum is a later card
  ([#645](https://github.com/dividebysandwich/rust-dos/issues/645)).

**Anachronox** checks disc 2, as its readme says: keep disc 2 in the
drive while playing. With disc 1 SafeDisc says it can't find the CD, on
a real PC too.

Why ISO images fail: SafeDisc's discs have sectors mastered to be
unreadable, and the protection refuses to run when reading them
succeeds. An ISO keeps only the 2048 bytes of data of each sector, so
those sectors read back as good ones and the check fails, as it would on
Windows 98. An image of whole 2352-byte sectors keeps each sector's
error-detection code (EDC), and rust-dos reads a sector whose EDC doesn't
match as the drive did: as an error.

SecuROM 4.8 also reads the subchannel, where its discs have entries
mastered with errors, and times reads to check where sectors lie on the
disc. A CUE sheet with BIN files keeps neither the measured positions
nor, without a `.sub` or `.subcode` file beside it, the subchannel. Only
BlindWrite's and Alcohol 120%'s images carry DPM data, the positions
their dump measured.

## DOS games

DOS games read their CDs through MSCDEX. A sector whose EDC doesn't
match reads as a CRC error there, and a file read over it as a data
error, as on a real drive. MSCDEX's reads aren't timed, so a DOS
protection that times the drive doesn't see a real drive's times.

Floppy protections need an 86Box `.86f` image, which keeps sectors with
bad CRCs and missing sectors. There is no floppy controller: protections
that depend on its timing, on sectors of other sizes or on weak bits
don't pass ([disk images](../CONFIGURATION.md#disk-images)).

## Manuals and code wheels

Games that ask for a word from the manual or a symbol on a code wheel
need the manual or wheel. Put a PDF or a picture of it in the game's
profile, and Ctrl+Shift+M shows it over the game:
[manuals and extras](../CONFIGURATION.md#manuals-and-extras) and
[GAME-PACKAGES.md](../GAME-PACKAGES.md).

## Dumping a disc

| Tool | What it makes | Good for |
|---|---|---|
| redumper or DiscImageCreator | CUE, BIN and a `.subcode` or `.sub` subchannel file | SafeDisc |
| CloneCD | `.ccd`, `.img` and `.sub` (discs of one session) | SafeDisc |
| BlindWrite 5 or 6, with subchannel and DPM | `.b5t` or `.b6t`, its data files, and a `.bwa` file with the DPM data | Everything in the table |
| Alcohol 120%, with subchannel and DPM | `.mds` and `.mdf` | Everything in the table |
| chdman (`chdman createcd`) from a CUE and BIN dump | `.chd` | SafeDisc; rust-dos reads no subchannel from a CHD |

Use the tool's settings for an exact copy of the disc: raw sectors of
2352 bytes, the subchannel, and DPM where the tool has it. Keep every
file the tool makes together, under the names it gave them.

Rust-DOS reads Alcohol 120% descriptors of version 1, of CDs, with the
tracks in one `.mdf` file
([#648](https://github.com/dividebysandwich/rust-dos/issues/648) has the
others). [CD images](../CONFIGURATION.md#cd-images) lists every format
and what each keeps.

## Mounting the image

Mount the descriptor or CUE sheet, not the data file:

```ini
[drives]
D=~/cds/grom.mds cdrom
```

At the DOS prompt, `MOUNT D ~/cds/grom.mds` does the same. For a game on
the built-in Windows, that drive line goes in the game's profile
([Windows games](../CONFIGURATION.md#windows-games)). A booted Windows
image gets the CD in its IDE CD-ROM drive
([booting a disk image](../CONFIGURATION.md#booting-a-disk-image)).

Keep the CD on the drive letter it had when the game was installed: the
game looks for its disc there ([the CD's drive letter and
label](../CONFIGURATION.md#the-cds-drive-letter-and-label)).

## When the check fails

1. **An ISO, or an image made from one:** dump the disc again in one of
   the formats the table names for its protection.
2. **The wrong disc:** a game on several CDs may check only one of them,
   as Anachronox does disc 2. Its readme says which.
3. **A file missing beside the image:** a `.sub` or `.subcode` file has
   to have the CUE sheet's name or a data file's; BlindWrite's and
   Alcohol's data files and the `.bwa` file stay beside the descriptor.
4. **The other Windows:** a game that fails on the built-in Windows may
   pass on a Windows 98 image, and the other way round.
5. **On the built-in Windows, a message naming a function or DLL:** the
   check may have passed, and the game stops at something the built-in
   Windows doesn't have yet. The last `[W32]` lines of the [log
   file](../README.md#log-file) name it.

If it still fails,
[open an issue](https://github.com/dividebysandwich/rust-dos/issues)
that names the game, its release and region, the protection and its
version if you know them, the tool and format you dumped with, which
Windows it ran on, and the last lines of the log. Don't attach the disc
image or anything taken from the disc.

Known limits that a protection could run into:

- On the built-in Windows, ASPI requests without an event finish at
  once, in no emulated time
  ([#636](https://github.com/dividebysandwich/rust-dos/issues/636)).
- A CloneCD image has to be of one session, and a CHD's subchannel isn't
  read.
