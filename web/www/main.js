// Rust-DOS in the browser. The emulator (pkg/, which ../build.sh builds)
// runs one animation frame at a time, as the rust-dos program runs its
// window, with the page's keyboard, mouse, sound and files. C: is a hard
// disk image the page keeps in the browser's storage.
//
// Page parameters: ?zip=URL copies an archive to C: at startup and ?run=
// types a command at the first prompt (both can be given more than once);
// ?persist=0 keeps C: in memory only; ?log sends the emulator's log to the
// console.

import init, { Machine } from './pkg/rust_dos_web.js';
import { DiskStore } from './storage.js';
import { unzip } from './zip.js';

const DRIVE_A = 0;
const DRIVE_C = 2;
const DRIVE_D = 3;
const DRIVE_Y = 24;
const SAMPLE_RATE = 44100;
/// How often changes to C: are saved.
const SAVE_EVERY_MS = 2000;
/// Disk images are read into the emulator this much at a time.
const READ_SLICE = 16 << 20;
/// The size of a new C:, unless Erase C: picked another.
const DEFAULT_C_MEGABYTES = 250;
/// Files dropped on their own that go in a drive rather than on C:.
const DISK_IMAGE = /\.(img|ima|vfd|flp|dsk|iso)$/i;

const CONFIG_KEY = 'rust-dos.conf';
const C_SIZE_KEY = 'rust-dos.c-megabytes';
const MUTED_KEY = 'rust-dos.muted';

const DEFAULT_CONFIG = `# Rust-DOS settings, in the format of rust-dos.conf. Remove the # in
# front of a setting to use it. They take effect when the machine restarts.

[emulator]
# Stretch the picture to 4:3, as a monitor showed 320x200 and 640x400.
aspect=true
# How the picture is scaled up: nearest (sharp pixels) or linear (smooth).
#filter=nearest
# CPU speed in instructions per millisecond, or max to run as fast as the
# browser keeps up with. Lower it for old games that run too fast.
#cycles=max
# Processor: 486 (a 486DX with FPU) or 386.
#cpu=486
# RAM in MB, 2 to 64.
#memsize=16
# How fast disks are: maximum, fast, medium or slow.
#hard_disk_speed=maximum
#floppy_disk_speed=maximum

[sound]
# The Sound Blaster: sb16, sbpro2, sb2 or none.
#sbtype=sb16
#sbbase=220
#irq=7
#dma=1
#hdma=5
# FM synthesizer: opl3 or opl2.
#opl=opl3
# The Gravis Ultrasound, with its MIDI patches on drive X:.
#gus=true
# The noises the disks make: off, seek-only or on.
#hard_disk_noise=off
#floppy_disk_noise=off

[autoexec]
# Commands typed at the DOS prompt on startup, before C:\\AUTOEXEC.BAT.
`;

const params = new URLSearchParams(location.search);
const persist = params.get('persist') !== '0';

const $ = (id) => document.getElementById(id);
const canvas = $('screen');
const screen2d = canvas.getContext('2d', { alpha: false });

let wasm;
let machine;
/// Where C: is kept, or null if it isn't.
let store = null;
/// The machine stopped: EXIT, a crash, or a restart on its way.
let halted = false;

// ---------------------------------------------------------------------
// Small helpers
// ---------------------------------------------------------------------

function remembered(key, fallback) {
  try {
    return localStorage.getItem(key) ?? fallback;
  } catch {
    return fallback;
  }
}

function remember(key, value) {
  try {
    localStorage.setItem(key, value);
  } catch {
    // Settings then last until the page closes.
  }
}

function toast(message, kind = '') {
  const note = document.createElement('div');
  note.className = `toast ${kind}`;
  note.textContent = message;
  $('toasts').append(note);
  setTimeout(() => note.remove(), kind === 'warn' ? 9000 : 4500);
}

const letter = (drive) => `${String.fromCharCode(65 + drive)}:`;

/// Tasks that load files, one after the other.
let work = Promise.resolve();
function enqueue(task) {
  work = work.then(task).catch((error) => toast(error.message ?? String(error), 'warn'));
  return work;
}

