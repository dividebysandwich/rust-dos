# Game manuals and game packages

Many DOS games ask a question from their manual before they let you play:
a word on a page, a symbol on the code wheel, a location on the map.
Rust-DOS can show the manual, code wheel, maps and reference cards over the
game while it is paused, so you don't have to leave the game to look the
answer up.

This guide covers:

1. [Using the manual viewer](#using-the-manual-viewer)
2. [Adding manuals to a game](#adding-manuals-to-a-game): the files and
   folders Rust-DOS looks in
3. [Game packages](#game-packages): one zip, 7z archive or folder with the
   game, its CD, its settings and its manuals, which Rust-DOS plays as it is
4. [Building a package](#building-a-package), step by step
5. [Using a package](#using-a-package)

The manual viewer works with [game profiles](CONFIGURATION.md#game-profiles),
the games listed on the settings window's **Games** page.

## Using the manual viewer

| Key | Action |
|---|---|
| Ctrl+Shift+M | Show the running game's manuals, and hide them again |
| M (on the Games page) | Show the selected game's manuals |

If a game has one manual, it opens straight away. If it has more, you get a
list: Enter opens one, Ins adds a file, Esc goes back.

While a manual is open the game is paused, and these keys turn and move
the pages:

| Key | Action |
|---|---|
| PgDn, Right, Space | Next page |
| PgUp, Left | Previous page |
| Down, Up, mouse wheel | Scroll down and up |
| Home, End | First and last page |
| `+`, `-` | Zoom: the whole page, its full width, 150%, 200% |
| `]`, `[` | Scroll right and left, when zoomed in |
| Esc | Back to the list, or back to the game |
| F1 | Help |

In the Rust-DOS window, the page is drawn at the window's own resolution,
so small print stays readable even when the game runs at 320x200. In
RetroArch, the page is drawn into the game's picture, at the picture's
resolution; zoom in to read small print. In both, the page keeps its
proportions on the stretched 4:3 picture.

The viewer shows:

* **PDF documents** (`.pdf`): scanned manuals and the ones with text.
  Password-protected PDFs can't be opened.
* **Pictures** (`.png`, `.jpg`, `.jpeg`, `.gif`): code wheels, maps,
  reference cards. A GIF shows its first frame.

The browser build of Rust-DOS has no manual viewer, because it can't read
the files on your computer.

## Adding manuals to a game

Rust-DOS finds a game's manuals in three places, and lists them in this
order:

1. The `manual=` lines in the `[game]` section of the game's profile.
2. The files in the game's extras folder, `games/<profile>.extras`.
3. The files in the `EXTRAS` folder of a [game package](#game-packages)
   the game is played from.

### Where the files are

Profiles are in the `games` folder next to your configuration file
(`rust-dos.conf`). A game's saves and other changes are in the `saves`
folder next to that (see [write overlays](#saves-and-the-write-overlay)):

```
rust-dos.conf
games/
    keen4.conf              the profile of the game "keen4"
    keen4.extras/           its extras folder: every PDF or picture in it
        Manual.pdf          is one of its manuals
        Hint Book.pdf
saves/
    keen4/
        C/                  what the game changed on drive C:
```

In RetroArch, all of this is in the core's folder in the frontend's save
directory: `saves/rust-dos/games` and `saves/rust-dos/saves`.

### The quickest way: the extras folder

Make a folder named after the profile with `.extras` added, next to the
profile, and put the files in it. For the profile `games/keen4.conf`,
that's `games/keen4.extras/`. Each PDF or picture in it is listed under its
file name without the extension: `Code Wheel.png` is listed as "Code
Wheel". The files are listed in alphabetical order. Subfolders and other
kinds of file are ignored.

### `manual=` lines

To give a file another title, or keep it somewhere else, add a `manual=`
line to the profile's `[game]` section for each file:

```ini
[game]
name=Pool of Radiance
manual=pool/Rule Book.pdf|Rule book
manual=pool/Adventurer's Journal.pdf|Journal
manual=~/scans/pool-codewheel.png|Code wheel
```

The path comes first, then optionally `|` and the title to list it as.
Relative paths are relative to the `games` folder, and `~` is your home
directory. A path can point into a zip or 7z archive, such as
`pool.zip/EXTRAS/Rules.pdf`, and the file is read from the archive.

You can also add a file without editing the profile: open the game's
manuals (M on the Games page), choose *+ Add a PDF or picture...* or press
Ins, and pick the file. Rust-DOS adds a `manual=` line for it to the
profile.

### Tips for preparing the files

* **Scans:** 150 to 200 dpi is plenty for reading on screen. Rust-DOS
  draws a page at most 4096 pixels on its longest side and scales it from
  there.
* **Code wheels:** a photo or scan of the whole wheel works. If the game
  asks you to line up rings, separate pictures of each ring are easier
  to use.
* **One file per thing:** "Manual", "Code wheel" and "Map" in the list are
  quicker to find than one PDF with everything in it.

## Game packages

A game package is a single zip archive, 7z archive or folder that holds
everything a game needs: its files, its CD or floppy images, its settings,
the commands that start it, and its manuals. Rust-DOS plays it as it is:
the package is never unpacked or changed, and anything the game writes
goes to the game's saves folder.

A package looks like this:

```
Pool of Radiance.zip
    rust-dos.conf           settings and start commands (optional)
    START.EXE               the game's own files...
    DATA/
    CD/                     ...a CD as a folder or an image (optional)
        POOL.CUE
        POOL.BIN
    EXTRAS/                 manuals, code wheels, maps (optional)
        Rule Book.pdf
        Code Wheel.png
```

The package becomes drive C:. Rust-DOS doesn't care about upper and lower
case in the names `rust-dos.conf`, `EXTRAS` and the files the game opens.

**One folder inside.** If everything in an archive is inside one folder
(`Pool of Radiance/START.EXE`, `Pool of Radiance/rust-dos.conf`, and so
on), that folder is the package's root, and C:. This is how most zip
programs pack a folder, so either way works.

### The package's `rust-dos.conf`

`rust-dos.conf` at the package's root is an ordinary [game
profile](CONFIGURATION.md#game-profiles). All its sections are optional,
and every relative path in it is relative to the package's root:

```ini
[game]
name=Pool of Radiance
manual=EXTRAS/Rule Book.pdf|Rule book

[emulator]
cycles=8000
memsize=4

[sound]
sbtype=sb16

[drives]
D=CD/POOL.CUE

[autoexec]
C:
CALL INSTALL.BAT
START
```

* **`[game]`:** `name=` is the name on the Games page. Without one, the
  package's file name is used. `manual=` lines list manuals in the
  package, with titles. The files in `EXTRAS` are listed without them.
  `overlay=false` makes a folder package writable in place; see [saves
  and the write overlay](#saves-and-the-write-overlay). `achievements=`
  is the [RetroAchievements](CONFIGURATION.md#retroachievements) hash;
  for a zip, Rust-DOS adds it by itself.
* **Settings sections** (`[emulator]`, `[sound]`, `[joystick]` and the
  rest) are the game's settings, applied over yours while it runs. List
  only what the game needs; [CONFIGURATION.md](CONFIGURATION.md) has every
  setting.
* **`[drives]`:** C: is the package unless you set it. Other drives can
  be in the package: a folder (`D=CD cdrom` makes the folder `CD` a CD-ROM
  drive) or a disk or CD image (`D=CD/POOL.CUE`, `A=DISKS/DISK1.IMG`,
  `A=DISKS/DISK1.IMG DISKS/DISK2.IMG` for a list that Ctrl+F4 steps
  through). The mount options are those of
  [`[drives]`](CONFIGURATION.md#drives).
* **`[autoexec]`:** the commands that start the game. Without them, C: is
  the current drive, and if the package's root holds a single program
  (setup and install programs aside), that program runs. To run a batch
  file and then more commands, start it with `CALL`, as in DOS: without
  `CALL`, the batch file ends the list.

Without a `rust-dos.conf`, a zip or 7z archive still works: it becomes C:
and its single program starts, with the default settings. A folder needs
the `rust-dos.conf` to count as a package; without it, a dropped folder is
just mounted as a drive.

### Saves and the write overlay

The package is never written to. Anything the game writes, renames or
deletes goes to `saves/<profile>/<drive letter>` next to the `games` folder,
and the game sees the package with those changes on top. That folder holds
the game's saved games and setup changes, so it's the one to back up or
sync between computers. R on the Games page deletes it, putting the game
back to how the package has it.

For a folder package, `overlay=false` in `[game]` lets the game write into
the folder itself. An archive can't be written to, so its changes always go
to the saves folder.

## Building a package

1. **Put the game in a folder.** Copy the installed game, the way it
   runs from C:, into a new folder named after the game.
2. **Add the CD, if there is one.** Copy the CD image into it, for
   example into a `CD` folder. A CUE sheet with its BIN files, an ISO, or
   the CD's files as a folder all work.
3. **Add the manuals.** Make an `EXTRAS` folder and put the PDFs and
   pictures in it, named the way they should be listed.
4. **Write `rust-dos.conf`.** Set the game's name, the settings it needs,
   the CD drive and the commands that start it (see [above](#the-packages-rust-dosconf)).
5. **Try it as a folder.** Drop the folder onto the Rust-DOS window. It
   becomes a game on the Games page and starts. Check that the game runs,
   finds its CD, and that Ctrl+Shift+M shows the manuals. While you're
   still changing things, put `overlay=false` in `[game]` to see the game's
   files change in the folder, and take it out again before packing. The
   profile is made from `rust-dos.conf` once: after changing it, delete
   the game on the Games page (Del) and drop the folder again.
6. **Pack it.** Zip the folder:

   ```sh
   cd ~/packages
   zip -r -0 "Pool of Radiance.zip" "Pool of Radiance"
   ```

   or make a 7z archive:

   ```sh
   7z a -ms=off "Pool of Radiance.7z" "Pool of Radiance"
   ```

   On Windows, *Send to > Compressed (zipped) folder* works too.

**Compression:** Rust-DOS reads stored (uncompressed) zip files straight
from the archive. A compressed file is unpacked into memory the first time
the game opens it, and stays there. That's fine for game files, but a
650 MB CD image takes a lot of memory and time. Store CD and disk images
uncompressed: `zip -0` stores everything, and `zip -n .bin:.iso:.img`
stores just those files. In a 7z archive, every file in a solid block is
unpacked together, so turn solid mode off (`-ms=off`). Zip archives are
the better choice for big games, and they're what RetroAchievements knows
games by.

Zip64 archives (bigger than 4 GB) work. Encrypted archives don't.

## Using a package

Every way of opening a package makes a game profile for it in the `games`
folder the first time, and finds that profile again after:

* **Drop it** onto the Rust-DOS window. The game starts.
* **On the command line:** `rust-dos --import "Pool of Radiance.zip"`
  makes the profile and starts the game. After that, `rust-dos --game
  "Pool of Radiance"` starts it.
* **On the Games page:** choose *+ Import a game's package, a GOG game or
  DOSBox .conf...* and pick the package. Enter starts the game.
* **In RetroArch:** load the zip, 7z archive or folder as content.

The profile points at the package where it is, so keep the package there.
If you move the package, delete its profile (Del on the Games page) and
open it again; the saves stay, as long as the profile's name doesn't
change.

The profile is made once from the package's `rust-dos.conf`. If you change
the package's `rust-dos.conf` later, delete the profile and open the
package again to pick up the changes. The `EXTRAS` folder is read every
time the manuals are shown, so new files there show up straight away.

You can also mount a package, or a folder or image inside one, by hand:

```
MOUNT C "~/packages/Pool of Radiance.zip" -overlay ~/saves/pool
MOUNT D "~/packages/Pool of Radiance.zip/CD/POOL.CUE"
```

Without `-overlay`, a drive from an archive is read-only.
