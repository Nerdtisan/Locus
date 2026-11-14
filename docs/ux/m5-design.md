# M5 design: Fusion-style sketch UI

- **Issue:** Nerdtisan/Locus#293 (epic #283). The foundation refactor that follows is #294.
- **Decision record:** [ADR 0017](../adr/0017-fusion-style-ui.md)
- **Mockup:** the clickable prototype is [`m5-mockup.html`](m5-mockup.html), which is also published as a private Artifact for the owner. It is a static design, not an editor.
- **Spec:** §4, §6, §7, §8, §9, §11, §12.1, ADR 0001
- **Status:** **approved** by the owner (revision 3, 2026-11-13). Revision 1 was reviewed as "much better". Revision 2 added the owner's five requests (§2.3, §5.1, §6.5, §6.6, §6.8). Revision 3 added plugins (§6.13), the View menu and navigation bar (§2.4), and constraints as symbols with no letter keys (owner decision, §2, §5). The §9 defaults stand.

Owner testing in M4 found Locus hard to use and not CAD-like enough. This document fixes how M5 should look and behave before anything is built. Where Fusion 360 and SolidCraft differ, it follows Fusion (owner decision, 2026-11-13). Fusion is a behaviour reference only (§18); no assets or code are copied.

## 1. Owner decisions this design builds on (2026-11-13)

