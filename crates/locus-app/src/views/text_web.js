// SPDX-License-Identifier: AGPL-3.0-or-later
//
// Browser font-file picker for the text tool (SPEC §9, #180), and the store of
// added fonts in the browser's private file system (OPFS, #222). Kept as plain
// JS like file_io/web.js. Nothing is uploaded: fonts stay in this browser.

// Matches locus_text::outline::MAX_OUTLINE_FONT_BYTES.
const MAX_BYTES = 64 * 1024 * 1024;
let picked = null;

// Resolves to the file name, or null (cancelled, too large or unreadable).
// The bytes are fetched with take_font().
export function pick_font() {
  return new Promise((resolve) => {
    const input = document.createElement("input");
    input.type = "file";
    input.accept = ".ttf,.otf,.ttc,.svg";
    input.onchange = async () => {
      const f = input.files && input.files[0];
      if (!f || f.size > MAX_BYTES) return resolve(null);
      try {
        picked = new Uint8Array(await f.arrayBuffer());
        resolve(f.name);
      } catch (_) {
        resolve(null);
      }
    };
    input.oncancel = () => resolve(null);
    input.click();
  });
}

export function take_font() {
  const p = picked || new Uint8Array();
  picked = null;
  return p;
}

// ── Added fonts, kept in OPFS under "fonts/" (#222) ──

const MAX_FONTS = 500;
let loaded = [];

async function fontDir() {
  if (!navigator.storage || !navigator.storage.getDirectory) throw new Error("no opfs");
  const root = await navigator.storage.getDirectory();
  return root.getDirectoryHandle("fonts", { create: true });
}

// Keep `bytes` as font `name` (already a safe file name), replacing it.
export async function font_store_save(name, bytes) {
  if (bytes.length > MAX_BYTES) return false;
  const dir = await fontDir();
  const w = await (await dir.getFileHandle(name, { create: true })).createWritable();
  await w.write(bytes);
  await w.close();
  return true;
}

export async function font_store_delete(name) {
  const dir = await fontDir();
  try {
    await dir.removeEntry(name);
  } catch (_) {}
  return true;
}

// Reads every kept font; resolves to how many. Fetch each with
// font_store_name(i) and font_store_take(i).
export async function font_store_load_all() {
  loaded = [];
  let dir;
  try {
    dir = await fontDir();
  } catch (_) {
    return 0;
  }
  for await (const [name, handle] of dir.entries()) {
    if (loaded.length >= MAX_FONTS) break;
    if (handle.kind !== "file") continue;
    try {
      const f = await handle.getFile();
      if (f.size > MAX_BYTES) continue;
      loaded.push([name, new Uint8Array(await f.arrayBuffer())]);
    } catch (_) {}
  }
  return loaded.length;
}

export function font_store_name(i) {
  return loaded[i] ? loaded[i][0] : "";
}

export function font_store_take(i) {
  const b = loaded[i] ? loaded[i][1] : new Uint8Array();
  if (loaded[i]) loaded[i][1] = null;
  return b || new Uint8Array();
}
