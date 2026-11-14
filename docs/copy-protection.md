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
   disc](#dumping-a-disc): redumper for most discs, and disc-archiver
   with redumper for a SecuROM disc. If you don't know the protection,
   make the most complete image: Alcohol 120% with subchannel data and
   data position measurement (DPM).
3. Mount the image as the game's CD (see [Mounting the
   image](#mounting-the-image)), with every file the dump made beside it.
4. Start the game. If the check fails, see [When the check
   fails](#when-the-check-fails).

[compatibility.md](compatibility.md) has what particular games need
beyond this, such as which of their discs to mount.

## The image each protection needs

| Protection | The image it needs |
|---|---|
| SafeDisc 1 and 2 | Whole 2352-byte sectors: redumper's BIN and CUE, CHD, or Alcohol 120% in raw mode. An ISO fails. |
| SecuROM 4.8, and any other disc protection | Alcohol 120% with the subchannel and DPM data, or disc-archiver's `.mds` with redumper's `.subcode` beside it |
| Code wheels, manual lookups | The manual or wheel as a PDF or picture (see [Manuals and code wheels](#manuals-and-code-wheels)) |

What each image keeps:

- An ISO keeps only the 2048 bytes of data of each sector. SafeDisc's
  discs have sectors mastered to be unreadable, and they read back from
  an ISO as good ones, so the check fails, as it would on a real PC.
- An image of whole 2352-byte sectors keeps each sector's
  error-detection code, and rust-dos reads a sector whose code doesn't
  match as an error, as the drive did.
- SecuROM 4.8 also reads the subchannel and times reads to find where
  sectors lie on the disc. A CUE sheet keeps the subchannel only with a
  `.sub` or `.subcode` file beside it, and never the sectors' positions.
  An Alcohol 120% image made with DPM keeps the positions, and so does
  disc-archiver's `.mds`. An `.mds` keeps the subchannel in its `.mdf`
  when Alcohol 120% made it with subchannel, or with a `.sub` or
  `.subcode` file beside it.

## DOS games

On the built-in DOS, games read their CDs through MSCDEX. A sector whose
error-detection code doesn't match reads as a CRC error there, and a file
read over it as a data error, as on a real drive. MSCDEX's reads aren't
timed, so a DOS protection that times the drive doesn't see a real
drive's times.

Floppy protections need an 86Box `.86f` image, which keeps sectors with
bad CRCs and missing sectors. The built-in DOS has no floppy controller,
only INT 13h, so protections that depend on the controller's timing
don't pass there. A [booted
system](../CONFIGURATION.md#booting-a-disk-image) has one, which reads
only 512-byte sectors. Sectors of other sizes aren't kept, and weak bits
read the same each time, on either ([disk
images](../CONFIGURATION.md#disk-images)).

## Manuals and code wheels

Games that ask for a word from the manual or a symbol on a code wheel
need the manual or wheel. Put a PDF or a picture of it in the game's
profile, and Ctrl+Shift+M shows it over the game:
[manuals and extras](../CONFIGURATION.md#manuals-and-extras) and
[GAME-PACKAGES.md](../GAME-PACKAGES.md).

## Dumping a disc

Rip the discs you own and obtained legally with
[redumper](https://github.com/superg/redumper), the dumper Redump uses.
It reads every sector raw, with the disc's subchannel, and writes a CUE
sheet and BIN files beside its own files (`.scram`, `.subcode`, `.toc`,
`.log` and others). It reads a CD completely only in a drive it knows:
`redumper --list-recommended-drives` lists them. Mount the CUE sheet,
with the `.subcode` file beside it under the name redumper gave it. That
image keeps what SafeDisc checks, and the subchannel.

redumper doesn't measure DPM, which SecuROM 4.8 and later check. For a
SecuROM disc, use
[disc-archiver](https://github.com/kibertoad/refurbished-dinosaurs-toolkit/tree/main/packages/disc-archiver)
with redumper. It dumps the disc with redumper, measures the DPM data in the
same drive (on Windows only), and writes it into an Alcohol 120% `.mds`: its
`mds` copy (`.mds` and `.mdf`), and an `.mds` beside the BIN files of its
`bincue-split` copy. The `.mds` holds no subchannel, and redumper's
`.subcode` stays in the dump's `archival` folder. Copy the `.subcode` beside
the `.mds` (`bincue-split/<name>.subcode` or `mds/<name>.subcode`), keeping
its name, and mount the `.mds`. The image then has both the DPM data and the
disc's subchannel. Rust-dos looks for the subchannel file only beside the
`.mds`, named as the `.mds` or one of its data files, with `.subcode` or
`.sub`. The log file says which file it reads, or why it reads none it
found. disc-archiver's README has the commands and says when it measures.

| Tool | What it makes | Good for |
|---|---|---|
| [redumper](https://github.com/superg/redumper) | CUE, BIN and a `.subcode` subchannel file | SafeDisc, and any disc whose check reads only sectors and subchannel |
| [disc-archiver](https://github.com/kibertoad/refurbished-dinosaurs-toolkit/tree/main/packages/disc-archiver) with redumper | redumper's dump, and an `.mds` with the DPM data; with redumper's `.subcode` copied beside it, the subchannel too | SecuROM 4.8 and later: the DPM data and the subchannel |
| [Alcohol 120%](https://www.alcohol-soft.com/), with subchannel and DPM | `.mds` and `.mdf` | Every protection in the table, both in one image |
| [chdman](https://docs.mamedev.org/tools/chdman.html), from a CUE and BIN dump | `.chd` | SafeDisc; rust-dos reads no subchannel from a CHD |

Each tool's own documentation says how to make an exact copy with it:
raw sectors of 2352 bytes, the subchannel, and DPM where the tool has
it. Keep every file the tool makes together, under the names it gave
them.

Rust-DOS reads Alcohol 120% descriptors of version 1, of CDs, with each
track in one data file beside the descriptor.

Images you already have from BlindWrite 5 or 6 (`.b5t` or `.b6t`, with
their data files and `.bwa` DPM file), CloneCD (`.ccd`, `.img` and
`.sub`, one session) or DiscImageCreator (CUE, BIN and `.sub`) work too:
a BlindWrite image with subchannel and DPM keeps what Alcohol 120%'s
does, and a CloneCD or DiscImageCreator image what redumper's does.
[CD images](../CONFIGURATION.md#cd-images) lists every format and what
each keeps.

## Mounting the image

Mount the descriptor or CUE sheet, not the data file:

```ini
[drives]
D=~/cds/game.mds cdrom
```

At the DOS prompt, `MOUNT D ~/cds/game.mds` does the same. For a game on
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
2. **The wrong disc:** a game on several CDs may check only one of them.
   Its readme or [compatibility.md](compatibility.md) says which.
3. **A file missing beside the image:** a `.sub` or `.subcode` file has
   to have the name of the CUE sheet, the `.mds` or a data file, and an
   image's data files (and BlindWrite's `.bwa` file) stay beside its
   descriptor. A subchannel file that doesn't cover the whole disc isn't
   read; the log file says so.
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
