// Node's built-in HTTP, WebSocket and child-process APIs keep this dependency free.
import { createServer } from 'node:http';
import { readFile, mkdtemp, rm, writeFile } from 'node:fs/promises';
import { spawn, execFileSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { cpus, tmpdir } from 'node:os';
import { dirname, resolve, sep, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
const hash = data => createHash('sha256').update(data).digest('hex');
const benchmarkSha256 = hash(await readFile(join(root, 'web/bench/xbrz.mjs')));
const runnerSha256 = hash(await readFile(fileURLToPath(import.meta.url)));
const rendererPath = resolve(process.env.XBRZ_RENDERER || join(root, 'web/www/main.js'));
const swiftshader = process.argv.includes('--swiftshader');
const output = process.argv.slice(2).find(arg => !arg.startsWith('--')) || '/tmp/xbrz-webgl.json';
const server = createServer(async (request, response) => {
  try {
    const path = resolve(root, '.' + new URL(request.url, 'http://localhost').pathname);
    if (!path.startsWith(root + sep)) throw new Error('Path outside repository');
    response.setHeader('Content-Type', path.endsWith('.mjs') ? 'text/javascript' : path.endsWith('.html') ? 'text/html' : 'text/plain');
    response.end(await readFile(path === join(root, 'web/www/main.js') ? rendererPath : path));
  } catch {
    response.writeHead(404).end();
  }
});
await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
const profile = await mkdtemp(join(tmpdir(), 'xbrz-bench-'));
const args = ['--headless=new', '--no-first-run', '--disable-background-networking',
  '--remote-debugging-port=0', `--user-data-dir=${profile}`, 'about:blank'];
if (swiftshader) args.unshift('--use-angle=swiftshader', '--enable-unsafe-swiftshader');
if (process.argv.includes('--no-sandbox')) args.unshift('--no-sandbox');
const browser = spawn(process.env.CHROME_BIN || 'google-chrome', args, { stdio: ['ignore', 'ignore', 'pipe'] });
let socket;
try {
  const endpoint = await new Promise((resolve, reject) => {
    let log = '';
    const timeout = setTimeout(() => reject(new Error(`Chrome did not start: ${log}`)), 20000);
    browser.on('error', reject);
    browser.on('exit', code => reject(new Error(`Chrome exited (${code}): ${log}`)));
    browser.stderr.on('data', data => {
      log += data;
      const match = log.match(/DevTools listening on (ws:\/\/\S+)/);
      if (match) { clearTimeout(timeout); resolve(match[1]); }
    });
  });
  socket = new WebSocket(endpoint);
  await new Promise((resolve, reject) => {
    socket.addEventListener('open', resolve, { once: true });
    socket.addEventListener('error', reject, { once: true });
  });
  let next = 0;
  const pending = new Map();
  socket.addEventListener('message', ({ data }) => {
    const message = JSON.parse(data);
    if (message.method === 'Runtime.consoleAPICalled') {
      console.error(...message.params.args.map(arg => arg.value));
    }
    if (pending.has(message.id)) {
      const { resolve, reject, timeout } = pending.get(message.id);
      pending.delete(message.id);
      clearTimeout(timeout);
      if (message.error) reject(new Error(JSON.stringify(message.error)));
      else resolve(message.result);
    }
  });
  function call(method, params = {}, sessionId) {
    return new Promise((resolve, reject) => {
      const id = ++next;
      const timeout = setTimeout(() => {
        pending.delete(id);
        reject(new Error(`Timed out: ${method}`));
      }, 600000);
      pending.set(id, { resolve, reject, timeout });
      socket.send(JSON.stringify({ id, method, params, ...(sessionId ? { sessionId } : {}) }));
    });
  }
  const { targetId } = await call('Target.createTarget', { url: 'about:blank' });
  const { sessionId } = await call('Target.attachToTarget', { targetId, flatten: true });
  await call('Runtime.enable', {}, sessionId);
  await call('Page.navigate', { url: `http://127.0.0.1:${server.address().port}/web/bench/xbrz.html` }, sessionId);
  let ready = false;
  for (let attempt = 0; attempt < 100; attempt++) {
    const result = await call('Runtime.evaluate', { expression: 'typeof window.runBenchmark === "function"', returnByValue: true }, sessionId);
    if (result.result.value) { ready = true; break; }
    await new Promise(resolve => setTimeout(resolve, 100));
  }
  if (!ready) throw new Error('Benchmark module did not load');
  const config = { samples: Number(process.env.XBRZ_SAMPLES || 6), rounds: Number(process.env.XBRZ_ROUNDS || 3), incremental: process.env.XBRZ_INCREMENTAL === "1" };
  if (!Number.isInteger(config.samples) || config.samples < 1 || !Number.isInteger(config.rounds) || config.rounds < 1) {
    throw new Error('Samples and rounds must be positive integers');
  }
  const result = await call('Runtime.evaluate', {
    expression: `window.runBenchmark(${JSON.stringify(config)})`, awaitPromise: true, returnByValue: true,
  }, sessionId);
  if (result.exceptionDetails) throw new Error(JSON.stringify(result.exceptionDetails));
  const git = args => execFileSync('git', args, { cwd: root, encoding: 'utf8' }).trim();
  const report = {
    date: new Date().toISOString(), revision: git(['rev-parse', 'HEAD']),
    localDiffSha256: hash(git(['diff', 'HEAD'])), lockfileSha256: hash(await readFile(join(root, 'Cargo.lock'))),
    shaderSha256: hash(await readFile(join(root, 'src/video/shader/xbrz_2x.glsl'))),
    vertexShaderSha256: hash(await readFile(join(root, 'src/video/shader/vertex.glsl'))),
    displayShaderSha256: hash(await readFile(join(root, 'src/video/shader/xbrz_display.glsl'))),
    rendererSha256: hash(await readFile(rendererPath)), rendererPath,
    chromeVersion: execFileSync(process.env.CHROME_BIN || 'google-chrome', ['--version'], { encoding: 'utf8' }).trim(),
    benchmarkSha256, runnerSha256,
    cpu: cpus().map(cpu => cpu.model).filter((value, i, values) => values.indexOf(value) === i),
    platform: process.platform, arch: process.arch, node: process.version, chromeArgs: args,
    ...result.result.value,
  };
  await writeFile(output, JSON.stringify(report, null, 2) + '\n');
  console.log(`Saved ${output}; renderer: ${report.renderer}; GPU timer: ${report.gpuTimer}`);
  for (const row of report.results) {
    const medians = Object.entries(row.times).map(([name, samples]) => {
      const values = samples.map(sample => report.gpuTimer ? sample.gpuMs : sample.wallMs)
        .filter(value => value !== null).sort((a, b) => a - b);
      if (!values.length) return `${name}=unavailable (all GPU samples disjoint)`;
      const mid = Math.floor(values.length / 2);
      const median = values.length % 2 ? values[mid] : (values[mid - 1] + values[mid]) / 2;
      return `${name}=${median.toFixed(3)}ms`;
    });
    console.log(`${row.width}x${row.height}->${row.outWidth}x${row.outHeight} ${row.pattern} ${row.change}: ${medians.join(' ')}`);
  }
} finally {
  socket?.close();
  browser.kill();
  await new Promise(resolve => {
    if (browser.exitCode !== null || browser.signalCode !== null) resolve();
    else browser.once('exit', resolve);
  });
  server.closeAllConnections();
  await new Promise(resolve => server.close(resolve));
  await rm(profile, { recursive: true, force: true });
}
