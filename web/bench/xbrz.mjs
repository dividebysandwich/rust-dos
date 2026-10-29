// Experimental passes stay here until correctness and timings justify them.
// No emulator, requestAnimationFrame, compositor, or vsync is in this timing.
window.runBenchmark = async function ({ samples = 6, rounds = 3 } = {}) {
  const canvas = document.createElement('canvas');
  const gl = canvas.getContext('webgl2', { alpha: false, antialias: false, depth: false, stencil: false });
  if (!gl) throw new Error('WebGL 2 is unavailable');
  let drawCount = 0;
  let textureAllocations = 0;
  const drawArrays = gl.drawArrays.bind(gl);
  gl.drawArrays = (...args) => { drawCount++; drawArrays(...args); };
  const texImage2D = gl.texImage2D.bind(gl);
  gl.texImage2D = (...args) => { textureAllocations++; texImage2D(...args); };
  const timer = gl.getExtension('EXT_disjoint_timer_query_webgl2');
  const debug = gl.getExtension('WEBGL_debug_renderer_info');
  const preamble = '#version 300 es\nprecision highp float;\nprecision highp int;\n';
  const [vertex, kernel, copy, page] = await Promise.all([
    '/src/video/shader/vertex.glsl', '/src/video/shader/xbrz_2x.glsl',
    '/src/video/shader/xbrz_display.glsl', '/web/www/main.js',
  ].map(async path => {
    const response = await fetch(path);
    if (!response.ok) throw new Error(`Source fetch failed: ${response.status}`);
    return response.text();
  }));
  const fragment = '#define XBRZ_REFERENCE\n' + kernel;
  const classify = '#define XBRZ_CORNERS\n' + kernel;
  const withCorners = '#define XBRZ_REFERENCE\n#define XBRZ_PRECOMPUTED\n' + kernel;
  const selectedCornerSource = '#define XBRZ_PRECOMPUTED\n' + kernel;
  const begin = page.indexOf('function makeGlScreen()');
  const end = page.indexOf('\n}\n', begin) + 3;
  if (begin < 0 || end < begin) throw new Error('The renderer marker changed');
  const liveCanvas = { width: 1, height: 1, getContext: () => gl, addEventListener() {} };
  const liveSources = name => {
    const body = { 'xbrz-corners': classify, 'xbrz-resolve': selectedCornerSource,
      'xbrz-display': copy, xbrz: kernel }[name];
    if (!body) throw new Error(`Unexpected benchmark shader: ${name}`);
    return [preamble + vertex, preamble + body];
  };
  const production = new Function('canvas', 'shader_program', 'toast', 'machine',
    page.slice(begin, end) + '\nreturn makeGlScreen();')(liveCanvas, liveSources,
    message => { throw new Error(message); }, null);
  const selectStart = performance.now();
  production.select('xbrz', false, false, [0, 0], 0);
  const productionSelectMs = performance.now() - selectStart;
  let revision = 0;
  const programs = [];
  function program(body, vs = vertex) {
    const shaders = [gl.VERTEX_SHADER, gl.FRAGMENT_SHADER].map((type, i) => {
      const shader = gl.createShader(type);
      gl.shaderSource(shader, preamble + (i ? body : vs));
      gl.compileShader(shader);
      if (!gl.getShaderParameter(shader, gl.COMPILE_STATUS)) {
        throw new Error(gl.getShaderInfoLog(shader));
      }
      return shader;
    });
    const entry = gl.createProgram();
    for (const shader of shaders) gl.attachShader(entry, shader);
    gl.linkProgram(entry);
    for (const shader of shaders) gl.deleteShader(shader);
    if (!gl.getProgramParameter(entry, gl.LINK_STATUS)) throw new Error(gl.getProgramInfoLog(entry));
    gl.useProgram(entry);
    gl.uniform1i(gl.getUniformLocation(entry, 'u_frame'), 0);
    gl.uniform1i(gl.getUniformLocation(entry, 'u_overlay'), 1);
    gl.uniform1i(gl.getUniformLocation(entry, 'u_overlay_enabled'), 0);
    gl.uniform1i(gl.getUniformLocation(entry, 'u_corners'), 2);
    programs.push(entry);
    return { program: entry, source: gl.getUniformLocation(entry, 'u_source') };
  }
  const direct = program(fragment);
  const prepass = program(classify);
  const resolve = program(withCorners);
  const selected = program(kernel);
  const selectedCorners = program(selectedCornerSource);
  // Intermediate rows must retain source order, before the display flip.
  const cached = program(fragment, vertex.replace('1.0 - p.y', 'p.y'));
  const cachedSelected = program(kernel, vertex.replace('1.0 - p.y', 'p.y'));
  const cachedCorners = program(selectedCornerSource, vertex.replace('1.0 - p.y', 'p.y'));
  const plain = program(copy);
  const vao = gl.createVertexArray();
  gl.bindVertexArray(vao);
  gl.disable(gl.DITHER);
  function texture(width, height, pixels = null) {
    const value = gl.createTexture();
    gl.activeTexture(gl.TEXTURE0);
    gl.bindTexture(gl.TEXTURE_2D, value);
    for (const key of [gl.TEXTURE_MIN_FILTER, gl.TEXTURE_MAG_FILTER]) {
      gl.texParameteri(gl.TEXTURE_2D, key, gl.NEAREST);
    }
    gl.texImage2D(gl.TEXTURE_2D, 0, gl.RGBA8, width, height, 0, gl.RGBA, gl.UNSIGNED_BYTE, pixels);
    return value;
  }
  function target(width, height) {
    const value = texture(width, height);
    const framebuffer = gl.createFramebuffer();
    gl.bindFramebuffer(gl.FRAMEBUFFER, framebuffer);
    gl.framebufferTexture2D(gl.FRAMEBUFFER, gl.COLOR_ATTACHMENT0, gl.TEXTURE_2D, value, 0);
    if (gl.checkFramebufferStatus(gl.FRAMEBUFFER) !== gl.FRAMEBUFFER_COMPLETE) {
      throw new Error('Incomplete benchmark framebuffer');
    }
    return { texture: value, framebuffer, width, height };
  }
  const overlay = texture(1, 1, new Uint8Array(4));
  gl.activeTexture(gl.TEXTURE1);
  gl.bindTexture(gl.TEXTURE_2D, overlay);
  function draw(entry, source, destination, width, height) {
    gl.bindVertexArray(vao);
    gl.bindFramebuffer(gl.FRAMEBUFFER, destination.framebuffer);
    gl.viewport(0, 0, destination.width, destination.height);
    gl.useProgram(entry.program);
    gl.uniform2f(entry.source, width, height);
    gl.activeTexture(gl.TEXTURE0);
    gl.bindTexture(gl.TEXTURE_2D, source);
    gl.drawArrays(gl.TRIANGLES, 0, 3);
  }
  const pause = () => new Promise(resolve => setTimeout(resolve, 1));
  const syncPixel = new Uint8Array(4);
  async function measure(render) {
    const query = timer ? gl.createQuery() : null;
    if (query) gl.beginQuery(timer.TIME_ELAPSED_EXT, query);
    const start = performance.now();
    render();
    if (query) gl.endQuery(timer.TIME_ELAPSED_EXT);
    gl.finish();
    // Browser finish can return after submission. Readback is the wall
    // fallback's completion fence when GPU timers are unavailable.
    if (!query) gl.readPixels(0, 0, 1, 1, gl.RGBA, gl.UNSIGNED_BYTE, syncPixel);
    const wallMs = performance.now() - start;
    let gpuMs = null;
    if (query) {
      for (let attempts = 0; !gl.getQueryParameter(query, gl.QUERY_RESULT_AVAILABLE); attempts++) {
        if (attempts > 10000) throw new Error('GPU timer did not complete');
        await pause();
      }
      if (!gl.getParameter(timer.GPU_DISJOINT_EXT)) {
        gpuMs = gl.getQueryParameter(query, gl.QUERY_RESULT) / 1e6;
      }
      gl.deleteQuery(query);
    }
    const error = gl.getError();
    if (error !== gl.NO_ERROR) throw new Error(`WebGL error: ${error}`);
    return { wallMs, gpuMs };
  }
  function fixture(width, height, pattern) {
    const pixels = new Uint8Array(width * height * 4);
    let seed = 0x2545f491;
    for (let y = 0; y < height; y++) {
      for (let x = 0; x < width; x++) {
        seed ^= seed << 13; seed ^= seed >>> 17; seed ^= seed << 5;
        const colour = pattern === 'noise' ? seed >>> 0 : Math.floor(x / 7) + Math.floor(y / 5);
        const at = (y * width + x) * 4;
        if (pattern === 'near-grey') {
          const grey = [0, 16, 32, 40, 80, 160, 255][(seed >>> 0) % 7];
          pixels.set([grey, grey, grey, 255], at);
        } else {
          pixels.set([(colour * 17) & 255, (colour * 43) & 255, (colour * 71) & 255,
            pattern === 'alpha' ? (x || y ? seed >>> 24 : 0) : 255], at);
        }
      }
    }
    return pixels;
  }
  const results = [];
  const checks = [
    [1, 1, 2, 2, 'alpha'], [1, 7, 3, 21, 'alpha'],
    [7, 1, 21, 3, 'alpha'], [5, 5, 10, 10, 'near-grey'],
    [7, 5, 23, 17, 'alpha'], [7, 5, 23, 17, 'near-grey'],
  ];
  const scenarios = [[320, 200, 160, 100], [320, 200, 320, 200], [320, 200, 640, 400],
    [320, 200, 1920, 1440], [640, 480, 1920, 1440]]
    .flatMap(size => ['pixel-art', 'noise'].map(pattern => [...size, pattern]));
  for (const [width, height, outWidth, outHeight, pattern] of [...checks, ...scenarios]) {
    liveCanvas.width = outWidth;
    liveCanvas.height = outHeight;
    const pixels = fixture(width, height, pattern);
    const source = texture(width, height, pixels);
    // One extra top/left corner preserves blend_of's boundary rules.
    const corners = target(width + 1, height + 1);
    const scaled = target(width * 2, height * 2);
    const output = target(outWidth, outHeight);
    const bindCorners = () => {
      gl.activeTexture(gl.TEXTURE2);
      gl.bindTexture(gl.TEXTURE_2D, corners.texture);
    };
    let productionDraws = 0;
    const variants = {
      direct: () => draw(direct, source, output, width, height),
      production_fresh: () => {
        production.upload(pixels, width, height, null, ++revision);
        gl.bindFramebuffer(gl.FRAMEBUFFER, output.framebuffer);
        const before = drawCount;
        production.render();
        productionDraws = drawCount - before;
      },
      production_reuse: () => {
        production.upload(pixels, width, height, null, revision);
        gl.bindFramebuffer(gl.FRAMEBUFFER, output.framebuffer);
        const before = drawCount;
        production.render();
        productionDraws = drawCount - before;
      },
      selected: () => draw(selected, source, output, width, height),
      corners: () => {
        draw(prepass, source, corners, width, height);
        bindCorners();
        draw(resolve, source, output, width, height);
      },
      corners_reuse: () => {
        bindCorners();
        draw(resolve, source, output, width, height);
      },
      corners_selected: () => {
        draw(prepass, source, corners, width, height);
        bindCorners();
        draw(selectedCorners, source, output, width, height);
      },
      fixed_2x: () => {
        draw(cached, source, scaled, width, height);
        draw(plain, scaled.texture, output, width * 2, height * 2);
      },
      fixed_2x_selected: () => {
        draw(cachedSelected, source, scaled, width, height);
        draw(plain, scaled.texture, output, width * 2, height * 2);
      },
      fixed_2x_corners: () => {
        draw(prepass, source, corners, width, height);
        bindCorners();
        draw(cachedCorners, source, scaled, width, height);
        draw(plain, scaled.texture, output, width * 2, height * 2);
      },
      fixed_2x_reuse: () => draw(plain, scaled.texture, output, width * 2, height * 2),
      nearest: () => draw(plain, source, output, width, height),
      upload_nearest: () => {
        gl.activeTexture(gl.TEXTURE0);
        gl.bindTexture(gl.TEXTURE_2D, source);
        gl.texSubImage2D(gl.TEXTURE_2D, 0, 0, 0, width, height, gl.RGBA, gl.UNSIGNED_BYTE, pixels);
        draw(plain, source, output, width, height);
      },
    };
    const read = () => {
      gl.bindFramebuffer(gl.FRAMEBUFFER, output.framebuffer);
      const data = new Uint8Array(outWidth * outHeight * 4);
      gl.readPixels(0, 0, outWidth, outHeight, gl.RGBA, gl.UNSIGNED_BYTE, data);
      return data;
    };
    let productionFirstFrame;
    for (let update = 0; update < (width < 10 ? 2 : 1); update++) {
      if (update) {
        pixels[0] ^= 127;
        gl.activeTexture(gl.TEXTURE0);
        gl.bindTexture(gl.TEXTURE_2D, source);
        gl.texSubImage2D(gl.TEXTURE_2D, 0, 0, 0, width, height, gl.RGBA, gl.UNSIGNED_BYTE, pixels);
      }
      variants.direct();
      const expected = read();
      // Force every render pass once before validating the reuse cases.
      for (const [name, render] of Object.entries(variants)) {
        if (name === 'production_fresh' && update === 0) {
          productionFirstFrame = await measure(render);
        } else {
          render();
        }
        if (name.startsWith('production_')) {
          const magnified = outWidth >= width * 2 && outHeight >= height * 2;
          const minified = outWidth < width || outHeight < height;
          const expectedDraws = name === 'production_reuse' || minified ? 1 : magnified ? 3 : 2;
          if (productionDraws !== expectedDraws) throw new Error(`Unexpected production passes: ${productionDraws} != ${expectedDraws}`);
        }
        const actual = read();
        if (!name.endsWith('nearest')) {
          const at = actual.findIndex((value, i) => value !== expected[i]);
          if (at !== -1) throw new Error(`${name}, ${width}x${height} ${pattern}, update ${update}: byte ${at}: ${actual[at]} != ${expected[at]}`);
        }
      }
    }
    gl.finish();
    const allocationsBeforeTiming = textureAllocations;
    const times = Object.fromEntries(Object.keys(variants).map(name => [name, []]));
    for (let round = 0; width >= 10 && round < rounds; round++) {
      // Rotate the order between rounds to reduce systematic warm-up bias.
      const names = Object.keys(variants);
      const order = [...names.slice(round % names.length), ...names.slice(0, round % names.length)];
      for (const name of order) {
        for (let i = 0; i < 3; i++) await measure(variants[name]);
        for (let i = 0; i < samples; i++) times[name].push(await measure(variants[name]));
      }
    }
    if (textureAllocations !== allocationsBeforeTiming) throw new Error('Per-frame texture allocation');
    if (width >= 10) results.push({
      width, height, outWidth, outHeight, pattern, outputEqual: true,
      intermediateBytes: { corners: (width + 1) * (height + 1) * 4, fixed2x: width * height * 16 },
      productionFirstFrame,
      textureAllocationsDuringTiming: textureAllocations - allocationsBeforeTiming, times,
    });
    console.log(`${width >= 10 ? 'Measured' : 'Validated'} ${width}x${height} -> ${outWidth}x${outHeight}, ${pattern}`);
    for (const entry of [corners, scaled, output]) {
      gl.deleteFramebuffer(entry.framebuffer);
      gl.deleteTexture(entry.texture);
    }
    gl.deleteTexture(source);
  }
  for (const entry of programs) gl.deleteProgram(entry);
  gl.deleteTexture(overlay);
  gl.deleteVertexArray(vao);
  return {
    browser: navigator.userAgent,
    renderer: gl.getParameter(debug ? debug.UNMASKED_RENDERER_WEBGL : gl.RENDERER),
    vendor: gl.getParameter(debug ? debug.UNMASKED_VENDOR_WEBGL : gl.VENDOR),
    gpuTimer: !!timer,
    samples, rounds, workloadVersion: 2, edgeChecks: checks.length, productionSelectMs,
    boundary: 'Persistent RGBA8 targets, dithering and overlays disabled. Draw passes only, except production_fresh and upload_nearest include a full source upload. Steady-state timings exclude allocations, emulator, compositor and vsync. productionFirstFrame includes initial target allocation with warmed shaders. GPU queries are asynchronous; disjoint samples are null. Wall time is CPU submission with timers, or completed rendering plus one-pixel synchronous readback without timers.',
    results,
  };
};