/// A date as DOS dates files.
function dosTime(date) {
  const year = Math.min(Math.max(date.getFullYear(), 1980), 2107);
  return {
    time: (date.getHours() << 11) | (date.getMinutes() << 5) | (date.getSeconds() >> 1),
    date: ((year - 1980) << 9) | ((date.getMonth() + 1) << 5) | date.getDate(),
  };
}

function formatSize(bytes) {
  if (!bytes) {
    return '';
  }
  if (bytes < 1 << 20) {
    return `${Math.round(bytes / 1024)} KB`;
  }
  // Floppies go by thousands of KB: 1.44 MB is 1440 KB.
  if (bytes < 4 << 20) {
    return `${Number((bytes / 1024000).toFixed(2))} MB`;
  }
  return `${Math.round(bytes / 1048576)} MB`;
}

// ---------------------------------------------------------------------
// Sound
// ---------------------------------------------------------------------

/// Web Audio, fed a buffer per frame, each starting where the last ends.
const speaker = {
  context: null,
  gain: null,
  next: 0,
  muted: remembered(MUTED_KEY, '0') === '1',

  /// Browsers start sound only after the user does something on the page.
  start() {
    if (this.muted) {
      return;
    }
    if (!this.context) {
      try {
        this.context = new AudioContext({ sampleRate: SAMPLE_RATE, latencyHint: 'interactive' });
      } catch {
        this.context = new AudioContext({ latencyHint: 'interactive' });
      }
      this.gain = this.context.createGain();
      this.gain.connect(this.context.destination);
    }
    if (this.context.state === 'suspended') {
      this.context.resume();
    }
  },

  playing() {
    return !this.muted && this.context?.state === 'running';
  },

  /// Frames waiting to be played. While nothing plays, a queue so full the
  /// emulator keeps its sound to itself.
  queued() {
    if (!this.playing()) {
      return SAMPLE_RATE;
    }
    return Math.max(0, Math.round((this.next - this.context.currentTime) * SAMPLE_RATE));
  },

  /// Play `planar`: the left channel's samples, then the right's.
  play(planar) {
    const frames = planar.length / 2;
    if (!frames || !this.playing()) {
      return;
    }
    const buffer = this.context.createBuffer(2, frames, SAMPLE_RATE);
    buffer.copyToChannel(planar.subarray(0, frames), 0);
    buffer.copyToChannel(planar.subarray(frames), 1);
    const source = this.context.createBufferSource();
    source.buffer = buffer;
    source.connect(this.gain);
    const at = Math.max(this.next, this.context.currentTime);
    source.start(at);
    this.next = at + frames / SAMPLE_RATE;
  },

  setMuted(muted) {
    this.muted = muted;
    remember(MUTED_KEY, muted ? '1' : '0');
    if (muted) {
      this.context?.suspend();
    } else {
      this.start();
    }
  },
};

function showSound() {
  const button = $('sound');
  button.textContent = speaker.muted ? 'Sound off' : 'Sound on';
  button.setAttribute('aria-pressed', String(!speaker.muted));
}

// ---------------------------------------------------------------------
// The screen
// ---------------------------------------------------------------------

/// Size the canvas to fill the stage at the picture's proportions: 4:3
/// with `aspect`, else its pixels' own.
function layout() {
  const stage = $('stage');
  const style = getComputedStyle(stage);
  const room = {
    width: stage.clientWidth - parseFloat(style.paddingLeft) - parseFloat(style.paddingRight),
    height: stage.clientHeight - parseFloat(style.paddingTop) - parseFloat(style.paddingBottom),
  };
  const ratio = machine?.aspect() ? 4 / 3 : canvas.width / canvas.height;
  let width = room.width;
  let height = width / ratio;
  if (height > room.height) {
    height = room.height;
    width = height * ratio;
  }
  canvas.style.width = `${Math.max(0, Math.floor(width))}px`;
  canvas.style.height = `${Math.max(0, Math.floor(height))}px`;
}

