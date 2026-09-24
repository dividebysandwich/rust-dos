// Reading .zip archives, as DOS games are passed around in: the files in
// them, stored or deflated (the browser inflates them), with the DOS time
// and date the archive keeps for each.

const END_OF_DIRECTORY = 0x06054b50;
const DIRECTORY_ENTRY = 0x02014b50;
const LOCAL_HEADER = 0x04034b50;

/// The files of the archive `bytes`: {path, data, time, date}, paths with
/// forward slashes. Directories are left out; their files carry them.
export async function unzip(bytes) {
  const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  const end = findEndOfDirectory(view);
  if (end < 0) {
    throw new Error('not a zip archive');
  }
  const count = view.getUint16(end + 10, true);
  let at = view.getUint32(end + 16, true);
  if (count === 0xffff || at === 0xffffffff) {
    throw new Error('ZIP64 archives are not supported');
  }
  const files = [];
  for (let n = 0; n < count; n++) {
    if (at + 46 > bytes.length || view.getUint32(at, true) !== DIRECTORY_ENTRY) {
      throw new Error('the archive is damaged');
    }
    const flags = view.getUint16(at + 8, true);
    const method = view.getUint16(at + 10, true);
    const time = view.getUint16(at + 12, true);
    const date = view.getUint16(at + 14, true);
    const compressed = view.getUint32(at + 20, true);
    const size = view.getUint32(at + 24, true);
    const nameLength = view.getUint16(at + 28, true);
    const extraLength = view.getUint16(at + 30, true);
    const commentLength = view.getUint16(at + 32, true);
    const local = view.getUint32(at + 42, true);
    const name = decodeName(bytes.subarray(at + 46, at + 46 + nameLength), flags);
    at += 46 + nameLength + extraLength + commentLength;

    const path = name.replace(/\\/g, '/');
    if (path.endsWith('/')) {
      continue;
    }
    if (flags & 1) {
      throw new Error(`${path} is encrypted`);
    }
    if (view.getUint32(local, true) !== LOCAL_HEADER) {
      throw new Error('the archive is damaged');
    }
    const start = local + 30 + view.getUint16(local + 26, true) + view.getUint16(local + 28, true);
    const stored = bytes.subarray(start, start + compressed);
    let data;
    if (method === 0) {
      data = stored;
    } else if (method === 8) {
      data = await inflate(stored);
    } else {
      throw new Error(`${path} is packed with a method this page can't unpack (${method})`);
    }
    if (data.length !== size) {
      throw new Error(`${path} is damaged`);
    }
    files.push({ path, data, time, date });
  }
  return files;
}

function findEndOfDirectory(view) {
  // The record is 22 bytes, followed by a comment of up to 64 KB.
  const last = view.byteLength - 22;
  for (let at = last; at >= 0 && at >= last - 0xffff; at--) {
    if (view.getUint32(at, true) === END_OF_DIRECTORY) {
      return at;
    }
  }
  return -1;
}

/// A file name: UTF-8 if the archive says so, else code page 437, of which
/// only the ASCII part matters for DOS names.
function decodeName(raw, flags) {
  if (flags & 0x800) {
    return new TextDecoder().decode(raw);
  }
  return Array.from(raw, (b) => (b < 0x80 ? String.fromCharCode(b) : '_')).join('');
}

async function inflate(raw) {
  const stream = new Blob([raw]).stream().pipeThrough(new DecompressionStream('deflate-raw'));
  return new Uint8Array(await new Response(stream).arrayBuffer());
}
