// SPDX-License-Identifier: AGPL-3.0-or-later
//
// Browser font-file picker for the text tool (SPEC §9, #180). Kept as plain JS
// like file_io/web.js. Fonts are read in memory only; nothing is uploaded.

// Matches locus_text::outline::MAX_OUTLINE_FONT_BYTES.
const MAX_BYTES = 64 * 1024 * 1024;
let picked = null;

// Resolves to the file name, or null (cancelled, too large or unreadable).
// The bytes are fetched with take_font().
export function pick_font() {
  return new Promise((resolve) => {
    const input = document.createElement("input");
    input.type = "file";
    input.accept = ".ttf,.otf,.svg";
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