function draw() {
  const width = machine.screen_width();
  const height = machine.screen_height();
  if (canvas.width !== width || canvas.height !== height) {
    canvas.width = width;
    canvas.height = height;
    layout();
  }
  // The pixels stay in the module's memory; the view is made anew each
  // time, as the memory can grow.
  const pixels = new Uint8ClampedArray(wasm.memory.buffer, machine.screen_pixels(), width * height * 4);
  screen2d.putImageData(new ImageData(pixels, width, height), 0, 0);
}

// ---------------------------------------------------------------------
// The machine's frames
// ---------------------------------------------------------------------

let frames = 0;
let lastStatus = 0;

function frame(now) {
  if (halted) {
    return;
  }
  requestAnimationFrame(frame);
  // The machine waits while a dialog is open, as it does under the rust-dos
  // program's settings window.
  if (document.querySelector('dialog[open]')) {
    return;
  }
  try {
    const changed = machine.run_frame(speaker.queued());
    speaker.play(machine.take_sound());
    if (changed) {
      draw();
    }
    showActivity(machine.take_drive_activity(), now);
    if (machine.take_settings_request()) {
      openSettings();
    }
    if (machine.exit_requested()) {
      saveC();
      halt('DOS was shut down with EXIT.');
      return;
    }
  } catch (error) {
    crash(error);
    return;
  }
  frames++;
  if (now - lastStatus >= 1000) {
    const fps = Math.round((frames * 1000) / (now - lastStatus));
    $('speed').textContent = `${machine.cycles().toLocaleString('en-US')} cycles/ms · ${fps} fps`;
    frames = 0;
    lastStatus = now;
  }
}

function halt(message) {
  halted = true;
  document.exitPointerLock?.();
  $('halt-message').textContent = message;
  $('halt').hidden = false;
}

function crash(error) {
  console.error(error);
  halt(`The emulator stopped: ${error.message ?? error}. Changes to C: from the last few seconds may be lost.`);
}

async function restart() {
  await saveC();
  halted = true;
  location.reload();
}

// ---------------------------------------------------------------------
// Keyboard and mouse
// ---------------------------------------------------------------------

function forPage(target) {
  return target instanceof Element && target.closest('input, textarea, select, dialog[open]');
}

function guard(action) {
  if (!machine || halted) {
    return false;
  }
  try {
    return action();
  } catch (error) {
    crash(error);
    return false;
  }
}

window.addEventListener('keydown', (event) => {
  if (!machine || halted || forPage(event.target) || document.querySelector('dialog[open]')) {
    return;
  }
  speaker.start();
  // Ctrl+F12 opens the settings and Ctrl+F10 lets go of the mouse, as in
  // the rust-dos program and DOSBox.
  if (event.ctrlKey && !event.altKey && event.code === 'F12') {
    event.preventDefault();
    if (!event.repeat) {
      openSettings();
    }
    return;
  }
  if (event.ctrlKey && event.code === 'F10') {
    event.preventDefault();
    document.exitPointerLock?.();
    return;
  }
  // The system's and the browser's shortcuts.
  if (event.metaKey) {
    return;
  }
  const taken = guard(() => machine.key_down(event.code, event.key, event.getModifierState('AltGraph')));
  // F11 still makes the browser full screen.
  if (taken && event.code !== 'F11') {
    event.preventDefault();
  }
});

window.addEventListener('keyup', (event) => {
  if (guard(() => machine.key_up(event.code))) {
    event.preventDefault();
  }
});

function releaseInput() {
  guard(() => machine.release_input());
  buttonsDown.clear();
}

window.addEventListener('blur', releaseInput);

/// Where the mouse is on the screen, in its pixels. While the mouse is
/// captured, the page moves it by the mouse's motion.
const pointer = { x: 0, y: 0 };
const buttonsDown = new Set();
const captured = () => document.pointerLockElement === canvas;

