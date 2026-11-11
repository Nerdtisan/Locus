# ADR-0012: Text entity — container with cached geometry, fonts by reference

- **Status:** Accepted
- **Date:** 2026-11-11
- **Spec:** `docs/SPEC.md §9, §10`
- **Issue:** Nerdtisan/Locus#179 (epic #22)

## Context

Text must be a live, constrainable entity that exports correctly on a machine
without the font that made it. Font licences often forbid embedding font
files in documents, and system fonts cannot be listed on the web. ADR 0009
already put text in core, not a plugin.

## Options

### A — Embed the font file in the `.locus` file
Always reproducible; rejected: licence risk and file size.

### B — Store only content + font name; lay out on every load
Small files; rejected: the file would open and export differently (or not at
all) without the font, and layout would run on load.

### C — Store content, font *reference* and the laid-out geometry (chosen)
The file opens and exports identically anywhere; editing needs the font.

## Decision

`Geom::Text { anchor, dir, text }` (format v9):

- `anchor` and `dir` are ordinary point entities. The baseline origin is the
  anchor; the baseline direction is `anchor → dir` (+x if they coincide). They
  constrain like any point (coincident, fix, distance); drawing a construction
  line between them lets horizontal/vertical/angle constrain the baseline.
  Text is not a curve, so it has no constraints of its own.
- `TextData` holds content, a `FontRef` (`Bundled(id)` / `File(name)` /
  `System(family)`), size (em, mm), alignment, line spacing, and `layout`: the
  geometry in the text's local frame (closed cubic/line contours for outline
  fonts, open strokes for single-line fonts). Moving or rotating the points
  never re-lays out.
- Layout happens in the caller (app, CLI, MCP) through a `FontProvider`
  (`locus-text::FontLibrary`), and the command carries the result, so commands
  stay plain serialisable data and `locus-doc` does no font I/O.
- "Font missing" is computed (`TextData::font_missing`), not stored: a document
  cannot know what is installed. Export uses the cache and never needs the
  font; editing a text whose font is gone fails with `FontUnavailable` until
  another font is chosen.
- Export converts each cached contour to an export path directly (not through
  `chain`, which could fuse touching glyphs). Text is excluded from region
  detection, offsets and trimming; kerf and other export-time operations act on
  its exported outline.
- Bounds on untrusted files: 10 000 characters, 500 lines, 50 000 contours,
  400 000 segments per text entity; finite numbers only.
- System fonts are discovered by the app shell behind `SystemFontSource`;
  `locus-text` stays free of filesystem access and wasm32-clean.

## Consequences

- Format v9 (migration v8 → v9 is the identity; a v8 file holds no text).
- Outline text makes files larger (a 100-character outline string is a few
  hundred KB of RON); acceptable for M3, revisit with a binary or delta-coded
  cache if it hurts.
- Mirror/pattern copies of text re-point the anchor and direction but keep the
  text readable (not mirrored).
- Text cannot join region booleans until a later milestone decides how glyph
  outlines should behave there.