1. Right-click opens a Fusion-style **marking menu**: a ring of 8 actions with a list beneath it. Pan moves to middle-drag and Space+drag.
2. **Style belongs to each object**, not to its layer: stroke colour, stroke width, pen number, fill and opacity (#287). Layers keep only their operation settings.
3. Placed **text stays re-editable** (#222).
4. Fusion wins wherever Fusion and SolidCraft differ.
5. **Constraints use symbols, not letters**, as in Fusion: no single-letter constraint keys (owner decision on revision 2).

**Spec conflict, flagged (§17):** §4 lists the fields each entity carries, and §7 puts pen number and pen width on the Draw layer. Decision 2 changes both. This PR records the decision in §0 and §12, and adds a note to §7. The data-model change (format version, migration of Draw-layer pens onto their objects) lands with #287.

## 2. Layout

```
┌ App bar: File Edit View Plugins Help · Save · Undo · Redo ··· document · profile ··· Search (S) · ? ┐
├ [SKETCH] [OUTPUT] [UTILITIES] ───────────────────────────────────────────────────────────────────┤
│ CREATE ▾ │ MODIFY ▾ │ CONSTRAIN ▾ │ INSPECT ▾ │ INSERT ▾ │ SELECT ▾ │                      EXPORT │
├──────────────┬──────────────────────────────────────────────────────────┬────────────────────────┤
│ BROWSER    ⧉ │                                          ┌ command ─────┐│ SKETCH PALETTE       ⧉ │
│ ▾ Layers     │                                          │ dialog       ││ Linetype Normal|Constr │
│ ▾ Parameters │               canvas                     │ (tool fields)││ grid · snap · auto · … │
│ ▾ Recipes    │                                          └──────────────┘│ tool options           │
│ ▸ Regions    │                                                          ├────────────────────────┤
│ ▸ Constraints│  [pan ± fit sel | grid&snaps▾ rulers | display▾ stroke prev] │ INSPECTOR          ⧉ │
│              │                                                          │ (changes with context) │
├──────────────┴──────────────────────────────────────────────────────────┴────────────────────────┤
│ prompt for the next click ·········· Under-constrained · 3 DOF · 1 selected · x y · snap · mm    │
└──────────────────────────────────────────────────────────────────────────────────────────────────┘
```

| Area | Contents | Replaces today |
|---|---|---|
| **Ribbon, SKETCH tab** | Groups Create / Modify / Constrain / Inspect / Insert / Select. Each shows its 2–5 most used tools as icons; the group label (▾) opens the full list with shortcuts. EXPORT sits at the right end, as Fusion's FINISH SKETCH does. | The flat tool strip (`app.rs` `tool_bar`) and the Sketch / Constrain menus |
| **Ribbon, OUTPUT tab** | Machine preview, Validate, Export…, Machine profiles…, Exporter plugins. | View ▸ Machine preview, File ▸ Export |
| **Ribbon, UTILITIES tab** | PLUGINS (Install plugin…, Manage plugins…) and AUTOMATION (MCP server…, Keyboard shortcuts), as Fusion keeps add-ins under Utilities. | Plugins menu items, View ▸ MCP |
| **App menus** | File, Edit, View (§2.4), Plugins (§6.13), Help: the full command set, as today's menu bar, and the place for rarely used commands. | Today's menu bar, reorganised |
| **Browser** (left) | One tree: Layers (visibility, lock, operation), Parameters, Recipes (patterns, mirrors, offsets, text, plugin objects), Regions, Constraints. Selecting a row selects its geometry. Right-click opens the marking menu with that row's items. | The Layers, Parameters, Patterns and Regions tabs |
| **Sketch palette** (right, top) | Linetype (Normal / Construction), sketch grid, snap, auto-constrain, show profiles / points / dimensions / constraints, stroke width. The active tool's options appear here (polygon sides, fillet radius…). | Scattered toggles in View and Constrain menus and the tool strip |
| **Inspector** (right, bottom) | Context-sensitive; see §2.1. | The Properties tab |
| **Command dialog** | Docked to the canvas's top-right corner while a dialog tool runs (Offset, Move, Pattern, Mirror, Text, Fillet…): selection boxes, fields, OK / Cancel. | Tool-strip widgets in `corner_sizes` and the Patterns tab |
| **Navigation bar** | Bottom centre of the canvas, as Fusion's: pan, zoom, fit, zoom to selection, Grid and snaps ▾, rulers, Display ▾, stroke width, machine preview (§2.4). | View menu toggles |
| **Status bar** | The next click's prompt, sketch DOF state (§5 Solver UX), selection count, cursor position, snap, units. | Status bar (kept, reorganised) |

The ribbon groups, in Fusion's order and naming:

| Group | Tools (key) |
|---|---|
| Create | Line (L); 2-Point / Center / 3-Point Rectangle (R); Center Diameter / 2-Point / 3-Point Circle (C); 3-Point (A) / Center / Tangent Arc; Polygon (G); Slot (U); Bézier Pen (N); Point (P); Text; Mirror; Circular Pattern; Rectangular Pattern; Offset (O) |
| Modify | Fillet; Chamfer; Trim (T); Extend; Break; Move/Copy (M); Rotate; Sketch Scale; Normal/Construction (X); Explode recipe; Style…; Delete (Del); then **modifier plugins** (e.g. Dogbone corners, Tabs and bridges) |
| Constrain | Sketch Dimension (D); then **symbol buttons, no keys**: Coincident, Collinear, Concentric, Midpoint, Fix/Unfix, Parallel, Perpendicular, Horizontal/Vertical, Tangent, Smooth (G2)\*, Equal, Symmetric; Auto-constrain toggle |
| Inspect | Measure (I); Machine preview; Show stroke width; Validate for export |
| Insert | Insert SVG…; Insert DXF…; then **generator plugins** (e.g. Finger-joint box, Living hinge, Spur gear) |
| Select | Select (Esc); Window select; Select chain; Select layer; Select all (Ctrl+A); Selection filters |

As in Fusion, Mirror, the patterns and Offset sit in Create, because they create geometry.

**Constraint symbols.** The CONSTRAIN group shows every constraint as a symbol button by default (they are small, so all twelve fit, as in Fusion). Hovering one shows its name. The same symbols mark constraints on the canvas; hovering one names it, and right-clicking it offers Delete. Constraints have no letter keys (owner decision): they are reached from the symbols, tool search (S), the marking-menu list (Constrain ▸), and pinning. A user can still bind a key to any constraint in Settings ▸ Keyboard; none are bound by default.

\* **Smooth (G2 curvature continuity)** is in Fusion's set but **not in spec §5** or the solver. It is flagged (§17): the mockup shows its symbol for completeness, but the app hides it until a spec change and a solver ticket add it.

### 2.4 View menu and navigation bar (revision 3)

Today's View menu, laid out as Fusion lays it out: the everyday toggles on a **navigation bar** at the bottom of the canvas, and everything in a **View ▾** app menu. Both, and the sketch palette's display checkboxes, are views of the same settings, so they never disagree.

| Navigation bar | |
|---|---|
| Pan · Zoom in · Zoom out · Fit (F6, double middle-click) · Zoom to selection | Wheel zooms about the cursor; Ctrl+= / Ctrl+- zoom about the centre |
| **Grid and snaps ▾** | Show grid, snap to grid, snap to geometry, rulers, grid spacing and minor divisions |
| Rulers | Top and left rulers in display units |
| **Display ▾** | Construction geometry, dimensions, constraint symbols, points, profiles, stroke width, machine preview |
| Stroke width · Machine preview | One-click toggles for the two most used display modes |

**View ▾** holds: Fit to content, Zoom in / out / to selection; Grid and snaps; Display; **Panes** (show or hide each pane, Shortcuts with ?) and Reset layout; **Theme** (light / dark / system); **Units** (mm / cm / in; stored as mm, §4); and **MCP server…** (desktop only, §16).

View settings are per user and stored in app settings, not in documents, except the document's display units.

### 2.1 Inspector contents

| Context | Sections |
|---|---|
| Nothing selected | Document: sketch status, profile, active layer, units. **Current style**: the style new geometry gets. |
| Curves | Header with type, count and DOF badge; Layer; Construction checkbox (X); Geometry (length, angle, radius; typing a value adds a driving dimension, the same as D); **Style**; Constraints touching the selection. |
| Text | Content, font, height, layer; Style. "Double-click to edit" hint; Explode. |
| Recipe instance (pattern copy, mirror copy, offset) | "Driven" badge, the recipe it came from, Edit recipe / Suppress / Explode. |
| Layer (in the browser) | Name, operation, export on/off, order, and the operation's settings (§7). Draw layers have no pen settings any more. |
| Mixed selection | Only the fields they share; differing values show "—"; typing sets them all in one command. |

### 2.2 Undockable panes (#285)

Every pane header has ⧉. On the desktop, dragging a tab out of the window (or ⧉) opens it in its own OS window through eframe's multi-viewport support, so it can sit on a second monitor; dragging it back re-docks it. On the web the pane floats inside the browser window. Window placement is saved with the layout (§7.4).

### 2.3 Customisable toolbar (owner request, revision 2)

Most-used tools should take one click, not two (open the group, pick the tool).

- **Pin**: drag any tool from a group's ▾ menu up onto the toolbar and drop it where it should go, or click the pin icon beside the tool in the menu.
- **Reorder**: drag a toolbar button sideways, within its group or into another group.
- **Remove**: drag a button off the toolbar, or right-click it ▸ Unpin. The tool stays in its group's menu, which always lists every tool.
- **Reset**: each menu ends with "Reset ‹group› toolbar".
- Pins are per user, saved in app settings as `locus.toolbar.v1` (an ordered list of action ids per group), never in documents. Unknown ids (a removed plugin, a renamed action) are dropped when loading. A corrupt value falls back to the defaults.
- Pinned actions are listed first in tool search before anything is typed.
- Keyboard and touch: the pin icon is a normal button, so pinning doesn't need dragging.

## 3. Interaction model

Every tool, whether it draws, modifies, constrains, places text or uses the pen, follows the same four steps:

1. **Pick the tool**: from the ribbon, a key, the marking menu or tool search (S). The status bar says what to click.
2. **Drag or type, with a live preview.** Heads-up fields next to the cursor show the values that matter (length and angle for a line, radius for a circle, distance for an offset). Typing goes into the focused field without clicking it, and Tab moves to the next. A typed value locks that degree of freedom while the mouse keeps moving, as in Fusion. Auto-constraint glyphs show what will be inferred; holding Alt suppresses them (§6.2).
3. **Enter commits; Esc cancels.** A commit is one command and one undo step (§11). For a polyline, Enter or Esc ends it and keeps the committed segments; a second Esc returns to Select. Dialog tools take Enter as OK.
4. **Values stay editable.** Typed values become driving dimensions; recipe parameters (offset distance, pattern count and spacing) show as dimensions on the canvas. Double-clicking one opens the inline editor, which takes a number or an expression (§5).

**Selection-first or tool-first.** Modify and constrain tools accept either order, as Fusion does. If the current selection already fits the tool, the tool fills its selection boxes from it. With Constrain, it applies at once. Otherwise the tool asks for picks.

### 3.1 Mouse

| Input | Action |
|---|---|
| Left click | Select one curve (Shift adds, Ctrl toggles); in a tool, the tool's next pick |
| Left drag on empty canvas | Window select (left-to-right: fully inside; right-to-left: crossing, as Fusion) |
| Left drag on a point or curve | Drag it; the sketch re-solves live (§3, < 16 ms) |
| **Double-click a curve** | **Select its chain** (#220): every curve joined end to end, stopping at branch points (three or more curves meeting). Shift / Ctrl + double-click add or toggle a chain. Double-click on text or a dimension edits it. |
| **Right-click** | **Marking menu** (§4). If the shape under the cursor isn't selected, it is selected first. |
| **Middle-drag, or Space + left-drag** | **Pan** (moved from right-drag) |
| Wheel / double middle-click | Zoom about the cursor / fit |
| Touch | As M4 (#250, #251); long-press opens the marking menu |

## 4. Marking menu (#290)

Right-click opens a ring of 8 actions around the cursor, with a list underneath.

| Slot | Action | Slot | Action |
|---|---|---|---|
| N | Repeat last command | S | Delete |
| NE | Undo | SW | Normal/Construction |
| E | Move/Copy | W | Trim |
| SE | Offset | NW | Sketch Dimension |

- **The ring never changes.** Its slots stay in the same places in every context, so a flick toward a slot becomes muscle memory. Actions that can't run are dimmed, not moved.
- **Flick:** press the right button, drag toward a slot and release, and it runs without the menu drawing. A plain right-click shows the menu until a choice or Esc.
- **The list depends on context.** With a selection: Constrain ▸ (constraints that fit the selection), Select chain, Copy, Paste, Duplicate, Style…, Edit text (on text), Properties. On empty canvas: Paste, Select all, Fit to content. On a browser layer row: Rename, Set active, Hide/Show, Lock, Select layer's objects, Delete layer.
- The ring and the list are both built from the action registry (§7.1).

## 5. Shortcut map

These follow Fusion's sketch keys. **Constraints have no letter keys**, as in Fusion (owner decision): use the CONSTRAIN symbols, tool search, the marking menu or a pinned button.

| Key | Action | Today |
|---|---|---|
| **S** | Tool search | Select |
| **L** | Line | Line |
| **R** | 2-Point Rectangle | Center Arc |
| **C** | Center Diameter Circle | Circle |
| A | 3-Point Arc | 3-Point Arc |
| **D** | Sketch Dimension | Dimension |
| **T** | Trim | Tangent |
| **O** | Offset | Concentric |
| **M** | Move/Copy | Midpoint |
| **X** | Normal/Construction | Collinear |
| I | Measure | Tangent Arc |
| P, G, U, N | Point, Polygon, Slot, Bézier pen | unchanged |
| — | All constraints (symbols only) | K H V Q W T X O E Y M F |
| — | 2-Point Rectangle moves from B to R | B |
| **Del** / Backspace | Delete the selection (#223) | — (didn't delete) |
| Esc | Cancel the step; again: back to Select | — |
| Enter / Tab | Commit / next typed field | — |
| Ctrl+Z, Ctrl+Y (Ctrl+Shift+Z) | Undo, redo | unchanged |
| Ctrl+C / V / D | Copy, paste, duplicate | — |
| Ctrl+A | Select all | — |
| Ctrl+S / Ctrl+E | Save / Export… | — / — |
| F6 · Ctrl+= · Ctrl+- | Fit · zoom in · zoom out | — |
| ? | Shortcut map | — |
| Alt (hold) | Suppress auto-constraints | unchanged |
| Space (hold) | Pan with the left button | — |

### 5.1 Shortcuts pane (owner request, revision 2)

The map is a **pane, not a dialog**, so it can stay open while working:

- **?** toggles it (as does Help ▸ Keyboard shortcuts). It reopens where it was last, docked or floating.
- It docks like any pane (by default under the Inspector). **⧉** undocks it into its own OS window, for example on a second screen (#285); **⇲ Dock** brings it back. On the web it floats inside the window.
- A filter box matches key or action name. Rows show where a key changed from today's Locus.
- It is `Tab::Shortcuts` in the dock layout, so its place is saved with the layout (§7.4).

Every key is a property of an action in the registry (§7.1), so the ribbon tooltips, menus, marking menu, tool search and this pane can't disagree. Users can rebind keys in Settings ▸ Keyboard; a rebinding that clashes is refused with the conflicting action named. Keys go to the focused text field first, as they do today (`egui_wants_keyboard_input`).

## 6. Flows

### 6.1 Tool search (S)

S opens a search box at the cursor (Fusion's shortcut box). It shows recent and pinned actions before you type. Typing filters every registry action by label and keywords, shows each one's key and group, and shows disabled actions with the reason ("select two lines"). Enter runs the top match; ↑↓ choose another. Select, previously S, is Esc.

### 6.2 Style panel and eyedropper (#287)

- The Inspector's Style section edits the selection: stroke colour, stroke width, pen number, fill (none or a colour), opacity, and a dash pattern where the export formats support one. Fill applies to closed shapes; on Engrave layers it still means hatch (§7).
- **Eyedropper** (in the Style header): click it, then click any object. Its style is copied to the selection in one command. With nothing selected, it sets the current style.
- New geometry takes the **current style**, shown in the Inspector when nothing is selected (default, §9 #5).
- Pen width now comes from each object for the stroke-width preview, hatch spacing on Draw layers and stroke-edge dimensions (ADR 0014).
- **Cut, Score and Drag-knife export** in the profile's colour and hairline, because LightBurn and Cricut read colour as the operation. The Style section says so on those layers. Draw and Engrave export each object's own style (#284) (default, §9 #3).

### 6.3 Export dialog (#291, #284)

- **Profile picker** at the top, with "Manage…" opening the profile manager, and the format (SVG / DXF / PDF / exporter plugins).
- **Layer table**: tick, name, operation, object count and validation result for each layer (§8). Reference and empty layers can't be ticked. "Show" selects a problem on the canvas.
- **Export each layer as a separate file**: when ticked, the dialog lists the file names it will write, `<document>-<layer>.<ext>` with unsafe characters replaced. Every file has the same origin and bounds. Desktop asks for a folder once and asks before overwriting; the web build downloads one `.zip`.
- The button says what will happen: "Export 3 files…".

### 6.4 Profile manager (#286)

A two-pane dialog. On the left are user profiles, then built-in presets marked with a lock, and New, Duplicate, Rename, Delete, Import .json and Export .json. On the right is the editor: name, format (exporter), units, bed size, origin corner (a 2×2 picker), X/Y offsets, and an operations table (colour, hairline width and default parameters per operation). Built-in presets are read-only and show "Duplicate to edit". Imported files are untrusted: size-bounded and validated (§7).

### 6.5 Text (#222)

Click to place the anchor, then type on the canvas. The Text dialog has a live preview and holds:

- **Content**, typed in place.
- **Font list**: a search box plus an **All / System / Custom** filter. Each font previews in its own face and is tagged Bundled, System, Custom or Single-line (Hershey/EMS, for plotters).
- The list is **resizable**: drag its corner or click ⤢ Taller. The size is remembered per user, for people with hundreds of fonts.
- **Add font…** loads a `.ttf`, `.otf` or `.ttc` file. On the desktop it is copied into the app's font folder; on the web it is kept in OPFS. Font files are untrusted input: size-bounded, parsed with `skrifa` without panicking, and refused with a message if unreadable. Documents keep the font reference, not the file (ADR 0012); a document using a missing custom font still opens and exports from its cached geometry. Added fonts appear under Custom, where they can also be removed.
- **Letter spacing** (tracking) in mm, typed or on a slider, with a live preview.
- Height, alignment, and placement: free, fit to box, or on a path.

**Double-clicking placed text reopens the dialog**, as does Edit text in the marking menu. Explode turns text into curves.

**As built (#222):**
- **Text dialog.** It docks at the canvas's top-right while the Text tool is active.
  - After a click places the text, the dialog switches to that text with its content selected, so you type straight in.
  - Double-clicking placed text reopens it. The inspector's editor and its "Edit in dialog" button reach the same editor.
- **Font list.** Search box; All / System / Bundled / Custom filter (System on desktop only); each font drawn in its own face with a tag (Bundled, Single-line, System, Custom). Its height, set by dragging the grip or ↕ Taller, is remembered per user.
- **Add font… and remove.**
  - Add font… loads a `.ttf`, `.otf`, `.ttc` or single-line `.svg` font and keeps it for later sessions: in the app's `fonts` folder on the desktop (atomic write), and in OPFS on the web. Names are sanitised, and sizes and counts are bounded.
  - Custom fonts have a ✕ to remove them.
- **Letter spacing** is in mm, typed or dragged on a slider. It is stored as `TextData::tracking` (format v14).
- **SVG export issues:**
  - Overlapping glyph contours (Figtree's `# $ & * + ^ { }` and more) are unioned on layout, so outline text exports with no "segments intersect" warnings. Texts placed earlier pick this up when they are next edited.
  - The "stray arc": a Bézier's clockwise biarc pieces were taken with their ends swapped wherever export operations or offsets turned a path into a loop. They now follow the curve (`Cubic::to_pieces_oriented`).
  - Canvas drawing of text was checked: the flatness is 0.2 pt. The jagged look the owner saw is most likely the crossing contours, which are now merged; it is left for the owner's hand-check.
- **Not built:** fit to box and text on a path (placement modes).

### 6.6 Offset (#218)

Pick one curve and its connected chain is selected (the Chain selection box is ticked by default). Then either:

- **drag on the canvas**: an arrow handle on the curve sets the distance with a live preview and a heads-up value. Dragging across the curve switches sides, as in Fusion; or
- **type** the distance (number or expression) in the dialog.

The canvas **⇅** control and the dialog's Flip button reverse the side without changing the distance. Corners are Round or Sharp. The handle and the field are one value, so changing either updates the other. Enter commits. The offset stays a live recipe (ADR 0010, 0011) with its distance shown as an editable dimension.

### 6.7 Move, Rotate and Scale (#219)

There are three tools, Move/Copy (M), Rotate and Sketch Scale, sharing one dialog with a Move | Rotate | Scale switch so changing mode keeps the selection. An on-canvas manipulator has X and Y arrows, a free-drag centre, a rotation ring and a scale handle. Rotate picks a pivot (the selection centre by default) and an angle; Scale picks a base point and a factor. **Create copy** is available in all three. Each commit is one command and respects constraints (§6.2). Rotate's current failure gets a regression test (#219).

**As built (#219):**
- Why Rotate failed: every rectangle carries its own Horizontal and Vertical constraints, so an in-place rotation was always refused as "held in place".
- Rotate now handles the constraints that lie wholly inside the selection, in the same undo step. A quarter turn swaps Horizontal and Vertical. A half turn keeps them. Any other angle removes them, along with horizontal and vertical distances, and the status bar names what was removed.
- Sketch Scale scales the selection's driving dimensions (not angles or expression-driven values), as Fusion does.
- Constraints that tie the selection to geometry outside it still apply, and a fix still blocks the change.
- Dragged values snap to 1°, ×0.01 and 0.1 mm; holding Shift snaps to 15°, ×0.1 and 1 mm.
- The pivot (or base point) snaps to points, and Reset in the dialog puts it back at the selection's centre.
- OK or Enter applies the change and returns to Select. Esc first resets the values, and a second Esc leaves the tool.

### 6.8 Patterns (#221)

There is one Pattern dialog with Rectangular and Circular tabs. Rectangular: objects, one or two directions (a line, or the X/Y axis), Extent or Spacing, quantity and distance per direction (expressions allowed), direction angle, One or Symmetric. Circular: objects, centre point, quantity, total angle or spacing, and rotate-instances.

**Everything can be set on the canvas without typing**, as in Fusion (owner request, revision 2):

| Handle | Sets |
|---|---|
| Arrowhead on each direction | That direction's spacing (or extent), dragged along the direction |
| Knob on a ring around the source | Direction 1's angle; direction 2 stays perpendicular unless it was picked from a line |
| − / + beside each count | Quantity per direction |
| Knob at the end of the circular arc | Total angle (snaps to 360° near a full turn) |
| Dot on each instance | Suppress / restore that instance |

The dialog and the handles are two views of the same values: dragging updates the fields live and typing moves the handles. After commit, the parameters are dimensions on the canvas (§6.2.1). The bug where an offset profile couldn't be patterned is fixed and tested in #221.

**As built (#221):**
- **Root cause of the bug:** a recipe refused any source that was another recipe's instance (`DocError::Driven`), and an offset's curves always are.
- **What a pattern may copy now:** the output of an older unsolved recipe: an offset, a pattern or a plugin object. Two things make this safe:
  - Regenerating in ID order writes the source recipe first, and no cycle can form.
  - A pattern that copies any of an offset's curves copies the whole current offset. The offset's curves change in number and kind as it regenerates (round corners add arcs), and the copies are rebuilt when they do.
- **Still not allowed:** a mirror of offset curves. Its copies are solved, and offset curves aren't solver variables.
- **Removing an offset that a pattern copies** is refused (`RecipeInUse`). Exploding it keeps its curves, so the pattern keeps working.
- **Tools:** Rectangular Pattern and Circular Pattern are separate Create tools that share one dialog with Rectangular | Circular tabs. Each direction can be X, Y or a picked line.
- **Not built yet:**
  - The knob for direction 1's free angle. It needs an `Axis` that isn't a line, which is a `.locus` format change, so a picked line stands in for now.
  - The suppress dots on each instance. Suppressing still works from the Patterns panel after the pattern is committed.

### 6.9 Mirror (#215)

Select the objects, then a line or construction line as the mirror line. Merge on mirror line is on by default to avoid double cuts. The copies are tied by Symmetric constraints (ADR 0011), so they follow the source and the line.

### 6.10 Construction (#289)

X, Modify ▸ Normal/Construction, the marking menu's SW slot and the Inspector checkbox all toggle the selection, with one undo step per toggle. The palette's **Linetype** switch makes drawing tools create construction geometry. Construction draws dashed in amber, takes constraints and is never exported (§4).

### 6.11 Chain select (#220)

See §3.1. Single click still selects one curve.

### 6.12 Delete (#223)

Del and Backspace delete the selection in one command. Deleting geometry that recipes or constraints depend on removes them too, and the status bar says what went ("Deleted 3 curves, 4 constraints"). Undo restores everything (§11). Layer delete keeps its warning (§4 rule 4).

### 6.13 Plugins (revision 3)

Plugins (§15) appear where their kind belongs, the way Fusion keeps generators in Insert and add-ins under Utilities:

| Kind (§15.2) | Where it appears |
|---|---|
| Generator | **INSERT ▾**, under a "Plugins" heading, tagged PLUGIN. Running one opens a command dialog that the app draws from the plugin's parameter schema (§15.3): numbers with units and ranges, integers, booleans, choices, text, and document-parameter references. The preview runs in the sandbox; OK inserts the result as one undoable command. |
| Modifier | **MODIFY ▾**, under "Plugins". It works on the selection, with the same schema-drawn dialog. |
| Exporter | The **Export dialog's format list** (e.g. HPGL, G-code), tagged PLUGIN. It gets the same validated export model as the built-in writers; the app does the file dialog and I/O (§15.4). |
| Importer, validator (M4) | Insert ▸ Insert file… picks importers by file type; validators add rows to the export dialog's validation column. |

- **Plugins ▾** app menu: Install plugin…, Manage plugins…, then the installed generators, modifiers and exporters listed by kind.
- **UTILITIES ▸ PLUGINS** on the ribbon: Install plugin…, Manage plugins….
- **Plugin manager**: each plugin's name, id, version, author, licence, kinds, API version, signed or unsigned, capabilities and an enable switch, with Remove. Disabling a plugin removes its actions from menus, the toolbar and search; documents that used it keep their cached output (§15.5). The footer states the limits (time limit, 256 MB memory) and that plugins get no files, network, clock, environment or randomness.
- **Install plugin…** opens a `.locusplugin` and shows a review before anything runs: manifest, API version support, SHA-256, signature status (unsigned is labelled) and requested capabilities. The archive is untrusted: size-bounded and validated.
- Plugin actions are **ordinary registry actions** (§7.1, ids `plugin.<id>.<command>`), so they show in tool search with a "plugin" tag and can be pinned to the toolbar. If a pinned plugin is removed or disabled, its button disappears; it comes back in place if the plugin is re-enabled.
- On the web, plugins run in a Web Worker with the same limits (ADR 0016).

## 7. Architecture for the foundation refactor (#294)

The aim is a structure that several M5 feature sessions can build on without colliding in `canvas.rs`. **#294 changes no behaviour.** It moves today's UI onto the new structure; the new layout and flows above come after it.

What is there today:

- `Canvas` (`canvas.rs`, 2,402 lines) owns the sketcher, view, selection, the drawing draft, modify state, typed input, the dimension editor, touch input, and the UI of every dock tab (`properties_ui`, `constraints_ui`, `layers_ui`, `parameters_ui`, `patterns_ui`, `regions_ui`).
- The tool lists are repeated: `Tool::DRAWING`, `Modify::ALL` and `Kind::ALL` are enumerated in nine places across `app.rs` (menus, tool strip), `canvas.rs` (hotkeys, touch menu) and `layers_panel.rs`. Keys live on three enums (`Tool::key`, `Kind::key` and a literal `'D'`).
- Each tool family handles input in its own way: `Draft` for drawing, `Modify` with `corner_first` / `offset_pick` for modify tools, and the selection for constraints. Text and the pen have their own state on top.

### 7.1 Action registry

```rust
// crates/locus-app/src/actions.rs
pub struct ActionId(pub &'static str);          // "sketch.line", "modify.trim", "constrain.tangent"

pub enum Group { Create, Modify, Constrain, Inspect, Insert, Select, Output, Edit, View, File }

pub enum Enabled { Yes, No(&'static str) }      // the reason shows in menus and tool search

pub struct Action {
    pub id: ActionId,
    pub label: &'static str,
    pub group: Group,
    pub icon: Icon,
    pub keywords: &'static [&'static str],      // tool search
    pub default_key: Option<egui::KeyboardShortcut>,
    pub enabled: fn(&AppState) -> Enabled,
    pub kind: ActionKind,
}

pub enum ActionKind {
    Tool(fn() -> Box<dyn Tool>),                // makes the tool active (§7.2)
    Command(fn(&mut AppState)),                 // runs once: delete, toggle construction, export…
    Toggle { get: fn(&AppState) -> bool, set: fn(&mut AppState, bool) },
}

pub struct Registry { actions: Vec<Action>, keys: KeyMap }
```

- **One static table** (`actions::BUILTIN`) holds every built-in action. Plugin commands (§15) are appended when plugins load, with ids under `plugin.<id>.<command>`, a `Group` from their kind (generator → Insert, modifier → Modify), and a `source: Plugin(id)` field that the menus use for the PLUGIN tag. Exporters register formats with the export dialog, not actions. Enabling, disabling or removing a plugin adds or removes its actions; `Toolbar` keeps the pin and hides the button while the action is missing.
- **Layout is data too.** `DEFAULT_RIBBON: &[(Group, &[ActionId])]` lists each group's default pinned tools; `MARKING_RING: [ActionId; 8]`; `fn marking_list(&AppState) -> Vec<ActionId>`. Menus, the ribbon, the marking menu, tool search, the shortcuts pane and touch long-press all read from the registry; none keep their own list.
- **Pinned actions** (§2.3). The registry owns a `Toolbar { groups: BTreeMap<Group, Vec<ActionId>> }`, starting from `DEFAULT_RIBBON` and overridden by the user's `locus.toolbar.v1`. It has `pin(id, group, index)`, `unpin(id)`, `move_to(id, group, index)` and `reset(group)`. The ribbon draws `Toolbar`, and each group's ▾ menu draws every action in that group with an `is_pinned` flag for the pin icon. Loading drops unknown ids and duplicates, so a plugin that is removed can't leave a dead button. Pins are UI settings and never touch the document.
- **Keys.** `KeyMap` is built from `default_key` plus user overrides saved in settings (`locus.keys.v1`). It is checked when it loads: two actions on one key is an error in a test for the defaults, and a refused rebinding for user overrides. Hotkey dispatch is one loop over `KeyMap`, keeping today's guard that a focused text field or the dimension editor gets keys first.
- **`enabled` gives a reason**, so menus and tool search can say why an action is unavailable instead of hiding it.
- **Every action that changes the document** ends in `Sketcher::apply(Command)`, so it is one undoable command (§11). The registry is UI; it adds no new way to mutate the document.

### 7.2 Tool trait

```rust
// crates/locus-app/src/tool.rs
pub trait Tool {
    fn id(&self) -> ActionId;
    fn prompt(&self, cx: &ToolCx) -> Cow<'static, str>;    // status bar
    fn fields(&mut self) -> &mut [Field];                    // heads-up / dialog values: length, angle, distance…
    fn needs(&self) -> &[Pick];                              // selection boxes: Curves, Chain, Line, Point…
    fn event(&mut self, ev: ToolEvent, cx: &mut ToolCx) -> Flow;
    fn preview(&self, cx: &ToolCx, out: &mut Overlay);       // rubber-band geometry, glyphs, handles
    fn commit(&mut self, cx: &ToolCx) -> Option<Command>;    // one locus_doc command (§11)
    fn dialog(&mut self, _ui: &mut egui::Ui, _cx: &mut ToolCx) {} // only dialog tools draw one
}

pub enum ToolEvent { Down(Pick), Move(Pick), Up(Pick), DoubleClick(Pick), FieldEdited(usize), Key(egui::Key) }
pub enum Flow { Continue, Commit, CommitAndContinue, Cancel, Done }
```

- **A `ToolHost`** owns the active `Box<dyn Tool>`, turns pointer, touch and keyboard input into `ToolEvent`s, routes typed digits into the focused `Field`, draws the heads-up fields, applies Enter / Esc / Tab the same way for every tool, and sends `commit()`'s command through `Sketcher::apply`. Typed fields become driving dimensions inside that same command, which is how values stay editable (§3 step 4).
- **`ToolCx`** gives read access to the document, solved positions, view, selection and the shared **`Picker`**: hit testing, snapping, auto-constraint inference (`infer.rs`), chain walking (#220) and selection filters.
- **Every tool family implements the trait.** Drawing tools are click sequences with fields. The pen is a drawing tool whose `Down`/`Up` drag makes handles. Modify tools fill `needs()` from the selection or from picks. Constraints are tools whose `needs()` is met by the selection, so they commit at once (selection-first). Text and the dialog tools (Offset, Move/Rotate/Scale, Pattern, Mirror) also draw `dialog()`. Select is the default tool.
- **Flow** gives every tool the same Enter / Esc behaviour. `CommitAndContinue` is the polyline case.

### 7.3 Splitting `Canvas`

```
crates/locus-app/src/
  state/            AppState and its parts; no egui widgets
    doc.rs          DocState: Sketcher, solve status, active layer, current style
    view.rs         ViewState: View transform, auto-fit, preview toggles
    selection.rs    Selection, hover, filters, chain select
  actions.rs        registry (§7.1)
  tool.rs  tools/   Tool trait, ToolHost, Picker; one module per tool family
  views/            one module per pane, each `fn ui(&mut self, ui, &mut AppState, &Registry)`
    canvas.rs       paints scene + overlay, feeds ToolHost; no panel UI
    browser.rs  palette.rs  inspector.rs  constraints.rs  parameters.rs
    export.rs  profiles.rs  marking_menu.rs  tool_search.rs  ribbon.rs  status.rs
  app.rs            dock, menu bar from the registry, layout persistence
```

- `AppState { doc, view, selection, tools: ToolHost, settings }` is the one mutable state. A view keeps only transient widget state, such as a search string or which row is expanded.
- **Views never mutate the document.** They call `doc.apply(Command)` or run an action. This keeps §11 enforceable by reading `views/`.
- **Order of work for #294**, each step a PR that is green and changes no behaviour:
  1. Add the registry with today's actions, and drive the existing menus, tool strip, hotkeys and touch menu from it.
  2. Move the state out of `Canvas` into `state/`, and the dock-tab UIs into `views/`.
  3. Add the `Tool` trait and `ToolHost`, and port the tools one family at a time: drawing, pen, modify, constraints, text.
  4. Persist the layout as v2 (§7.4).

After #294, each Phase C feature is a new view or a new tool plus registry entries. Sessions working in parallel then touch separate files.

### 7.4 Saved dock layout: `locus.layout.v1` → `v2`

Today `app.rs` stores `DockState<Tab>` as JSON under `locus.layout.v1`, and falls back to the default if it doesn't parse or has lost the canvas (`parse_layout`). M5 changes the tab set: Layers, Regions and Patterns merge into Browser, Properties becomes Inspector, and Palette and Shortcuts are new. Shortcuts is not in the default layout; ? adds it.

- **New key `locus.layout.v2`** holding `{ "version": 2, "dock": DockState<Tab>, "windows": [ { "tab", "pos", "size", "monitor" } ] }`. `windows` records undocked OS windows (#285).
- **Loading:**
  1. If v2 parses and contains Canvas, use it. Drop any window whose monitor is gone and re-dock its tab.
  2. Otherwise, if v1 parses, **migrate it**: Canvas→Canvas, Properties→Inspector, Layers→Browser, Regions→Browser, Patterns→Browser (duplicates removed, keeping the first), Parameters→Parameters, Constraints→Constraints. Then add Palette above Inspector if it's missing. If the result has no Canvas, use the default.
  3. Otherwise use the default layout.
- The migrated layout is written as v2. **v1 is left in place**, so an older build still opens with its own layout.
- Tests (in #294): v1 default → v2 expected; v1 with every tab moved → no tab lost; corrupt v1 or v2 → default; unknown future version → default, with a warning and no crash. The loader never panics on stored data (CLAUDE.md, untrusted input).
- **As built in #294 (#307):** v2 is the versioned envelope and loader above, with a frozen `v1::Tab` enum. Because #294 changes no behaviour, `migrate_tab` currently maps each v1 tab to the same tab. The new tabs (Browser, Inspector, Palette, Shortcuts) come with the layout ticket. That ticket changes `migrate_tab` to the mapping above and bumps the format to **v3**, using this loader and its tests (`crates/locus-app/src/layout.rs`). The loader also reads unset (`null`) pane rectangles as zero, so a layout saved before a pane was drawn is no longer silently replaced by the default.
- **As built in #285:** `locus.layout.v3` holds `{ version: 3, dock, windows }`. v2 and v1 are both migrated with the mapping above through one frozen `v2::Tab` enum (v2 is v1 plus Shortcuts from #299), and Palette is split in above Inspector. Each undocked pane is an egui viewport drawn by the shell: an OS window on the desktop, and an in-page window on the web, where eframe embeds viewports. A tab gets its own window from its context menu ("Open in its own window"), from egui_dock's Eject, or by dragging it out of the dock. egui_dock floats it inside the main window, and once the drag ends the shell turns that into a pane window at the same place. Closing the window or pressing **Dock** puts the pane back beside its neighbour. Dragging a pane window back into the dock isn't possible across OS windows. Window geometry is bounded when loaded. A window with unusable geometry, or a pane that is both docked and windowed, is re-docked. The saved monitor (its size) is informational only, because egui can't list monitors, so View ▸ Dock all windows is the way back if a monitor is gone.

## 8. Phase C tickets mapped to this design

| Ticket | Design section | Built on |
|---|---|---|
| #290 right-click menu | §4 | registry (marking ring, list) |
| #223 Delete | §6.12 | registry key |
| #289 construction | §6.10 | registry, palette, inspector |
| #220 chain select | §3.1 | Picker |
| #218 offset, #219 move/rotate/scale, #221 patterns, #215 mirror, #217 chamfer | §6.6–6.9 | Tool trait + dialog |
| #222 text | §6.5 | Tool trait + dialog |
| #287 per-object style, #284 export stroke width | §6.2 | inspector; `locus-doc` style field and migration |
| #286 profiles, #291 per-layer export | §6.3, §6.4 | views |
| #285 undock | §2.2, §7.4 | layout v2 |
| Customisable toolbar (new, revision 2) | §2.3 | registry `Toolbar` |
| Shortcuts pane (new, revision 2) | §5.1 | registry + views + layout v2 |
| View menu and navigation bar (revision 3) | §2.4 | registry toggles + views |
| Plugins in the tools (revision 3) | §6.13 | registry plugin actions + views |
| Constraint symbols (revision 3) | §2, §5 | registry (no default keys) + canvas glyphs |
| Ribbon, browser, palette, inspector, status | §2 | registry + views |

## 9. Decisions and defaults

The owner decided item 1 on revision 2. Items 2–5 weren't ruled on, so the design uses the recommended option for each (2026-11-13). Each is a small, contained change if the owner later wants the alternative.

| # | Default | Alternative |
|---|---|---|
| 1 | **Decided (owner):** constraints have **no letter keys**; they are symbol buttons with tooltips, reachable from tool search, the marking menu and pinning | — |
| 2 | **Horizontal/Vertical** as one combined constraint, as in Fusion | Separate Horizontal and Vertical |
| 3 | **Cut, Score and Drag-knife** export in the profile's colour and hairline; Draw and Engrave export each object's style | Object style everywhere |
| 4 | **Marking menu:** a fixed ring of 8 with a contextual list beneath | A ring that changes with context |
| 5 | **Current style** for new geometry, set in the Inspector or with the eyedropper | New geometry takes a fixed default style |