function screenPoint(event) {
  const rect = canvas.getBoundingClientRect();
  return {
    x: ((event.clientX - rect.left) * canvas.width) / rect.width,
    y: ((event.clientY - rect.top) * canvas.height) / rect.height,
  };
}

function movePointer(event) {
  if (captured()) {
    const rect = canvas.getBoundingClientRect();
    pointer.x = Math.min(Math.max(pointer.x + (event.movementX * canvas.width) / rect.width, 0), canvas.width - 1);
    pointer.y = Math.min(Math.max(pointer.y + (event.movementY * canvas.height) / rect.height, 0), canvas.height - 1);
  } else {
    Object.assign(pointer, screenPoint(event));
  }
  guard(() => machine.mouse_move(pointer.x, pointer.y));
}

canvas.addEventListener('pointermove', movePointer);

canvas.addEventListener('pointerdown', (event) => {
  speaker.start();
  if (!machine || halted) {
    return;
  }
  event.preventDefault();
  // A program using the mouse gets it captured by a click, which it
  // doesn't see, as in DOSBox.
  if (event.pointerType === 'mouse' && !captured() && guard(() => machine.mouse_installed())) {
    Object.assign(pointer, screenPoint(event));
    try {
      canvas.requestPointerLock()?.catch?.(() => {});
    } catch {
      // Without capture the mouse still works, up to the screen's edges.
    }
    return;
  }
  movePointer(event);
  buttonsDown.add(event.button);
  guard(() => machine.mouse_button(event.button, true));
});

window.addEventListener('pointerup', (event) => {
  if (buttonsDown.delete(event.button)) {
    guard(() => machine.mouse_button(event.button, false));
  }
});

canvas.addEventListener('contextmenu', (event) => event.preventDefault());

document.addEventListener('pointerlockchange', () => {
  canvas.classList.toggle('captured', captured());
  $('mouse-hint').hidden = !captured();
});

// ---------------------------------------------------------------------
// C: in the browser's storage
// ---------------------------------------------------------------------

/// Pieces of C: written and not saved yet.
const unsaved = new Set();
let saving = null;
let saveFailed = false;
/// Erase C: is on its way: nothing more is saved.
let erasing = false;

/// Save the pieces of C: written since the last save.
async function saveC() {
  if (!store || erasing || !machine) {
    return;
  }
  try {
    for (const index of machine.written_chunks(DRIVE_C)) {
      unsaved.add(index);
    }
  } catch {
    return;
  }
  if (saving) {
    await saving;
    return saveC();
  }
  if (!unsaved.size) {
    return;
  }
  const indices = [...unsaved];
  unsaved.clear();
  const pieces = indices.map((index) => [index, machine.image_chunk(DRIVE_C, index) ?? null]);
  saving = store
    .save(pieces)
    .then(
      () => {
        if (saveFailed) {
          toast('C: is being saved again.');
        }
        saveFailed = false;
      },
      (error) => {
        for (const index of indices) {
          unsaved.add(index);
        }
        if (!saveFailed) {
          toast(`Changes to C: can't be saved in the browser's storage: ${error.message ?? error}`, 'warn');
        }
        saveFailed = true;
      },
    )
    .finally(() => {
      saving = null;
    });
  await saving;
}

let askedToKeep = false;

/// Ask the browser not to clear its storage of C: when it runs short of
/// room, once files were put there.
function askToKeepC() {
  if (store && !askedToKeep && navigator.storage?.persist) {
    askedToKeep = true;
    navigator.storage.persist().catch(() => {});
  }
}

/// Mount C: as the browser keeps it, or a new one.
async function setUpC() {
  if (persist) {
    try {
      store = await DiskStore.open();
    } catch (error) {
      toast(`The browser's storage can't be used, so C: is lost when the page closes: ${error.message ?? error}`, 'warn');
    }
  }
  const chunk = Machine.chunk_size();
  if (store) {
    try {
      if (await store.restore(machine, chunk)) {
        machine.mount_image(DRIVE_C, 'C.IMG', 'hdd');
        return;
      }
    } catch (error) {
      toast(`C: couldn't be read back from the browser's storage, so it starts out empty: ${error.message ?? error}`, 'warn');
    }
  }
  const megabytes = Number(remembered(C_SIZE_KEY, DEFAULT_C_MEGABYTES)) || DEFAULT_C_MEGABYTES;
  machine.format_c(megabytes);
  if (store) {
    try {
      await store.reset(machine.image_size(DRIVE_C), chunk);
      await saveC();
    } catch (error) {
      toast(`C: can't be kept in the browser's storage: ${error.message ?? error}`, 'warn');
      store = null;
    }
  }
}

