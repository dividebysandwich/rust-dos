// Keeping C: in the browser: its image in IndexedDB, as the pieces the
// emulator counts writes in (Machine.chunk_size()), so saving a change
// writes only the pieces it touched. Pieces of zeros aren't stored.

const DATABASE = 'rust-dos';
const STORE = 'c-drive';
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
    const request = indexedDB.open(DATABASE, 1);
    request.onupgradeneeded = () => request.result.createObjectStore(STORE);
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
}
