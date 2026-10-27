import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';
import vm from 'node:vm';

const source = readFileSync(new URL('../www/main.js', import.meta.url), 'utf8');

// Extract top-level functions and run the real page code with stubbed globals.
// Their closing braces start at column zero, so tests need no browser parser.
function load(name, globals) {
  const start = source.indexOf(`function ${name}()`);
  assert.notEqual(start, -1);
  const end = source.indexOf('\n}\n', start) + 3;
  return vm.runInNewContext(`${source.slice(start, end)}\n${name}`, globals);
}

for (const [xbrz, visible] of [[false, false], [true, false], [true, true]]) {
  test(`draw uploads ${xbrz ? 'native' : 'composed'} pixels, overlay=${visible}`, () => {
    const buffer = new ArrayBuffer(256);
    new Uint8Array(buffer, 0, 64).fill(17);
    new Uint8Array(buffer, 64, 16).fill(42);
    new Uint8Array(buffer, 128, 64).fill(33);
    let upload;
    let layouts = 0;
    let renders = 0;
    const draw = load('draw', {
      Uint8Array,
      wasm: { memory: { buffer } },
      machine: {
        screen_width: () => 4,
        screen_height: () => 4,
        screen_pixels: () => 0,
        shader_width: () => 2,
        shader_height: () => 2,
        shader_pixels: () => 64,
        shader_overlay_pixels: () => 128,
        shader_overlay_visible: () => visible,
      },
      screenGl: {
        xbrz: () => xbrz,
        upload: (...args) => { upload = args; return xbrz; },
        render: () => renders++,
      },
      layout: () => layouts++,
    });
    draw();
    const [pixels, width, height, overlay] = upload;
    assert.equal(width, xbrz ? 2 : 4);
    assert.equal(height, width);
    assert.equal(pixels.length, width * height * 4);
    assert.ok(pixels.every(value => value === (xbrz ? 42 : 17)));
    if (xbrz && visible) {
      assert.equal(overlay.width, 4);
      assert.equal(overlay.height, 4);
      assert.equal(overlay.pixels.length, 64);
      assert.ok(overlay.pixels.every(value => value === 33));
    } else {
      assert.equal(overlay, null);
    }
    assert.equal(layouts, xbrz ? 1 : 0);
    assert.equal(renders, xbrz ? 0 : 1);
  });
}

test('WebGL handles overlay removal, shader fallback and context restoration', () => {
  const calls = [];
  const events = new Map();
  const warnings = [];
  let textures = 0;
  const gl = new Proxy({
    createTexture: () => ++textures,
    createProgram: () => ({ shaders: [] }),
    createShader: () => ({}),
    shaderSource: (shader, text) => { shader.text = text; },
    attachShader: (program, shader) => program.shaders.push(shader),
    getProgramParameter: program => !program.shaders.some(shader => shader.text === 'broken'),
    getUniformLocation: (_program, name) => name,
    getProgramInfoLog: () => '',
    getShaderInfoLog: () => '',
    isContextLost: () => false,
  }, {
    get(target, key) {
      if (key in target) return target[key];
      if (/^[A-Z0-9_]+$/.test(key)) return key;
      return (...args) => calls.push([key, ...args]);
    },
  });
  const renderer = load('makeGlScreen', {
    Uint8Array,
    canvas: {
      width: 4,
      height: 4,
      getContext: () => gl,
      addEventListener: (name, callback) => events.set(name, callback),
    },
    shader_program: name => ['vertex', name],
    toast: message => warnings.push(message),
    console: { warn() {} },
    machine: null,
  })();
  const pixels = new Uint8Array(16);
  renderer.select('xbrz', false, false, [0, 0], 0);
  assert.equal(renderer.xbrz(), true);
  assert.ok(calls.some(([name, uniform, value]) =>
    name === 'uniform1i' && uniform === 'u_overlay' && value === 1));
  calls.length = 0;
  assert.equal(
    renderer.upload(pixels, 2, 2, { pixels: new Uint8Array(64), width: 4, height: 4 }),
    true);
  renderer.render();
  assert.ok(calls.some(call =>
    call[0] === 'texImage2D' && call[4] === 4 && call[5] === 4 && call[9] === null));
  assert.ok(calls.some(call => call[0] === 'texSubImage2D' && call[9]?.length === 64));
  assert.ok(calls.some(([name, uniform, width, height]) =>
    name === 'uniform2f' && uniform === 'u_source' && width === 2 && height === 2));
  assert.ok(calls.some(([name, uniform, value]) =>
    name === 'uniform1i' && uniform === 'u_overlay_enabled' && value === 1));
  assert.equal(calls.some(([name]) => name === 'generateMipmap'), false);
  calls.length = 0;
  renderer.upload(pixels, 2, 2, { pixels: new Uint8Array(64), width: 4, height: 4 });
  assert.equal(calls.some(([name]) => name === 'texImage2D'), false);
  assert.ok(calls.some(([name]) => name === 'texSubImage2D'));
  calls.length = 0;
  assert.equal(renderer.upload(pixels, 2, 2), false);
  renderer.render();
  assert.ok(calls.some(([name, uniform, value]) =>
    name === 'uniform1i' && uniform === 'u_overlay_enabled' && value === 0));
  renderer.select('crt', false, false, [0, 0], 0);
  assert.equal(renderer.xbrz(), false);
  calls.length = 0;
  renderer.upload(pixels, 2, 2);
  assert.ok(calls.some(([name]) => name === 'generateMipmap'));
  renderer.select('broken', false, false, [0, 0], 0);
  assert.equal(renderer.xbrz(), false);
  assert.equal(warnings.length, 1);
  renderer.select('xbrz', false, false, [0, 0], 0);
  renderer.upload(pixels, 2, 2, { pixels: new Uint8Array(64), width: 4, height: 4 });
  events.get('webglcontextrestored')();
  renderer.select('xbrz', false, false, [0, 0], 0);
  calls.length = 0;
  renderer.upload(pixels, 2, 2);
  renderer.render();
  assert.ok(calls.some(([name, uniform, value]) =>
    name === 'uniform1i' && uniform === 'u_overlay_enabled' && value === 0));
  calls.length = 0;
  renderer.upload(pixels, 2, 2, { pixels: new Uint8Array(64), width: 4, height: 4 });
  assert.ok(calls.some(call => call[0] === 'texImage2D' && call[4] === 4 && call[5] === 4));
  assert.ok(calls.some(([name]) => name === 'texSubImage2D'));
});

test('showPicture uploads the current frame when switching shaders', () => {
  let renders = 0;
  let selected;
  let draws = 0;
  const show = load('showPicture', {
    screenGl: { select: (...args) => { selected = args; }, xbrz: () => true, render: () => renders++ },
    machine: {
      screen_width: () => 4, screen_height: () => 4,
      shader_width: () => 2, shader_height: () => 2,
      shader: () => 'xbrz', smooth: () => false, mono: () => false,
      shader_curvature: () => [0, 0], shader_glow: () => 0, aspect: () => '4:3',
    },
    shownLook: null, shownAspect: '4:3', layout() {}, draw: () => draws++,
  });
  show();
  assert.equal(selected[0], 'xbrz');
  assert.equal(renders, 0);
  assert.equal(draws, 1);
});