// ---------------------------------------------------------------------
// Files and disk images
// ---------------------------------------------------------------------

/// Copy files to C: ({path, data, time, date}, paths with forward
/// slashes), with names made DOS names.
function copyToC(copies) {
  const paths = Machine.dos_paths(copies.map((copy) => copy.path));
  let copied = 0;
  const failures = [];
  copies.forEach((copy, i) => {
    try {
      machine.put_file(paths[i], copy.data, copy.time, copy.date);
      copied++;
    } catch (error) {
      failures.push(error.message ?? String(error));
    }
  });
  const tops = new Set(paths.map((path) => path.split('\\')[0]));
  const where = tops.size === 1 && paths[0].includes('\\') ? `C:\\${paths[0].split('\\')[0]}` : 'C:\\';
  if (copied) {
    toast(`Copied ${copied} ${copied === 1 ? 'file' : 'files'} to ${where}`);
  }
  if (failures.length) {
    toast(failures.length === 1 ? failures[0] : `${failures.length} files weren't copied. ${failures[0]}`, 'warn');
  }
  saveC();
  askToKeepC();
}

/// The files of a .zip archive, for C:. They go in a directory named after
/// the archive, unless they are all in one directory in it already.
async function zipFiles(name, bytes) {
  const files = await unzip(bytes);
  const tops = new Set(files.map((file) => file.path.split('/')[0]));
  const inOne = tops.size === 1 && files.every((file) => file.path.includes('/'));
  const prefix = inOne ? '' : `${name.replace(/\.zip$/i, '')}/`;
  return files.map((file) => ({ ...file, path: prefix + file.path }));
}

/// Add what was dropped or picked ({path, file}): archives and files go on
/// C:, disk images on their own go in a drive.
async function addItems(items) {
  const copies = [];
  for (const { path, file } of items) {
    const alone = !path.includes('/');
    try {
      if (alone && /\.zip$/i.test(path)) {
        copies.push(...(await zipFiles(file.name, new Uint8Array(await file.arrayBuffer()))));
      } else if (alone && DISK_IMAGE.test(path)) {
        await insertImage(file);
      } else {
        const data = new Uint8Array(await file.arrayBuffer());
        copies.push({ path, data, ...dosTime(new Date(file.lastModified)) });
      }
    } catch (error) {
      toast(`${path}: ${error.message ?? error}`, 'warn');
    }
  }
  if (copies.length) {
    copyToC(copies);
  }
}

async function fetchZip(url) {
  try {
    const response = await fetch(url);
    if (!response.ok) {
      throw new Error(`${response.status} ${response.statusText}`);
    }
    const name = decodeURIComponent(new URL(url, location.href).pathname.split('/').pop() || 'download.zip');
    copyToC(await zipFiles(name, new Uint8Array(await response.arrayBuffer())));
  } catch (error) {
    toast(`${url} couldn't be loaded: ${error.message ?? error}`, 'warn');
  }
}

/// The mounted drives, as the machine lists them.
function drives() {
  return JSON.parse(machine.drives());
}

/// The first drive letter from D: on that isn't taken.
function freeDrive() {
  const taken = new Set(drives().map((drive) => drive.drive));
  for (let drive = DRIVE_D; drive <= DRIVE_Y; drive++) {
    if (!taken.has(drive)) {
      return drive;
    }
  }
  return undefined;
}

