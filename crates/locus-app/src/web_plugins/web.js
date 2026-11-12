// SPDX-License-Identifier: AGPL-3.0-or-later
//
// Browser glue for plugins in the web build (SPEC §15, ADR 0016, #253).
// Plain JS, like file_io/web.js, so the Rust side needs few web-sys features.

// Largest .locusplugin read (matches locus_plugin_host::archive::MAX_ARCHIVE_BYTES).
const MAX_ARCHIVE_BYTES = 80 * 1024 * 1024;
const DIR = "plugins";
const EXT = ".locusplugin";
// Matches locus_plugin_host::core_host::wire::WORKER_READY.
const READY = "locus-plugin-worker:ready";

// ── Running a call ──────────────────────────────────────────────────────────
// Every call gets a fresh dedicated worker, which is terminated when it
// answers or when the timeout passes (SPEC §15.4). The answer is the
// worker's JSON string; a timeout or a crashed worker gives a JSON error.

// A failed call, in the worker's reply format (wire::WireError).
function error(kind, message) {
  return JSON.stringify({ Err: { kind, message } });
}

export function plugin_call(component, request, timeoutMs) {
  return new Promise((resolve) => {
    let worker;
    try {
      worker = new Worker("locus-plugin-worker_loader.js");
    } catch (_) {
      resolve(error("Worker", "The plugin worker couldn't start."));
      return;
    }
    let done = false;
    const finish = (json) => {
      if (done) return;
      done = true;
      clearTimeout(timer);
      worker.terminate();
      resolve(json);
    };
    const timer = setTimeout(
      () => finish(error("Timeout", `the plugin took longer than ${timeoutMs / 1000} s`)),
      timeoutMs,
    );
    let sent = false;
    worker.onmessage = (e) => {
      // The worker says when it can take the call (wire::WORKER_READY).
      if (!sent && e.data === READY) {
        sent = true;
        worker.postMessage({ component, request });
        return;
      }
      finish(typeof e.data === "string" ? e.data : error("Worker", "the plugin worker sent garbage"));
    };
    worker.onerror = () => finish(error("Worker", "the plugin worker crashed"));
  });
}

// ── Install: a binary file picker ───────────────────────────────────────────
// Resolves to a Uint8Array, a string (an error), or null (cancelled).

export function pick_bytes(accept) {
  return new Promise((resolve) => {
    const input = document.createElement("input");
    input.type = "file";
    input.accept = accept;
    input.onchange = async () => {
      const f = input.files && input.files[0];
      if (!f) return resolve(null);
      if (f.size > MAX_ARCHIVE_BYTES) return resolve("The file is too large to be a plugin.");
      try {
        resolve(new Uint8Array(await f.arrayBuffer()));
      } catch (_) {
        resolve("Could not read the file.");
      }
    };
    input.oncancel = () => resolve(null);
    input.click();
  });
}

// ── Installed plugins in OPFS: plugins/<id>.locusplugin ────────────────────
// The archive is kept as it was installed and re-checked when loaded.

async function dir(create) {
  if (!navigator.storage || !navigator.storage.getDirectory) throw new Error("no opfs");
  const root = await navigator.storage.getDirectory();
  return root.getDirectoryHandle(DIR, { create });
}

// JSON array of installed ids.
export async function plugin_ids() {
  const ids = [];
  try {
    const d = await dir(false);
    for await (const name of d.keys()) {
      if (name.endsWith(EXT)) ids.push(name.slice(0, -EXT.length));
    }
  } catch (_) {
    // No folder yet: nothing installed.
  }
  return JSON.stringify(ids);
}

export async function plugin_read(id) {
  const d = await dir(false);
  const f = await (await d.getFileHandle(id + EXT)).getFile();
  if (f.size > MAX_ARCHIVE_BYTES) throw new Error("too large");
  return new Uint8Array(await f.arrayBuffer());
}

// The writable stream swaps in atomically on close(): a failed install never
// leaves half a plugin behind or damages the installed one.
export async function plugin_write(id, bytes) {
  const d = await dir(true);
  const w = await (await d.getFileHandle(id + EXT, { create: true })).createWritable();
  await w.write(bytes);
  await w.close();
}

export async function plugin_remove(id) {
  const d = await dir(false);
  await d.removeEntry(id + EXT);
}
