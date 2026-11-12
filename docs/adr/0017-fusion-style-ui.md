# ADR-0017: Fusion-style UI: ribbon, marking menu, one tool model, action registry

- **Status:** Proposed (accepted when the owner signs off the M5 design)
- **Date:** 2026-11-13
- **Spec:** `docs/SPEC.md §6`, `§11`, `§12.1` (closes §12 item 15), `§4`, `§7`; builds on ADR 0001
- **Issue:** Nerdtisan/Locus#293 (epic #283); refactor #294
- **Design:** [`docs/ux/m5-design.md`](../ux/m5-design.md), clickable mockup [`docs/ux/m5-mockup.html`](../ux/m5-mockup.html)

## Context

ADR 0001 chose eframe + egui_dock and an egui-painter canvas. It settled the framework but not the interaction design. M0–M4 added tools one ticket at a time. In M4 testing the owner found the app hard to use and not CAD-like enough (#283): right-drag pans, so there is no context menu; Delete doesn't delete; tools behave differently from each other; and style sits on layers.

The code shows the same pattern:

- `crates/locus-app/src/canvas.rs` is 2,402 lines. `Canvas` owns the document, view, selection, every tool's state, typed input, touch input, and the UI of six dock tabs.
- The tool, modify and constraint lists (`Tool::DRAWING`, `Modify::ALL`, `Kind::ALL`) are enumerated in 9 places across `app.rs`, `canvas.rs` and `canvas/layers_panel.rs`. Shortcuts live on three enums plus a literal.
- Drawing (`Draft`), modify (`Modify` + `corner_first` / `offset_pick`), constraint, text and pen input are each handled in their own way.

M5 needs one consistent UI, and a structure that several feature sessions can build on in parallel.

## Options

### A. Polish the current UI

Fix the reported bugs, add a context menu and keep the flat tool strip and tab set.

- Least work up front.
- Leaves the duplicated lists and the different tool models in place, and every M5 ticket would edit `canvas.rs`. The "not CAD-like" finding stays.

### B. A SolidCraft-style UI

SolidCraft (§18) is the closest Rust reference.

- Its code is permissively licensed and the closest technically.
- The owner decided that Fusion wins where the two differ (2026-11-13). Fusion is the benchmark users know, and SolidCraft is itself a Fusion reimplementation.

### C. Fusion-style UI on an action registry and a tool trait (chosen)

- A ribbon grouped Create / Modify / Constrain / Inspect / Insert / Select, with an OUTPUT tab.
- A browser tree, a sketch palette, a context-sensitive inspector, command dialogs and a status bar.
- A right-click marking menu: a fixed ring of 8 and a context list. Pan moves to middle-drag and Space+drag.
- Fusion's sketch keys.
- One four-step tool model: pick → drag or type with a live preview → Enter / Esc → values stay as dimensions.
- Per-object style. Re-editable text.
- Underneath: one action registry feeding every menu, key and search, a `Tool` trait for every tool family, `Canvas` split into state plus views, and the saved layout migrated from v1 to v2.

## Measurements

This is a design decision, not a performance spike, so the measurements are of the current code (`origin/dev` at `495fd86`):

| Measure | Today | After #294 |
|---|---|---|
| `canvas.rs` lines | 2,402 | canvas view only; state in `state/`, panes in `views/` |
| Places enumerating the tool / modify / constraint lists | 9 (3 files) | 1 (the registry) |
| Places defining shortcuts | 3 enums + 1 literal | 1 (`KeyMap` from the registry) |
| Tool input models | 5 (draw, pen, modify, constrain, text) | 1 (`Tool` + `ToolHost`) |
| Shortcut letters that clash with Fusion's sketch keys | 6 (S, R, T, O, M, X) | 0 |

No runtime performance changes are expected: the registry and trait are dispatch over a few dozen entries per frame. The drag re-solve budget (§3, < 16 ms for 500 entities) is unaffected; #294 re-checks it.

## Decision

**Option C.** Locus adopts the Fusion-style layout, marking menu, shortcut map and single tool model described in `docs/ux/m5-design.md`. The M5 foundation refactor (#294) introduces the `Action` registry, the `Tool` trait and `ToolHost`, the `AppState` / `views/` split, and layout persistence v2. It changes no behaviour.

## Consequences

- **Spec.** §4 entities gain a style (stroke colour, width, pen number, fill, opacity), and §7 Draw layers lose pen number and pen width. These are recorded in §0 and noted in §7; the file-format change and migration land with #287. §12 gains item 15 pointing here.
- **Keys change** for existing users: S, R, T, O, M, X, I and E now do different things, and V and B are freed (design doc §5). The shortcut map (?) shows the changes, and the registry lets users rebind keys.
- **Right-drag no longer pans.** Middle-drag and Space+drag pan instead. Touch is unchanged.
- **The saved layout** moves to `locus.layout.v2`, migrated from v1. v1 is kept so older builds still open.
- **Parallel work.** After #294, feature tickets add a view or a tool plus registry entries, so sessions don't collide in one file.
- **Core crates are untouched** by the UI work; it all lives in `locus-app`. The command API (§11) stays the only way to mutate the document, and views are kept to it by structure.
- **Revisit** if the owner's hands-on check of the M5 build finds the marking menu or the key map worse than today's, or if egui_dock's multi-viewport support (#285) can't restore windows reliably across platforms.