/// Put the disk or CD image `file` in `drive`: by default a CD in the CD-ROM
/// drive, a floppy in A: and a hard disk in the next free drive.
async function insertImage(file, drive) {
  const cd = /\.iso$/i.test(file.name);
  const floppy = !cd && Machine.is_floppy_size(file.size);
  if (drive === undefined) {
    const cdDrive = drives().find((d) => d.kind === 'cdrom');
    drive = cd && cdDrive ? cdDrive.drive : floppy ? DRIVE_A : freeDrive();
  }
  if (drive === undefined) {
    throw new Error('no drive letter is free for it');
  }
  machine.begin_image(file.size);
  for (let at = 0; at < file.size; at += READ_SLICE) {
    machine.write_image(at, new Uint8Array(await file.slice(at, at + READ_SLICE).arrayBuffer()));
  }
  machine.mount_image(drive, file.name, cd ? 'cdrom' : '');
  toast(`${file.name} is in drive ${letter(drive)}`);
  showDrives();
}

/// The files of dropped entries (FileSystemEntry, or File where the
/// browser gives only that), folders with everything in them.
async function droppedFiles(entries) {
  const files = [];
  async function walk(entry, prefix) {
    if (entry instanceof File) {
      files.push({ path: entry.name, file: entry });
    } else if (entry.isFile) {
      const file = await new Promise((resolve, reject) => entry.file(resolve, reject));
      files.push({ path: prefix + entry.name, file });
    } else if (entry.isDirectory) {
      const reader = entry.createReader();
      for (;;) {
        const batch = await new Promise((resolve, reject) => reader.readEntries(resolve, reject));
        if (!batch.length) {
          break;
        }
        for (const child of batch) {
          await walk(child, `${prefix}${entry.name}/`);
        }
      }
    }
  }
  for (const entry of entries) {
    await walk(entry, '');
  }
  return files;
}

let dragDepth = 0;
const draggingFiles = (event) => event.dataTransfer?.types?.includes('Files');

window.addEventListener('dragenter', (event) => {
  if (draggingFiles(event)) {
    event.preventDefault();
    dragDepth++;
    $('drop').hidden = false;
  }
});

window.addEventListener('dragover', (event) => {
  if (draggingFiles(event)) {
    event.preventDefault();
  }
});

window.addEventListener('dragleave', () => {
  dragDepth = Math.max(0, dragDepth - 1);
  if (!dragDepth) {
    $('drop').hidden = true;
  }
});

window.addEventListener('drop', (event) => {
  event.preventDefault();
  dragDepth = 0;
  $('drop').hidden = true;
  if (!machine || halted) {
    return;
  }
  // The entries have to be taken while the drop is being handled.
  const entries = [...event.dataTransfer.items]
    .filter((item) => item.kind === 'file')
    .map((item) => item.webkitGetAsEntry?.() ?? item.getAsFile())
    .filter(Boolean);
  enqueue(async () => addItems(await droppedFiles(entries)));
});

$('add-files').addEventListener('click', () => $('file-input').click());
$('add-folder').addEventListener('click', () => $('folder-input').click());

$('file-input').addEventListener('change', (event) => {
  const files = [...event.target.files];
  event.target.value = '';
  enqueue(() => addItems(files.map((file) => ({ path: file.name, file }))));
});

$('folder-input').addEventListener('change', (event) => {
  const files = [...event.target.files];
  event.target.value = '';
  enqueue(() => addItems(files.map((file) => ({ path: file.webkitRelativePath || file.name, file }))));
});

// ---------------------------------------------------------------------
// Drives: the status strip and the dialog
// ---------------------------------------------------------------------

const KIND_NAMES = { floppy: 'Floppy', hdd: 'Hard disk', cdrom: 'CD-ROM' };
/// When each drive's light goes out.
const lampUntil = new Map();

function showActivity(mask, now) {
  for (let drive = 0; mask >> drive; drive++) {
    if ((mask >> drive) & 1) {
      lampUntil.set(drive, now + 120);
    }
  }
  for (const lamp of document.querySelectorAll('.chip .lamp')) {
    lamp.classList.toggle('on', (lampUntil.get(Number(lamp.dataset.drive)) ?? 0) > now);
  }
}

