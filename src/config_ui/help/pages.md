# Drives {#drives}

The drives DOS sees. Most games need just one: a folder on your computer
mounted as **C:**, with the game in it.

- **Ins** (or *+ Mount a drive*) mounts a folder, a CD image (`.cue`,
  `.iso`) or a floppy or hard disk image as a new drive.
- **Enter** changes what a drive holds: this is how to put in the next
  CD or floppy when a game asks for it.
- **Del** unmounts a drive.
- **B** boots from a floppy or hard disk image, as `BOOT` does: for
  Windows 95, 3.1 or a game on its own boot disk.

A drive marked *boot* boots when Rust-DOS starts, in place of DOS: set
it with *Auto-boot* under **Enter**. One marked *ovl* keeps its folder or
archive as it is, its changes in the folder *Changes to* under **Enter**
names.

A system booted from an image, such as Windows 95, sees disks, not your
folders. A folder on D: or later marked *share* becomes one of its hard
disks when it boots (*81h* and so on while it runs), and what it changes
there goes back into the folder when it shuts down. **S** copies its new
and changed files over while it runs. A folder mounted as a **cdrom**
goes into its CD-ROM drive as a disc (marked *CD*), at any time; **R**
puts it in again with what the folder holds now.

Mounts made here last until Rust-DOS quits; **F2** saves them for the
next start. **Ctrl+F4** puts in the next disk of a drive mounted with
several images.

> You can also drop a game's folder, a CD image or a zip file onto the
> window.

# Mount a drive {#mount}

- **Drive**: the letter DOS sees it as. **C:** for the game's folder,
  **D:** for its CD, **A:** for floppies.
- **Path**: a folder, a zip or 7z archive, or an image file: a CD
  (`.cue`, `.iso`, `.bin`), a floppy or a hard disk image (`.img`).
  *Browse...* picks one. On Windows the buttons over its list, or
  **Left** and **Right**, go to the other drives.
- **Type**: **cdrom** for a game CD, so the game finds its disc;
  **floppy** for floppy disks; **hdd** for the rest.
- **Label**: the volume name. Some games check their CD's label; leave it
  empty to use the image's own.
- **Read-only**: keeps programs from changing the files.
- **Write to**: it set, any changes are written to this location; 
  delete the folder to undo it all.
- **Auto-boot**: for a floppy or hard disk image, boot from it when
  Rust-DOS starts instead of starting DOS. Only one drive boots; **F2**
  saves it.

**Tab** goes to the next field, **Enter** mounts. *[ Boot ]* mounts the
image and boots from it right away.

# Create a disk image {#new-image}

Makes a new, empty floppy or hard disk image file and mounts it, for
instance to install Windows or a game onto, or as a blank disk a game
wants to save to.

- **File**: where the image goes on your computer.
- **Type**: a floppy size, or a hard disk.
- **Size**: a hard disk's size in MB.
- **Mount as**: the drive letter it gets.

# Games {#games}

Game profiles: each game's own settings, drives and start command,
launched with **Enter**.

- *+ New game from the current settings*: mount the game's folder first,
  get it running, then make a profile of it: it keeps what you changed.
- *+ Import a game's package, a GOG game or DOSBox .conf*: turns a
  game's package, a game bought on GOG or one set up for DOSBox into a
  profile with its settings, CD and manuals.

While a game plays, **F2** saves changed settings to its profile only.
**Del** deletes a profile (not the game's files).

**M** shows the game's manuals.

A game's own drives keep its files as installed: what it writes, such
as saved games, goes to `saves` beside the games folder. **R** deletes
that, taking the game back to how it was installed.

# Manuals {#manuals}

A game's manuals and extras: its manual, code wheel, maps and reference
cards, as PDF documents or PNG, JPEG or GIF pictures, for the copy
protection's questions. **Ctrl+Shift+M** shows the running game's.

- **Enter** opens one; **Ins** adds one, which goes in the profile.
- Opened again, the manual open last comes back on its page.
- The files in `games/<game>.extras` are listed too.
- **PgUp**/**PgDn** turn the pages, **Up**/**Down** scroll, **+**/**-**
  zoom, **[** and **]** scroll sideways.

# New game {#new-game}

Makes a profile of the settings as they are now, and the drives you
mounted:

- **Name**: what the Games page lists it as.
- **Directory**: the DOS folder the game is in, such as `C:\KEEN4`.
- **Command**: the program that starts it, such as `KEEN4E`.

# Import a game {#import}

Pick a game's package (a zip or 7z archive, or a folder with a
`rust-dos.conf`), the folder of a GOG game (the one with a
`goggame-*.info` file), or a DOSBox `.conf` file. The game becomes a
profile with its settings, drives, CD images and manuals, and **Enter**
on the Games page launches it.

# States {#states}

Save states store the whole game at one moment, so you can save anywhere,
even in games that don't let you.

- **Ins** saves to the slot selected, **Enter** loads it, **Del** empties
  it.
- **Ctrl+F1** saves and **Ctrl+F2** loads without opening this window;
  **Ctrl+F3** picks the next slot.

Each game profile has slots of its own.

# Cheats {#cheats}

Finds a number in the game's memory, such as lives or money, and changes
it:

- *Value size*: **8-bit** fits most counters; **16-bit** or **32-bit**
  for money or scores above 255.
- *Search in*: **conventional memory** for most games, **all memory**
  for DOS/4GW and other protected-mode games.
- Type the value the game shows (say, 3 lives) in *New search*, and press
  **Enter**.
- Close the window, play until the value changes (lose a life), open it
  again, and at *Narrow down* pick *equal to* and type the new value.
- Repeat until one or a few addresses are left. **Enter** sets one to a
  new value; **Ins** freezes it so it never changes, **Del** drops it or
  unfreezes it.

For a bar without a number (energy), search with no value, then narrow
down by *decreased* or *increased* as the bar changes.

# RetroAchievements {#achievements}

Achievements, leaderboards and rich presence from
retroachievements.org, for games launched from their profiles.

- Turn *RetroAchievements* on, type your user name and password, and
  pick *Log in*. The site's token is kept in the configuration file,
  not your password.
- Launch the game on the Games page. A zip dropped on the window gets a
  profile that says which version it is; for other games, pick the
  zip or `.dosz` the game came in at *Game's archive*.
- The list shows the game's achievements: **√** unlocked, and the
  progress of those that count something.

*Hardcore mode*, from the next game on: no save states, rewind or
cheats, and the unlocks count as hardcore.

# The game's archive {#achievements-archive}

The zip or `.dosz` the game came in, as RetroAchievements knows it
(the files in it, not how they are packed). Its hash goes into the
game's profile, as `achievements=` in `[game]`.

# Stats {#stats}

How well the game runs: **FPS**, the frames a second it draws, and
**CPU**, how busy the emulator keeps one core of your computer.

When **CPU** is near 100% (red), your computer can't keep up: set a
lower *CPU speed (cycles)* on the Emulator page, or use a simpler CRT
shader.

**Ctrl+Shift+F12** shows these two numbers over the game while it runs.
