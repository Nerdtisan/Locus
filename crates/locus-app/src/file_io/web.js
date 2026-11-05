// SPDX-License-Identifier: AGPL-3.0-or-later
//
// Browser glue for file_io (SPEC §0 Web hosting, M1 #75). Kept as plain JS so
// the Rust side doesn't need a large web-sys feature set for OPFS / IndexedDB.

const AUTOSAVE_NAME = "autosave.locus";
const DB_NAME = "locus";
const STORE = "kv";
// Matches locus_doc::file::MAX_FILE_BYTES.
const MAX_BYTES = 256 * 1024 * 1024;

// Save: hand the browser a file to download.
export function download(name, data, mime) {
  const url = URL.createObjectURL(new Blob([data], { type: mime }));
  const a = document.createElement("a");
  a.href = url;
  a.download = name;
  document.body.appendChild(a);
  a.click();
  a.remove();
  setTimeout(() => URL.revokeObjectURL(url), 10000);
}

// Open: file picker. Resolves to JSON {name, text}, {error}, or null (cancelled).
export function pick_text(accept) {
  return new Promise((resolve) => {
    const input = document.createElement("input");
    input.type = "file";
    input.accept = accept;
    input.onchange = async () => {
      const f = input.files && input.files[0];
      if (!f) return resolve(null);
      if (f.size > MAX_BYTES) return resolve(JSON.stringify({ error: "The file is too large to open." }));
      try {
        resolve(JSON.stringify({ name: f.name, text: await f.text() }));
      } catch (_) {
        resolve(JSON.stringify({ error: "Could not read the file." }));
      }
    };
    input.oncancel = () => resolve(null);
    input.click();
  });
}

// ── Autosave: OPFS first, IndexedDB fallback (Safari has no OPFS writable on
// the main thread). Each copy carries a timestamp so the newest wins on read.
// Neither write can lose the previous copy: an OPFS writable stream swaps in
// atomically on close(), and an IndexedDB put is one transaction.

async function opfsFile(create) {
  if (!navigator.storage || !navigator.storage.getDirectory) throw new Error("no opfs");
  const dir = await navigator.storage.getDirectory();
  return dir.getFileHandle(AUTOSAVE_NAME, { create });
}

function openDb() {
  return new Promise((resolve, reject) => {
    const req = indexedDB.open(DB_NAME, 1);
    req.onupgradeneeded = () => req.result.createObjectStore(STORE);
    req.onsuccess = () => resolve(req.result);
    req.onerror = () => reject(req.error);
  });
}

export async function autosave_write(text) {
  const payload = JSON.stringify({ t: Date.now(), text });
  try {
    const w = await (await opfsFile(true)).createWritable();
    await w.write(payload);
    await w.close();
    return;
  } catch (_) {
    // fall through to IndexedDB
  }
  const db = await openDb();
  try {
    await new Promise((resolve, reject) => {
      const tx = db.transaction(STORE, "readwrite");
      tx.objectStore(STORE).put(payload, AUTOSAVE_NAME);
      tx.oncomplete = resolve;
      tx.onerror = tx.onabort = () => reject(tx.error);
    });
  } finally {
    db.close();
  }
}

// Newest autosaved document text, or null.
export async function autosave_read() {
  const copies = [];
  try {
    const f = await (await opfsFile(false)).getFile();
    copies.push(JSON.parse(await f.text()));
  } catch (_) {}
  try {
    const db = await openDb();
    try {
      const v = await new Promise((resolve, reject) => {
        const r = db.transaction(STORE, "readonly").objectStore(STORE).get(AUTOSAVE_NAME);
        r.onsuccess = () => resolve(r.result);
        r.onerror = () => reject(r.error);
      });
      if (v) copies.push(JSON.parse(v));
    } finally {
      db.close();
    }
  } catch (_) {}
  copies.sort((a, b) => b.t - a.t);
  return copies.length ? copies[0].text : null;
}