/// The drives in the status strip, and in the dialog if it is open.
function showDrives() {
  const chips = $('drive-chips');
  chips.replaceChildren();
  for (const drive of drives()) {
    if (drive.kind === 'virtual') {
      continue;
    }
    const chip = document.createElement('button');
    chip.type = 'button';
    chip.className = 'chip';
    chip.title = [KIND_NAMES[drive.kind], drive.label, drive.image].filter(Boolean).join(' · ');
    const lamp = document.createElement('span');
    lamp.className = 'lamp';
    lamp.dataset.drive = drive.drive;
    const name = document.createElement('b');
    name.textContent = drive.letter + ':';
    chip.append(lamp, name, ' ', formatSize(drive.size) || drive.label);
    chip.addEventListener('click', openDrives);
    chips.append(chip);
  }
  if ($('drives').open) {
    fillDrivesDialog();
  }
}

function button(text, action, className = '') {
  const b = document.createElement('button');
  b.type = 'button';
  b.textContent = text;
  b.className = className;
  b.addEventListener('click', action);
  return b;
}

/// Offer `drive`'s image as a file to save.
function download(drive, name) {
  const size = machine.image_size(drive);
  const chunk = Machine.chunk_size();
  const zeros = new Uint8Array(chunk);
  const parts = [];
  for (let index = 0; index * chunk < size; index++) {
    parts.push(machine.image_chunk(drive, index) ?? zeros.subarray(0, Math.min(chunk, size - index * chunk)));
  }
  const url = URL.createObjectURL(new Blob(parts, { type: 'application/octet-stream' }));
  const link = document.createElement('a');
  link.href = url;
  link.download = name;
  link.click();
  setTimeout(() => URL.revokeObjectURL(url), 60_000);
}

function fillDrivesDialog() {
  const rows = $('drive-rows');
  rows.replaceChildren();
  const mounted = drives();
  for (const drive of mounted) {
    if (drive.kind === 'virtual') {
      continue;
    }
    const row = rows.insertRow();
    row.insertCell().textContent = drive.letter + ':';
    row.insertCell().textContent = KIND_NAMES[drive.kind] ?? drive.kind;
    const label = row.insertCell();
    label.textContent = drive.label;
    if (drive.image && drive.drive !== DRIVE_C) {
      const image = document.createElement('div');
      image.className = 'image-name';
      image.textContent = drive.image;
      label.append(image);
    }
    row.insertCell().textContent = formatSize(drive.size);
    const actions = row.insertCell();
    actions.className = 'actions-cell';
    if (drive.size && machine.image_size(drive.drive)) {
      const name = drive.drive === DRIVE_C ? 'rust-dos-C.img' : drive.image || `${drive.letter}.img`;
      actions.append(button('Download', () => download(drive.drive, name)));
    }
    if (drive.drive !== DRIVE_C) {
      actions.append(
        button('Eject', () => {
          try {
            machine.unmount(drive.drive);
          } catch (error) {
            toast(error.message ?? String(error), 'warn');
          }
          showDrives();
        }),
      );
    }
  }
  $('c-note').textContent = store
    ? "C: is kept in this browser's storage, and is there again the next time you open this page."
    : 'C: is lost when the page closes: this page is not keeping it in the browser.';

  // Every drive letter but C: and the drives held in memory.
  const select = $('image-drive');
  const chosen = select.value;
  select.replaceChildren();
  const inMemory = new Set(mounted.filter((d) => d.kind === 'virtual').map((d) => d.drive));
  for (let drive = DRIVE_A; drive <= DRIVE_Y; drive++) {
    if (drive === DRIVE_C || inMemory.has(drive)) {
      continue;
    }
    const option = new Option(letter(drive), drive);
    select.append(option);
  }
  select.value = chosen || String(DRIVE_A);
  $('c-size').value = remembered(C_SIZE_KEY, String(DEFAULT_C_MEGABYTES));
}

