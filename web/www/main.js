// Rust-DOS in the browser. The emulator (pkg/, which ../build.sh builds)
// runs one animation frame at a time, as the rust-dos program runs its
// window, with the page's keyboard, mouse, sound and files. C: is a hard
// disk image the page keeps in the browser's storage.
//
// Page parameters: ?zip=URL copies an archive to C: at startup and ?run=
// types a command at the first prompt (both can be given more than once);
// ?persist=0 keeps C: in memory only; ?log sends the emulator's log to the
// console; ?renderer=2d draws without WebGL 2, and so without the CRT
// shaders.

import init, { Machine, shader_program } from './pkg/rust_dos_web.js';
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
/// Wheel travel that makes a notch, in pixels, and what a line and a page
/// of it are.
const WHEEL_NOTCH = 100;
const WHEEL_UNITS = [1, 33, 400];
const C_SIZE_KEY = 'rust-dos.c-megabytes';
const MUTED_KEY = 'rust-dos.muted';

const DEFAULT_CONFIG = `# Rust-DOS settings, in the format of rust-dos.conf. Remove the # in
# front of a setting to use it. They take effect when the machine restarts.

[emulator]
# Stretch the picture to 4:3, as a monitor showed 320x200 and 640x400.
aspect=true
# How the picture is scaled up: nearest (sharp pixels) or linear (smooth),
# without a CRT shader.
#filter=nearest
# A CRT look: none, scanlines, aperture or curved. It needs WebGL 2.
#shader=none
# A monochrome monitor: off (colour), white, amber or green.
#monochrome=off
# CPU speed in instructions per millisecond, or max to run as fast as the
# browser keeps up with. Lower it for old games that run too fast.
#cycles=max
# Processor: 486 (a 486DX with FPU) or 386.
#cpu=486
# The display adapter: svga (VGA with VESA modes), vga, ega, cga or
# hercules.
#machine=svga
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

[mixer]
# The volume of each sound source in percent, 0 to 200 (100 is as loud as
# the card makes it), and of everything together.
#master=100
#speaker=100
#sb=100
#fm=100
#gus=100
#midi=100
#cdaudio=100
#disknoise=100

[autoexec]
# Commands typed at the DOS prompt on startup, before C:\\AUTOEXEC.BAT.
`;

const params = new URLSearchParams(location.search);
const persist = params.get('persist') !== '0';

const $ = (id) => document.getElementById(id);
const canvas = $('screen');
/// The picture is drawn with WebGL 2, through the CRT shaders, or else put
/// on a 2D canvas as it is. A canvas keeps the kind it is first asked for.
const screenGl = params.get('renderer') === '2d' ? null : makeGlScreen();
const screen2d = screenGl ? null : canvas.getContext('2d', { alpha: false });

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

