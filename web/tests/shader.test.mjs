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
        shader_revision: () => 7,
        shader_overlay_revision: () => 9,
      },
      screenGl: {
        xbrz: () => xbrz,
        upload: (...args) => { upload = args; return xbrz; },
        render: () => renders++,
      },
      layout: () => layouts++,
    });
    draw();
    const [pixels, width, height, overlay, revision] = upload;
    assert.equal(revision, xbrz ? 7 : undefined);
    assert.equal(width, xbrz ? 2 : 4);
    assert.equal(height, width);
    assert.equal(pixels.length, width * height * 4);
    assert.ok(pixels.every(value => value === (xbrz ? 42 : 17)));
    if (xbrz && visible) {
      assert.equal(overlay.width, 4);
      assert.equal(overlay.height, 4);
      assert.equal(overlay.pixels.length, 64);
      assert.ok(overlay.pixels.every(value => value === 33));
      assert.equal(overlay.revision, 9);
    } else {
      assert.equal(overlay, null);
    }
    assert.equal(layouts, xbrz ? 1 : 0);
    assert.equal(renders, xbrz ? 0 : 1);
  });
}

function rendererFixture() {
  const calls = [];
  const events = new Map();
  const warnings = [];
  let textures = 0;
  let framebuffers = 0;
  let boundFramebuffer = null;
  const enabled = new Set(['DITHER']);
  const logs = [];
  const gl = new Proxy({
    createTexture: () => ++textures,
    createFramebuffer: () => ({ framebuffer: ++framebuffers }),
    bindFramebuffer: (target, value) => {
      calls.push(['bindFramebuffer', target, value]);
      boundFramebuffer = value;
    },
    getParameter: name => name === 'MAX_TEXTURE_SIZE' ? 4096
      : name === 'MAX_VIEWPORT_DIMS' ? [4096, 4096] : boundFramebuffer,
    isEnabled: state => enabled.has(state),
    enable: state => enabled.add(state),
    disable: state => enabled.delete(state),
    checkFramebufferStatus: () => 'FRAMEBUFFER_COMPLETE',
    getError: () => 'NO_ERROR',
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
  const canvas = {
    width: 4,
    height: 4,
    getContext: () => gl,
    addEventListener: (name, callback) => events.set(name, callback),
  };
  const renderer = load('makeGlScreen', {
    Uint8Array,
    canvas,
    shader_program: name => ['vertex', name],
    toast: message => warnings.push(message),
    console: { warn: (...args) => logs.push(args) },
    machine: null,
  })();
  return { renderer, calls, events, warnings, logs, gl, canvas, enabled };
}

test('WebGL handles overlay removal, shader fallback and context restoration', () => {
  const { renderer, calls, events, warnings } = rendererFixture();
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

test('xBRZ reuses base passes across overlays and resizes, and restores GL state', () => {
  const { renderer, calls, events, gl, canvas, enabled } = rendererFixture();
  const pixels = new Uint8Array(16);
  const overlay = { pixels: new Uint8Array(64), width: 4, height: 4, revision: 1 };
  const draws = () => calls.filter(([name]) => name === 'drawArrays').length;
  const uploads = () => calls.filter(([name]) => name === 'texSubImage2D');
  const output = { external: true };
  gl.bindFramebuffer(gl.DRAW_FRAMEBUFFER, output);
  gl.enable(gl.BLEND);
  gl.enable(gl.SCISSOR_TEST);
  renderer.select('xbrz', false, false, [0, 0], 0);
  renderer.upload(pixels, 2, 2, overlay, 1);
  calls.length = 0;
  renderer.render();
  assert.equal(draws(), 3, 'classify, resolve 2x, display');
  assert.equal(gl.getParameter(gl.DRAW_FRAMEBUFFER_BINDING), output);
  assert.deepEqual([...enabled].sort(), ['BLEND', 'DITHER', 'SCISSOR_TEST']);
  calls.length = 0;
  renderer.upload(pixels, 2, 2, overlay, 1);
  renderer.render();
  assert.equal(uploads().length, 0);
  assert.equal(draws(), 1);
  calls.length = 0;
  renderer.upload(pixels, 2, 2, { ...overlay, revision: 2 }, 1);
  renderer.render();
  assert.equal(uploads().length, 1);
  assert.equal(uploads()[0][9].length, 64, 'only the overlay uploads');
  assert.equal(draws(), 1);
  calls.length = 0;
  renderer.upload(pixels, 2, 2, { ...overlay, revision: 2 }, 2);
  renderer.render();
  assert.equal(uploads().length, 1);
  assert.equal(uploads()[0][9].length, 16, 'only the source uploads');
  assert.equal(draws(), 3);
  canvas.width = canvas.height = 2;
  calls.length = 0;
  renderer.render();
  assert.equal(draws(), 1, 'small output resolves directly with cached corners');
  calls.length = 0;
  renderer.upload(pixels, 2, 2, null, 3);
  renderer.render();
  assert.equal(draws(), 2, 'classify and direct resolve, without a 2x pass');
  canvas.width = canvas.height = 8;
  calls.length = 0;
  renderer.render();
  assert.equal(draws(), 2, 'reuse corners, build the 2x base and display');
  canvas.width = canvas.height = 10;
  calls.length = 0;
  renderer.render();
  assert.equal(draws(), 1, 'resizing keeps the filtered base');
  calls.length = 0;
  renderer.upload(new Uint8Array(12), 3, 1, null, 4);
  renderer.render();
  assert.equal(draws(), 3);
  assert.equal(calls.filter(([name]) => name === 'deleteFramebuffer').length, 2);
  events.get('webglcontextrestored')();
  renderer.select('xbrz', false, false, [0, 0], 0);
  calls.length = 0;
  renderer.upload(new Uint8Array(12), 3, 1, null, 4);
  renderer.render();
  assert.equal(uploads().length, 1, 'a restored context uploads even the same revision');
  assert.equal(draws(), 3);
});

test('xBRZ target failure retains direct filtering without retrying the allocation', () => {
  const { renderer, calls, logs, gl, enabled } = rendererFixture();
  const output = { external: true };
  gl.bindFramebuffer(gl.DRAW_FRAMEBUFFER, output);
  gl.enable(gl.BLEND);
  gl.enable(gl.SCISSOR_TEST);
  gl.checkFramebufferStatus = () => 'INCOMPLETE';
  renderer.select('xbrz', false, false, [0, 0], 0);
  renderer.upload(new Uint8Array(16), 2, 2, null, 1);
  calls.length = 0;
  renderer.render();
  renderer.render();
  assert.equal(renderer.xbrz(), true);
  assert.equal(logs.length, 1);
  assert.equal(calls.filter(([name]) => name === 'drawArrays').length, 2);
  assert.equal(calls.filter(([name]) => name === 'framebufferTexture2D').length, 1);
  assert.equal(gl.getParameter(gl.DRAW_FRAMEBUFFER_BINDING), output);
  assert.deepEqual([...enabled].sort(), ['BLEND', 'DITHER', 'SCISSOR_TEST']);
});

test('xBRZ starts at small output without allocating a 2x target', () => {
  const { renderer, calls, canvas } = rendererFixture();
  canvas.width = canvas.height = 2;
  renderer.select('xbrz', false, false, [0, 0], 0);
  renderer.upload(new Uint8Array(16), 2, 2, null, 1);
  calls.length = 0;
  renderer.render();
  assert.equal(calls.filter(([name]) => name === 'drawArrays').length, 2);
  assert.equal(calls.some(call => call[0] === 'texImage2D' && call[4] === 4), false);
  assert.equal(calls.filter(([name]) => name === 'framebufferTexture2D').length, 1);
});

test('minification skips source-sized passes', () => {
  const { renderer, calls, canvas } = rendererFixture();
  canvas.width = canvas.height = 1;
  renderer.select('xbrz', false, false, [0, 0], 0);
  renderer.upload(new Uint8Array(16), 2, 2, null, 1);
  calls.length = 0;
  renderer.render();
  assert.equal(calls.filter(([name]) => name === 'drawArrays').length, 1);
  assert.equal(calls.filter(([name]) => name === 'framebufferTexture2D').length, 0);
  canvas.width = canvas.height = 4;
  calls.length = 0;
  renderer.render();
  assert.equal(calls.filter(([name]) => name === 'drawArrays').length, 3);
});

test('viewport limits cannot leave a partially classified corner texture', () => {
  const { renderer, calls, gl, logs } = rendererFixture();
  const getParameter = gl.getParameter;
  gl.getParameter = name => name === 'MAX_VIEWPORT_DIMS' ? [2, 2] : getParameter(name);
  renderer.select('xbrz', false, false, [0, 0], 0);
  renderer.upload(new Uint8Array(16), 2, 2, null, 1);
  calls.length = 0;
  renderer.render();
  assert.equal(calls.filter(([name]) => name === 'drawArrays').length, 1);
  assert.equal(calls.filter(([name]) => name === 'framebufferTexture2D').length, 0);
  assert.equal(logs.length, 0);
  calls.length = 0;
  renderer.upload(new Uint8Array(4), 1, 1, null, 2);
  renderer.render();
  assert.equal(calls.filter(([name]) => name === 'drawArrays').length, 3);
});

test('xBRZ pass compilation failure keeps the direct shader available', () => {
  const { renderer, calls, warnings, gl } = rendererFixture();
  gl.getProgramParameter = program => !program.shaders.some(shader => shader.text === 'xbrz-corners');
  renderer.select('xbrz', false, false, [0, 0], 0);
  renderer.upload(new Uint8Array(16), 2, 2, null, 1);
  calls.length = 0;
  renderer.render();
  assert.equal(renderer.xbrz(), true);
  assert.equal(warnings.length, 0, 'the working filter needs no failure toast');
  assert.equal(calls.filter(([name]) => name === 'drawArrays').length, 1);
});

test('texture limits retain a direct resolve when the 2x target does not fit', () => {
  const { renderer, calls, gl, canvas } = rendererFixture();
  const getParameter = gl.getParameter;
  gl.getParameter = name => name === 'MAX_TEXTURE_SIZE' ? 4 : getParameter(name);
  canvas.width = canvas.height = 12;
  renderer.select('xbrz', false, false, [0, 0], 0);
  renderer.upload(new Uint8Array(36), 3, 3, null, 1);
  calls.length = 0;
  renderer.render();
  assert.equal(calls.filter(([name]) => name === 'drawArrays').length, 2);
  assert.equal(calls.some(call => call[0] === 'texImage2D' && call[4] === 6), false);
});

test('shader switching cannot reuse composed pixels as the xBRZ base', () => {
  const { renderer, calls } = rendererFixture();
  const pixels = new Uint8Array(16);
  renderer.select('xbrz', false, false, [0, 0], 0);
  renderer.upload(pixels, 2, 2, null, 1);
  renderer.render();
  renderer.select('crt', false, false, [0, 0], 0);
  renderer.upload(pixels, 2, 2);
  renderer.render();
  renderer.select('xbrz', false, false, [0, 0], 0);
  calls.length = 0;
  renderer.upload(pixels, 2, 2, null, 1);
  renderer.render();
  assert.equal(calls.filter(([name]) => name === 'texSubImage2D').length, 1);
  assert.equal(calls.filter(([name]) => name === 'drawArrays').length, 3);
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
