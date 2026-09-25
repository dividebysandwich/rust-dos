// Keeping C: in the browser: its image in IndexedDB, as the pieces the
// emulator counts writes in (Machine.chunk_size()), so saving a change
// writes only the pieces it touched. Pieces of zeros aren't stored. The
// save states' slots are kept there too, by `<game or dos>/<slot>`.

const DATABASE = 'rust-dos';
const STORE = 'c-drive';
const STATES = 'states';
const META = 'meta';

function done(request) {
  return new Promise((resolve, reject) => {
    request.onsuccess = () => resolve(request.result);
    request.onerror = () => reject(request.error);
  });
}

function finished(transaction) {
  return new Promise((resolve, reject) => {
    transaction.oncomplete = () => resolve();
    transaction.onerror = () => reject(transaction.error);
    transaction.onabort = () => reject(transaction.error ?? new Error('storage transaction aborted'));
  });
}

export class DiskStore {
  constructor(db) {
    this.db = db;
  }

  /// The browser's store for C:, or null if it has none (private
  /// windows of some browsers, blocked site data).
  static async open() {
    if (!('indexedDB' in globalThis)) {
      return null;
    }
    // Version 1 kept C: only; version 2 adds the save states.
    const request = indexedDB.open(DATABASE, 2);
    request.onupgradeneeded = () => {
      const db = request.result;
      for (const name of [STORE, STATES]) {
        if (!db.objectStoreNames.contains(name)) {
          db.createObjectStore(name);
        }
      }
    };
    return new DiskStore(await done(request));
  }

  /// Put the stored C: into `machine` as the image `begin_image` starts,
  /// ready for `mount_image`. False if there is none stored with pieces of
  /// `chunkSize` bytes.
  async restore(machine, chunkSize) {
    const transaction = this.db.transaction(STORE, 'readonly');
    const store = transaction.objectStore(STORE);
    const meta = await done(store.get(META));
    if (!meta || meta.chunk !== chunkSize) {
      return false;
    }
    machine.begin_image(meta.size);
    // Numbers sort before strings, so this is every piece and not the
    // meta record.
    await new Promise((resolve, reject) => {
      const cursor = store.openCursor(IDBKeyRange.upperBound(Number.MAX_SAFE_INTEGER));
      cursor.onsuccess = () => {
        const at = cursor.result;
        if (!at) {
          resolve();
          return;
        }
        try {
          machine.write_image(at.key * chunkSize, at.value);
        } catch (error) {
          reject(error);
          return;
        }
        at.continue();
      };
      cursor.onerror = () => reject(cursor.error);
    });
    return true;
  }

  /// Start over with an empty C: of `size` bytes.
  async reset(size, chunkSize) {
    const transaction = this.db.transaction(STORE, 'readwrite');
    const store = transaction.objectStore(STORE);
    store.clear();
    store.put({ size, chunk: chunkSize }, META);
    await finished(transaction);
  }

  /// Forget the stored C:, so the next start makes a new one.
  async clear() {
    const transaction = this.db.transaction(STORE, 'readwrite');
    transaction.objectStore(STORE).clear();
    await finished(transaction);
  }

  /// Store pieces of C:: [number, bytes], with null bytes for a piece of
  /// zeros.
  async save(pieces) {
    const transaction = this.db.transaction(STORE, 'readwrite');
    const store = transaction.objectStore(STORE);
    for (const [index, bytes] of pieces) {
      if (bytes) {
        store.put(bytes, index);
      } else {
        store.delete(index);
      }
    }
    await finished(transaction);
  }

  /// The save states kept: [key, bytes] each.
  async states() {
    const transaction = this.db.transaction(STATES, 'readonly');
    const store = transaction.objectStore(STATES);
    const [keys, values] = await Promise.all([done(store.getAllKeys()), done(store.getAll())]);
    return keys.map((key, i) => [key, values[i]]);
  }

  /// Keep save states: [key, bytes], with null bytes for a slot emptied.
  async saveStates(changes) {
    const transaction = this.db.transaction(STATES, 'readwrite');
    const store = transaction.objectStore(STATES);
    for (const [key, bytes] of changes) {
      if (bytes) {
        store.put(bytes, key);
      } else {
        store.delete(key);
      }
    }
    await finished(transaction);
  }
}