/// Returns whether the browser kept it.
function remember(key, value) {
  try {
    localStorage.setItem(key, value);
    return true;
  } catch {
    // Settings then last until the page closes.
    return false;
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
/// The sound goes through `gain`, where a recording takes it, then
/// `output`, which the mute turns down while a recording runs; otherwise
/// the mute suspends it all.
const speaker = {
  context: null,
  gain: null,
  output: null,
  next: 0,
  muted: remembered(MUTED_KEY, '0') === '1',
  /// Where a recording takes the sound (`record`), while one runs.
  tap: null,

  /// Browsers start sound only after the user does something on the page.
  start() {
    if (this.muted && !this.tap) {
      return;
    }
    if (!this.context) {
      try {
        this.context = new AudioContext({ sampleRate: SAMPLE_RATE, latencyHint: 'interactive' });
      } catch {
        this.context = new AudioContext({ latencyHint: 'interactive' });
      }
      this.gain = this.context.createGain();
      this.output = this.context.createGain();
      this.gain.connect(this.output);
      this.output.connect(this.context.destination);
    }
    this.output.gain.value = this.muted ? 0 : 1;
    if (this.context.state === 'suspended') {
      this.context.resume();
    }
  },

  playing() {
    return (!this.muted || this.tap !== null) && this.context?.state === 'running';
  },

  /// The sound as a stream, for a recording, heard or not.
  record() {
    this.tap = true;
    this.start();
    this.tap = this.context.createMediaStreamDestination();
    this.gain.connect(this.tap);
    return this.tap.stream;
  },

  stopRecording() {
    if (this.tap) {
      this.gain.disconnect(this.tap);
      this.tap = null;
    }
    if (this.muted) {
      this.context?.suspend();
    }
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
    if (muted && !this.tap) {
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

/// Turn the sound off or on, with the Sound button or Ctrl+F8 (`announce`:
/// say so on the screen), and tell the machine's mixer.
function setSound(muted, announce) {
  speaker.setMuted(muted);
  showSound();
  if (machine) {
    guard(() => machine.set_muted(muted, announce));
  }
}

// ---------------------------------------------------------------------
// The screen
// ---------------------------------------------------------------------

/// Size the canvas to fill the stage at the picture's proportions: 4:3
/// with `aspect`, else its pixels' own. With WebGL it has the screen's
/// pixels, which the CRT shaders need, and is drawn again.
function layout() {
  const stage = $('stage');
  const style = getComputedStyle(stage);
  const room = {
    width: stage.clientWidth - parseFloat(style.paddingLeft) - parseFloat(style.paddingRight),
    height: stage.clientHeight - parseFloat(style.paddingTop) - parseFloat(style.paddingBottom),
  };
  const ratio = machine?.aspect() ? 4 / 3 : (machine?.screen_width() ?? 640) / (machine?.screen_height() ?? 400);
  let width = room.width;
  let height = width / ratio;
  if (height > room.height) {
    height = room.height;
    width = height * ratio;
  }
  width = Math.max(0, Math.floor(width));
  height = Math.max(0, Math.floor(height));
  canvas.style.width = `${width}px`;
  canvas.style.height = `${height}px`;
  if (screenGl) {
    const scale = window.devicePixelRatio || 1;
    const pixels = { width: Math.max(1, Math.round(width * scale)), height: Math.max(1, Math.round(height * scale)) };
    if (canvas.width !== pixels.width || canvas.height !== pixels.height) {
      canvas.width = pixels.width;
      canvas.height = pixels.height;
    }
    screenGl.render();
  }
}

/// The screen's pixels change with the browser's zoom and between monitors.
function watchPixelRatio() {
  matchMedia(`(resolution: ${window.devicePixelRatio}dppx)`).addEventListener(
    'change',
    () => {
      layout();
      watchPixelRatio();
    },
    { once: true },
  );
}

function draw() {
  const width = machine.screen_width();
  const height = machine.screen_height();
  // The pixels stay in the module's memory; the view is made anew each
  // time, as the memory can grow.
  if (screenGl) {
    const pixels = new Uint8Array(wasm.memory.buffer, machine.screen_pixels(), width * height * 4);
    if (screenGl.upload(pixels, width, height)) {
      layout();
    } else {
      screenGl.render();
    }
    return;
  }
  if (canvas.width !== width || canvas.height !== height) {
    canvas.width = width;
    canvas.height = height;
    layout();
  }
  const pixels = new Uint8ClampedArray(wasm.memory.buffer, machine.screen_pixels(), width * height * 4);
  screen2d.putImageData(new ImageData(pixels, width, height), 0, 0);
}

/// The screen drawn with WebGL 2: the picture as a texture, shown through
/// the look the settings choose, whose shaders come from the emulator
/// (`shader_program`). Null where the browser has no WebGL 2.
function makeGlScreen() {
  const gl = canvas.getContext('webgl2', { alpha: false, antialias: false, depth: false, stencil: false });
  if (!gl) {
    return null;
  }
  let texture;
  let vao;
  /// The looks compiled so far, by name; null for one that doesn't work.
  let programs;
  /// The picture's size, once there is one.
  let frame;
  let look = 'none';
  let smooth = false;
  /// A monochrome tube, whose phosphor is one colour: no colour mask.
  let mono = false;

  function setUp() {
    texture = gl.createTexture();
    gl.bindTexture(gl.TEXTURE_2D, texture);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_S, gl.CLAMP_TO_EDGE);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_T, gl.CLAMP_TO_EDGE);
    // The vertex shader makes its triangle from the vertex number.
    vao = gl.createVertexArray();
    programs = new Map();
    frame = null;
  }

  function compile(name) {
    const sources = shader_program(name);
    const program = gl.createProgram();
    const shaders = [gl.VERTEX_SHADER, gl.FRAGMENT_SHADER].map((type, i) => {
      const shader = gl.createShader(type);
      gl.shaderSource(shader, sources[i]);
      gl.compileShader(shader);
      gl.attachShader(program, shader);
      return shader;
    });
    gl.linkProgram(program);
    const linked = gl.getProgramParameter(program, gl.LINK_STATUS);
    if (!linked) {
      const logs = shaders.map((shader) => gl.getShaderInfoLog(shader));
      console.warn(`The ${name} shader doesn't work here:`, gl.getProgramInfoLog(program), ...logs);
    }
    for (const shader of shaders) {
      gl.deleteShader(shader);
    }
    if (!linked) {
      gl.deleteProgram(program);
      return null;
    }
    gl.useProgram(program);
    gl.uniform1i(gl.getUniformLocation(program, 'u_frame'), 0);
    return {
      program,
      source: gl.getUniformLocation(program, 'u_source'),
      output: gl.getUniformLocation(program, 'u_output'),
      mask: gl.getUniformLocation(program, 'u_mask'),
    };
  }

  function program(name) {
    if (!programs.has(name)) {
      programs.set(name, compile(name));
      if (!programs.get(name) && !gl.isContextLost()) {
        toast(`The ${name} CRT shader doesn't work in this browser.`, 'warn');
      }
    }
    return programs.get(name);
  }

  /// Set the texture's filter: the CRT looks read its mipmap, and the
  /// picture without one is sharp or smooth.
  function filter() {
    const mipmaps = look !== 'none';
    gl.bindTexture(gl.TEXTURE_2D, texture);
    const magnify = mipmaps || smooth ? gl.LINEAR : gl.NEAREST;
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, mipmaps ? gl.LINEAR_MIPMAP_LINEAR : magnify);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, magnify);
    if (mipmaps && frame) {
      gl.generateMipmap(gl.TEXTURE_2D);
    }
  }

  canvas.addEventListener('webglcontextlost', (event) => event.preventDefault());
  canvas.addEventListener('webglcontextrestored', () => {
    setUp();
    if (machine) {
      shownLook = undefined;
      showPicture();
      draw();
    }
  });
  setUp();

  return {
    /// Draw through the look `name`, or the plain picture, `smooth` or
    /// sharp, on a colour or a monochrome (`isMono`) tube. A look that
    /// doesn't compile leaves the picture plain.
    select(name, isSmooth, isMono) {
      look = program(name) ? name : 'none';
      smooth = isSmooth;
      mono = isMono;
      program(look);
      filter();
    },

    /// Take the picture's pixels (RGBA). Returns whether its size changed.
    upload(pixels, width, height) {
      gl.bindTexture(gl.TEXTURE_2D, texture);
      const resized = frame?.width !== width || frame?.height !== height;
      if (resized) {
        gl.texImage2D(gl.TEXTURE_2D, 0, gl.RGBA8, width, height, 0, gl.RGBA, gl.UNSIGNED_BYTE, null);
        frame = { width, height };
      }
      gl.texSubImage2D(gl.TEXTURE_2D, 0, 0, 0, width, height, gl.RGBA, gl.UNSIGNED_BYTE, pixels);
      if (look !== 'none') {
        gl.generateMipmap(gl.TEXTURE_2D);
      }
      return resized;
    },

    /// Draw the picture over the whole canvas.
    render() {
      const entry = programs.get(look);
      if (!frame || !entry || gl.isContextLost()) {
        return;
      }
      gl.viewport(0, 0, canvas.width, canvas.height);
      gl.useProgram(entry.program);
      gl.uniform2f(entry.source, frame.width, frame.height);
      gl.uniform2f(entry.output, canvas.width, canvas.height);
      gl.uniform1f(entry.mask, mono ? 0 : 1);
      gl.bindVertexArray(vao);
      gl.activeTexture(gl.TEXTURE0);
      gl.bindTexture(gl.TEXTURE_2D, texture);
      gl.drawArrays(gl.TRIANGLES, 0, 3);
    },
  };
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
    // DOSCONFIG opens the settings window.
    if (machine.settings_open() !== settingsShown) {
      syncSettings();
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
  // Ctrl+F12 opens and closes the settings window and Ctrl+F10 lets go of
  // the mouse, as in the rust-dos program and DOSBox.
  if (event.ctrlKey && !event.altKey && event.code === 'F12') {
    event.preventDefault();
    if (!event.repeat) {
      settingsInput(() => machine.toggle_settings());
    }
    return;
  }
  if (event.ctrlKey && event.code === 'F10') {
    event.preventDefault();
    document.exitPointerLock?.();
    return;
  }
  // Alt+Pause pauses the machine, holding Alt+F12 runs it fast, Ctrl+F11
  // and Ctrl+Shift+F11 slow the CPU down and speed it up, and Ctrl+F8 turns
  // the sound off and on.
  if (event.altKey && !event.ctrlKey && event.code === 'Pause') {
    event.preventDefault();
    if (!event.repeat) {
      guard(() => machine.toggle_pause());
    }
    return;
  }
  if (event.altKey && !event.ctrlKey && event.code === 'F12') {
    event.preventDefault();
    guard(() => machine.set_fast_forward(true));
    return;
  }
  if (event.ctrlKey && !event.altKey && event.code === 'F11') {
    event.preventDefault();
    guard(() => machine.step_speed(event.shiftKey));
    return;
  }
  // Ctrl+F5 saves a screenshot and Ctrl+F7 records video, as the buttons
  // do (and as in the rust-dos program).
  if (event.ctrlKey && !event.altKey && (event.code === 'F5' || event.code === 'F7')) {
    event.preventDefault();
    if (!event.repeat) {
      if (event.code === 'F5') {
        screenshot();
      } else {
        toggleRecording();
      }
    }
    return;
  }
  if (event.ctrlKey && !event.altKey && event.code === 'F8') {
    event.preventDefault();
    if (!event.repeat) {
      setSound(!speaker.muted, true);
    }
    return;
  }
  // The system's and the browser's shortcuts.
  if (event.metaKey) {
    return;
  }
  // The settings window has the keyboard while it is open.
  if (settingsShown) {
    if (event.code !== 'F11') {
      event.preventDefault();
    }
    const ctrl = event.ctrlKey && !event.getModifierState('AltGraph');
    settingsInput(() => machine.settings_key(event.key, ctrl, event.shiftKey));
    return;
  }
  const taken = guard(() => machine.key_down(event.code, event.key, event.getModifierState('AltGraph')));
  // F11 still makes the browser full screen.
  if (taken && event.code !== 'F11') {
    event.preventDefault();
  }
});

window.addEventListener('keyup', (event) => {
  // Fast forward lasts while F12 is held.
  if (event.code === 'F12' && machine) {
    guard(() => machine.set_fast_forward(false));
  }
  if (guard(() => machine.key_up(event.code))) {
    event.preventDefault();
  }
});

function releaseInput() {
  guard(() => machine.release_input());
  guard(() => machine.set_fast_forward(false));
  buttonsDown.clear();
}

window.addEventListener('blur', releaseInput);

/// Where the mouse is on the screen, in its pixels. While the mouse is
/// captured, the page moves it by the mouse's motion.
const pointer = { x: 0, y: 0 };
const buttonsDown = new Set();
const captured = () => document.pointerLockElement === canvas;

/// The screen pixel under the mouse, through the curve of a curved CRT
/// shader.
function screenPoint(event) {
  const rect = canvas.getBoundingClientRect();
  const [x, y] = machine.frame_point((event.clientX - rect.left) / rect.width, (event.clientY - rect.top) / rect.height);
  return { x, y };
}

function movePointer(event) {
  if (!machine || halted) {
    return;
  }
  if (captured()) {
    // The motion in screen pixels: the program's mickeys keep counting at
    // the edges, where the cursor stops.
    const rect = canvas.getBoundingClientRect();
    const width = machine.screen_width();
    const height = machine.screen_height();
    const dx = (event.movementX * width) / rect.width;
    const dy = (event.movementY * height) / rect.height;
    pointer.x = Math.min(Math.max(pointer.x + dx, 0), width - 1);
    pointer.y = Math.min(Math.max(pointer.y + dy, 0), height - 1);
    guard(() => machine.mouse_move_by(dx, dy));
    return;
  }
  Object.assign(pointer, screenPoint(event));
  guard(() => machine.mouse_move(pointer.x, pointer.y));
}

canvas.addEventListener('pointermove', (event) => {
  // The machine's mouse is still while the settings window is open.
  if (!settingsShown) {
    movePointer(event);
  }
});

canvas.addEventListener('pointerdown', (event) => {
  speaker.start();
  if (!machine || halted) {
    return;
  }
  event.preventDefault();
  // Clicks on the settings window go to it (see below).
  if (settingsShown) {
    return;
  }
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

// The settings window takes clicks when they end, not as they start: its
// Insert opens the file picker, which a touch may do only once it ends.
canvas.addEventListener('click', (event) => {
  if (settingsShown && event.button === 0) {
    const { x, y } = screenPoint(event);
    settingsInput(() => machine.settings_click(x, y));
  }
});

/// Wheel travel short of a notch.
let wheelTravel = 0;

canvas.addEventListener(
  'wheel',
  (event) => {
    if (!settingsShown) {
      return;
    }
    event.preventDefault();
    wheelTravel += event.deltaY * (WHEEL_UNITS[event.deltaMode] ?? 1);
    const notches = Math.trunc(wheelTravel / WHEEL_NOTCH);
    if (notches) {
      wheelTravel -= notches * WHEEL_NOTCH;
      // Up is more than 0 for the window.
      settingsInput(() => machine.settings_wheel(-notches));
    }
  },
  { passive: false },
);

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
  saveBlob(new Blob(parts, { type: 'application/octet-stream' }), name);
}

/// Offer `blob` as a file called `name` to save.
function saveBlob(blob, name) {
  const url = URL.createObjectURL(blob);
  const link = document.createElement('a');
  link.href = url;
  link.download = name;
  link.click();
  setTimeout(() => URL.revokeObjectURL(url), 60_000);
}

/// The date and time for a capture's file name, as the rust-dos program
/// names them: 2026-09-25_14-03-07.
function captureStamp() {
  const d = new Date();
  const two = (n) => String(n).padStart(2, '0');
  return `${d.getFullYear()}-${two(d.getMonth() + 1)}-${two(d.getDate())}_${two(d.getHours())}-${two(d.getMinutes())}-${two(d.getSeconds())}`;
}

/// Save the picture as a PNG image (Screenshot, Ctrl+F5).
function screenshot() {
  const png = guard(() => machine.screenshot_png());
  if (png?.length) {
    saveBlob(new Blob([png], { type: 'image/png' }), `rust-dos_screenshot_${captureStamp()}.png`);
  }
}

/// The video recording running (Record, Ctrl+F7): the screen as the page
/// shows it, CRT look and all, and the sound.
let recording = null;

function toggleRecording() {
  if (recording) {
    recording.stop();
    return;
  }
  if (!window.MediaRecorder || !canvas.captureStream) {
    toast("This browser can't record the screen.", 'warn');
    return;
  }
  const video = canvas.captureStream(60);
  let tracks = video.getVideoTracks();
  try {
    tracks = tracks.concat(speaker.record().getAudioTracks());
  } catch {
    // Without Web Audio, a recording without sound.
  }
  const types = ['video/webm;codecs=vp9,opus', 'video/webm;codecs=vp8,opus', 'video/webm', 'video/mp4'];
  const type = types.find((t) => MediaRecorder.isTypeSupported(t));
  const recorder = new MediaRecorder(new MediaStream(tracks), type ? { mimeType: type } : undefined);
  const chunks = [];
  recorder.addEventListener('dataavailable', (event) => {
    if (event.data.size) {
      chunks.push(event.data);
    }
  });
  recorder.addEventListener('stop', () => {
    speaker.stopRecording();
    video.getTracks().forEach((track) => track.stop());
    const extension = recorder.mimeType.includes('mp4') ? 'mp4' : 'webm';
    saveBlob(new Blob(chunks, { type: recorder.mimeType }), `rust-dos_video_${captureStamp()}.${extension}`);
    recording = null;
    showRecording();
  });
  recorder.start(1000);
  recording = recorder;
  showRecording();
}

function showRecording() {
  const button = $('record');
  button.textContent = recording ? 'Stop recording' : 'Record';
  button.setAttribute('aria-pressed', String(recording !== null));
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

/// The drive the image being picked goes in, or undefined for whichever
/// suits it.
let imageDrive;

function pickImage(drive) {
  imageDrive = drive;
  $('image-input').click();
}

$('open-drives').addEventListener('click', openDrives);
$('choose-image').addEventListener('click', () => pickImage(Number($('image-drive').value)));
$('image-input').addEventListener('change', (event) => {
  const [file] = event.target.files;
  event.target.value = '';
  if (file) {
    const drive = imageDrive;
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

/// Whether the settings window was open when the page last looked. The
/// emulator draws it over the screen; it has the keyboard and the mouse
/// while it is open.
let settingsShown = false;
let shownAspect;
/// The CRT look and filter WebGL draws with.
let shownLook;

/// Hand the settings window input, then take care of what came of it.
function settingsInput(action) {
  guard(() => {
    action();
    syncSettings();
    showDrives();
  });
}

/// Keep up with the settings window: it takes the keyboard and the mouse
/// from DOS as it opens, asks for disk images, saves the settings for the
/// page to keep and changes how the picture is shown.
function syncSettings() {
  const open = machine.settings_open();
  if (open && !settingsShown) {
    releaseInput();
    document.exitPointerLock?.();
  }
  settingsShown = open;
  const drive = machine.take_image_request();
  if (drive !== undefined) {
    pickImage(drive < 0 ? undefined : drive);
  }
  const text = machine.take_saved_config();
  if (text !== undefined && !remember(CONFIG_KEY, text)) {
    toast("The settings can't be kept: this browser doesn't let the page store them.", 'warn');
  }
  showPicture();
}

/// Show the picture as the settings have it: stretched to 4:3 or not,
/// sharp or smooth, through a CRT shader or not, on a colour or a
/// monochrome tube.
function showPicture() {
  if (screenGl) {
    const look = { name: machine.shader(), smooth: machine.smooth(), mono: machine.mono() };
    if (look.name !== shownLook?.name || look.smooth !== shownLook?.smooth || look.mono !== shownLook?.mono) {
      shownLook = look;
      screenGl.select(look.name, look.smooth, look.mono);
      screenGl.render();
    }
  } else {
    canvas.classList.toggle('smooth', machine.smooth());
  }
  if (machine.aspect() !== shownAspect) {
    shownAspect = machine.aspect();
    layout();
  }
}

function openSettings() {
  settingsInput(() => {
    if (!machine.settings_open()) {
      machine.toggle_settings();
    }
  });
}

/// The configuration file as text, for what the settings window doesn't
/// have: `[autoexec]`.
function openConfigFile() {
  if (!machine || halted || $('config-file').open) {
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
  openDialog($('config-file'));
}

$('open-settings').addEventListener('click', openSettings);
$('open-config').addEventListener('click', openConfigFile);
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
  setSound(!speaker.muted, false);
});

$('screenshot').addEventListener('click', () => {
  if (machine) {
    screenshot();
  }
});

$('record').addEventListener('click', () => {
  if (machine) {
    toggleRecording();
  }
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
if (screenGl) {
  watchPixelRatio();
}

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
  machine.set_muted(speaker.muted, false);
  if (params.has('log')) {
    machine.log_to_console();
  }
  machine.set_shaders_available(screenGl !== null);
  for (const warning of machine.warnings()) {
    toast(`Settings: ${warning}`, 'warn');
  }
  await setUpC();
  for (const url of params.getAll('zip')) {
    await fetchZip(url);
  }
  showPicture();
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