function openDialog(dialog) {
  releaseInput();
  document.exitPointerLock?.();
  dialog.showModal();
}

// A closed dialog gives the keyboard back to DOS.
for (const dialog of document.querySelectorAll('dialog')) {
  dialog.addEventListener('close', () => document.activeElement?.blur());
}

function openDrives() {
  if (!machine || halted) {
    return;
  }
  fillDrivesDialog();
  openDialog($('drives'));
}

$('open-drives').addEventListener('click', openDrives);
$('choose-image').addEventListener('click', () => $('image-input').click());
$('image-input').addEventListener('change', (event) => {
  const [file] = event.target.files;
  event.target.value = '';
  if (file) {
    const drive = Number($('image-drive').value);
    enqueue(() => insertImage(file, drive));
  }
});

$('erase-c').addEventListener('click', async () => {
  remember(C_SIZE_KEY, $('c-size').value);
  erasing = true;
  halted = true;
  try {
    await store?.clear();
  } finally {
    location.reload();
  }
});

// ---------------------------------------------------------------------
// Settings
// ---------------------------------------------------------------------

function openSettings() {
  if (!machine || halted || $('settings').open) {
    return;
  }
  $('config').value = remembered(CONFIG_KEY, DEFAULT_CONFIG);
  const warnings = $('config-warnings');
  warnings.replaceChildren(
    ...machine.warnings().map((warning) => {
      const item = document.createElement('li');
      item.textContent = warning;
      return item;
    }),
  );
  openDialog($('settings'));
}

$('open-settings').addEventListener('click', openSettings);
$('config-defaults').addEventListener('click', () => {
  $('config').value = DEFAULT_CONFIG;
});
$('config-save').addEventListener('click', () => {
  remember(CONFIG_KEY, $('config').value);
  restart();
});

// ---------------------------------------------------------------------
// The rest of the bar
// ---------------------------------------------------------------------

$('sound').addEventListener('click', () => {
  speaker.setMuted(!speaker.muted);
  showSound();
});

$('fullscreen').addEventListener('click', async () => {
  try {
    await $('stage').requestFullscreen();
    // Keys like Esc and Ctrl+W then go to DOS, where the browser allows it.
    await navigator.keyboard?.lock?.();
  } catch (error) {
    toast(`Full screen isn't available: ${error.message ?? error}`, 'warn');
  }
});

// Buttons give the keyboard back to DOS once clicked.
document.querySelector('.bar').addEventListener('click', (event) => event.target.closest('button')?.blur());
$('drive-chips').addEventListener('click', (event) => event.target.closest('button')?.blur());

$('restart').addEventListener('click', () => location.reload());

new ResizeObserver(layout).observe($('stage'));
document.addEventListener('fullscreenchange', layout);

document.addEventListener('visibilitychange', () => {
  if (document.hidden) {
    releaseInput();
    saveC();
  }
});
window.addEventListener('pagehide', () => saveC());

// ---------------------------------------------------------------------
// Start
// ---------------------------------------------------------------------

async function start() {
  showSound();
  try {
    wasm = await init();
  } catch (error) {
    $('loading').querySelector('p').textContent = `Rust-DOS didn't load: ${error.message ?? error}`;
    return;
  }
  machine = new Machine(remembered(CONFIG_KEY, DEFAULT_CONFIG));
  if (params.has('log')) {
    machine.log_to_console();
  }
  for (const warning of machine.warnings()) {
    toast(`Settings: ${warning}`, 'warn');
  }
  await setUpC();
  for (const url of params.getAll('zip')) {
    await fetchZip(url);
  }
  canvas.classList.toggle('smooth', machine.smooth());
  const notes = [
    store
      ? "C: is kept in this browser's storage."
      : 'C: is in memory only, and is lost when the page closes.',
    'Drop files, folders, .zip archives or disk images here to add them.',
  ];
  machine.boot(notes, params.getAll('run'));
  showDrives();
  layout();
  $('loading').hidden = true;
  requestAnimationFrame(frame);
  setInterval(saveC, SAVE_EVERY_MS);
}

start();
