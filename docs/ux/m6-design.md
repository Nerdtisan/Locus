# M6 design: line, brush and fill styles

- **Issue:** Nerdtisan/Locus#425 (epic #424)
- **Decision record:** [ADR 0019](../adr/0019-line-brush-fill-styles.md)
- **Mockup:** [`m6-mockup.html`](m6-mockup.html), a clickable static prototype in the M5 mockup's visual language. It is not an editor.
- **Spec:** §4, §6.2, §6.2.1, §6.3, §7, §8, §9, §10, §11, §13 (M6); builds on [`m5-design.md`](m5-design.md) and ADR 0017
- **Status:** **for owner sign-off.** No M6 feature is built until the owner approves the mockup.

M6 adds stroke and brush styling like Illustrator's, Affinity's and VectorStyler's, plus gradient fills, an edit history and some workflow fixes. Every style stays editable and has a defined machine result (#424). Everything here follows the M5 tool model: pick, then drag or type with a live preview, then Enter or Esc, and values stay editable. Every change is **one command and one undo step** (§11). A multi-selection is still one step.

## 1. Owner decisions and open questions

Built on these owner decisions:
- Style is per object (#287).
- Fills belong to regions (#451).
- Regions are per layer (#457).
- Both parts of #459 are in scope (2026-11-19).
- Every style has a machine result (#424).

**Owner decisions, 2026-11-24** (SPEC §0):
- **§1 is reworded.** Gradients are visual region fills with a defined machine result, and the timeline is undo history plus recipes, not a feature tree.
- **Gradients get a "Rotate with shape" option per fill, default on** (§6).
- **Auto trace is Locus's own implementation** of the published Potrace algorithm, with no Potrace code (§10).
- **Arcs vs Béziers** for variable-width, calligraphic and paint outlines: spike S1 measured both and chose kept cubic Béziers, with biarcs only for the cleanup and arc-based output (owner-approved 2026-11-24; Expand tolerance default 0.01 mm) ([ADR 0019, S1–S3 results](../adr/0019-line-brush-fill-styles.md#spike-results-s1s3-2026-11-24-track-a-step-a0)).

**Defaults still standing** (ADR 0019 "Consequences"). The mockup shows the recommended option for each.

| # | Question | Recommended | Alternative |
|---|---|---|---|
| 1 | §1 non-goals | **Decided:** §1 reworded (owner, 2026-11-24) | — |
| 2 | Machine layers and styled strokes | **Stroke styles: Centreline** by default; **Styled** per layer | Styled by default |
| 3 | Gradients | On **regions** only (#451); **Rotate with shape** on by default (owner, 2026-11-24) | Also on objects (reverses #451) |
| 4 | History after jumping back | **Linear**: a new edit drops the redo steps | Branching tree |
| 5 | Default cap and join | **Round / Round** (today's look) | Butt / Miter (Illustrator's) |
| 6 | Plotter fill of wide strokes | **Outline + hatch** at the pen width | Outline only |

## 2. Layout changes

The M5 layout stays. M6 adds:

- **Inspector ▸ Style** grows from one block into collapsible sub-sections (§3).
- **Brushes pane** (new, dockable and undockable like every pane, #285): View ▸ Panes ▸ Brushes, or **B** (free since M5).
- **History pane** (new): View ▸ Panes ▸ History, or **Ctrl+H**. It has two tabs, History and Timeline.
- **Browser ▸ Layers:** each row gains a blend-mode chip and drag reordering. The layer's Inspector gains Blending and Regions sections.
- **Ribbon additions:**
  - SKETCH ▸ Create: **Paint brush** (Shift+B).
  - SKETCH ▸ Modify: **Expand to geometry** (Ctrl+Shift+E) and **Width tool** (Shift+W).
  - SKETCH ▸ Insert: **Auto trace…**, next to Insert SVG/DXF.
  - SKETCH ▸ Inspect: **Machine preview**, which also shows styles.
- **Marking menu list** (the ring is unchanged, ADR 0017): with a styled object, **Expand to geometry**, **Edit width profile**, **Apply brush ▸** and **Copy style / Paste style**.

New keys (they don't clash with Fusion's sketch keys or M5):

| Key | Action |
|---|---|
| B | Brushes pane (toggle) |
| Shift+B | Paint brush tool |
| Shift+W | Width tool (edit width points on the canvas) |
| Ctrl+Shift+E | Expand to geometry |
| Ctrl+H | History pane |
| Ctrl+Alt+C / Ctrl+Alt+V | Copy style / Paste style (the eyedropper as keys) |
| Alt+↑ / Alt+↓ | Move the focused layer row up or down (#462) |
| M with a reference selected | Move reference (#463); Rotate and Scale from the same dialog's switch, as for shapes (#219) |

## 3. Inspector ▸ Style (#426–#429, #430–#435)

For curves, text and mixed selections (M5 §2.1), the Style section reads top to bottom:

```
STYLE                                   [eyedropper] [⋯ Copy/Paste style]
  Stroke     [■ swatch] By layer ▾    Opacity [100 %]
  Width      [0.40 mm]  Pen [1]
  Profile    [▭ Uniform ▾] [Edit…]               ← #426 presets + custom
  ▸ Caps & joins   Cap [⊐ ◖ ▣]  Join [◸ ◜ ◿]  Miter limit [4]  Align [centre|in|out]
  ▸ Dashes         [— — — ▾ preset]  dash/gap pairs [2][1][  ][  ]  Phase [0]  ☐ Align to corners
  ▸ Strokes        list: ● 0.40 mm · black · on path   (the base stroke)
                         ● 0.20 mm · red · +0.8 mm  [dash]   ⋮ drag to reorder
                   [+ Add stroke]
  ▸ Brush          [None ▾ | thumbnails…]  Scale [100 %] ⇋ flip ⇅ flip  [Reseed]
  Machine          "Cut layer: cuts the centreline (Stroke styles: Centreline)"  [change…]
```

- **The sub-sections** collapse and remember their state per user. A sub-section with a non-default value shows a dot, so a collapsed one still says it's in use.
- **Mixed values** show "—". Typing sets the value on all of them, in one command (M5 §2.1).
- **Profile presets:**
  - Uniform.
  - Taper both ends.
  - Taper start.
  - Taper end.
  - Bulge (thick middle).
  - Pinch (thin middle).
  - Pressure (simulated): a smooth random profile from a seed, with **Reseed**.
  - Custom, when the points were edited.
  - Each preset is a set of width points, so editing one turns it into Custom.
- **Dash presets:**
  - Solid.
  - Dash 3/2.
  - Dot (0/1.5 with round cap).
  - Dash-dot.
  - Long dash.
  - Up to 8 dash/gap pairs.
  - Fields take expressions over parameters (M3), like every length.
- **Multi-strokes:**
  - The list always starts with the base stroke (the object's own width, colour and pen).
  - **+ Add stroke** copies the selected row, offset by its width.
  - Each row edits width, colour, pen, offset (signed: + is left of the path, or outside on a clockwise closed shape) and its own dash.
  - Drag the ⋮ grip to reorder. The paint order is the list order, the first row drawn first.
  - Bound: 16 rows.
- **Brush picker:**
  - The document's brushes first, then the library, as thumbnails.
  - Picking one applies it.
  - A brush replaces Profile and Dashes, which dim with "Set by the brush". Caps and joins stay for calligraphic.
  - **Reseed** is shown for scatter brushes.
- **Machine row:** a sentence saying what this object's layer will do with the style (§9). **change…** opens the layer's Inspector at its **Stroke styles** setting.

**Current style** (M5 §9 #5) carries all of this, so new geometry and the paint brush pick it up.

## 4. Width-profile editor (#426)

Two linked editors:

1. **On the canvas (Width tool, Shift+W, or Profile ▸ Edit…):**
   - The selected path shows its **width points** as diamond handles on both edges of the stroke, joined across the path by a thin line.
   - **Hover** previews the width at the cursor.
   - **Drag a handle across the path** to change that side's width. Holding **Alt** changes one side only (asymmetric). The default is symmetric.
   - **Drag along the path** to move the point. It can't pass its neighbours.
   - **Click on the stroke edge** to add a point. **Double-click a point** to type its width (left, right, in mm or %).
   - **Delete** removes the focused point. The two end points stay.
   - **Tab** cycles through the points, and **←/→** nudge the position by 1 % (Shift: 10 %).
   - Each drag is **one undo step on release** (one `SetStyle`), with live preview while dragging.
   - The HUD shows `t 42 % · W 1.20 mm (L 0.60 / R 0.60)`.
2. **In the Inspector (Profile ▸ Edit… opens a docked command dialog, M5 style):**
   - A graph: width (y) over path length (x), with the left and right curves mirrored. Its points are linked to the canvas handles.
   - Presets, **Flip** (reverse along the path), **Mirror** (copy left to right), **Save as preset…** (into the user library, §5) and **Reset**.
   - OK commits. Cancel restores (Esc).

States:
- **Constant-width spans** keep exact arcs (ADR 0019 decision 3).
- **A profile on a closed path** wraps: t = 0 and t = 1 are the same point, so their widths are linked.
- **Construction curves** show no styled stroke (§4), so the editor is disabled there with "Construction lines aren't styled".

## 5. Brushes pane (#435) and the brush types (#430–#434)

```
BRUSHES                                   [list|grid] [⋯]
  [search…]                       Show: All | Document | Library
  IN THIS DOCUMENT
    ▬ Rope (art)           2 uses
    ✒ Nib 30° (calligraphic) 1 use
  LIBRARY
    ⋯ Stitch (pattern)  · ✲ Leaves (scatter) · …
  [+ New from selection ▾]  [Edit]  [Duplicate]  [Delete]  [Import…]  [Export…]
```

- **Applying:** click a brush to apply it to the selection (one `SetStyle`), or drag it onto a path. With nothing selected, it sets the current style.
- **New from selection ▾:**
  - Calligraphic needs no selection.
  - Art, Scatter and Pattern use the selected geometry as the artwork.
  - The **New brush dialog** previews the artwork along a sample S-curve. It also asks for the name, the type's options and the artwork's direction (an arrow overlay: which way is "along the path").
  - The artwork is **copied**, so the brush doesn't follow later edits to those shapes (ADR 0019 decision 1).
  - Pattern brushes have five tile slots (side, outer corner, inner corner, start, end). Each is filled by selecting geometry and clicking the slot's **Set from selection**.
- **Edit:**
  - Opens the same dialog for an existing brush.
  - **OK applies to every use in this document**, as one `EditBrush` step, and the dialog says how many objects change.
  - A library brush edited here is saved to the library only with **Save to library**.
- **Delete** of a brush in use asks: **Replace with plain stroke** (one compound step) or Cancel.
- **Library:**
  - Per user, `locus.brushes.v1`, plus bundled starter brushes (rope, chain, stitch, dots, leaves, two nibs), which are read-only with a lock, like the profile presets.
  - Import and export `.locusbrush` (JSON, untrusted, bounded).
  - A document carries copies of its brushes.
  - **Update from library** appears on a document brush whose library twin has changed.
- **Brush type options:**
  - **Calligraphic (#430):** angle, roundness, size, and variation (fixed, random from seed, or "pressure" when drawn with a pen), with a live nib preview.
  - **Art (#431):** Stretch or Proportional, protected start and end lengths (shown as two handles on the preview), flip along and across, and colour (Keep, or Tint to the stroke colour).
  - **Scatter (#432):** size, spacing, offset from the path and rotation. Each is Fixed or Random (min–max), and rotation is relative to the path or to the page. The seed is per use.
  - **Pattern (#433):** fit (Stretch, Add space, Approximate), spacing, and the five tiles.
- **Paint brush tool (#434, Shift+B):**
  - Draw freehand with the mouse, pen or touch (#250, #251). Smoothing is set in the palette (0–100).
  - Width comes from pressure with a pen, or simulated (speed) with a mouse.
  - Release commits **one path plus a width profile**: a Bézier chain fitted within the smoothing tolerance, never raster.
  - **Contour mode** (a palette toggle) commits the expanded outline as a closed path instead.
  - Edit it afterwards like any profile (§4).

## 6. Gradient fill editor (#436)

Regions only (open question 3). Select a region (#370), then **Inspector ▸ Fill**:

```
FILL
  ☑ Fill   Type [Solid | Linear | Radial | Elliptical | Conic]   Opacity [100 %]
  ☑ Rotate with shape
  [■■■■■■■■■■■■■■■■■■ stop bar with ▲ stops ]   ⇄ reverse
  Stop  [■ #1f6fd1]  Position [35 %]  Opacity [100 %]   [Delete stop]
  Angle [30°]  Ratio [60 %] (elliptical)
  Presets [▦ ▦ ▦ ▦ ▦  + Save]
  ▸ Machine   None | Hatch by luminance (spacing 0.2–1.2 mm, angle 45°) | Laser power (needs a greyscale profile)
```

- **On the canvas**, while the region is selected and Show fills is on (#338):
  - A **gradient line**, with a round start handle, a square end handle and the stops as small swatches on the line.
  - Drag the handles to move or rotate the gradient. Shift snaps to 15°.
  - Drag a stop along the line to move it, or off the line to delete it. Click the line to add a stop.
  - Radial and elliptical gradients add a **radius ring**, and elliptical adds a **ratio handle**.
  - Each drag is one `SetRegionFill` on release.
- **Several regions selected:** shared values show, differing ones show "—", and one edit applies to all, each region keeping its own box.
- **Rotate with shape** (per fill, **default on**, owner decision 2026-11-24):
  - The handles are stored in the gradient's frame: the region's box, measured along the frame's angle. The frame is refitted after every edit, so the gradient always follows a move or scale.
  - With the option **on**, a Rotate or Mirror command on the shape (#219, #215) also turns or flips the frame, in the same undo step, so the gradient turns with the shape, as in Illustrator and Affinity.
  - With it **off**, the gradient keeps its page direction when the shape rotates, for example "light from the top" on parts rotated for nesting.
  - Toggling it doesn't change the current look.
  - Its tooltip: “Follows Rotate and Mirror. Reshaping by dragging points refits the gradient to the new outline.”
- **Swatches and presets** are saved in the user library next to brushes (#435).

## 7. History and timeline (#446)

A dockable **History** pane with two tabs:

- **History:**
  - Every undo step's label, newest at the bottom, with the current state highlighted.
  - Click a row to go back to it (one batch, one re-solve). Later rows dim but stay until you edit, then they go (linear, open question 4).
  - ↑/↓ move the selection, Enter jumps to it, and Ctrl+Z and Ctrl+Y move one row.
  - The pane footer says "Session only · 1 000 steps" (ADR 0019 decision 5).
- **Timeline:**
  - Features in creation order, as a Fusion-style strip of icons with names: mirror, patterns, offset, booleans, text on path, plugin objects, brushed styles.
  - Hovering one highlights its geometry. Double-clicking edits it (its M5 dialog).
  - The right-click list offers Edit, Suppress or Unsuppress, Delete, and Explode.
  - **Drag** reorders only where nothing depends on the feature. Otherwise the drop is refused, with "Offset 2 uses Pattern 1's copies".
  - **A failed feature shows red** with its error (for example "Offset collapses at −3 mm"). It keeps its last good result, and the sketch still solves.
  - Suppressed features show struck through.

## 8. Layers: blending, interaction, reorder (#459, #462)

**Browser ▸ Layers:**
- Each row shows a blend chip when the mode isn't Normal ("Multiply"), an opacity when it's below 100 %, and a **link badge** when the layer is cut by or knocked out by other layers.
- **Drag reorder (#462):**
  - Grab a row (or its ⋮ grip). A **2 px insertion line** shows where it will land. Dropping it is one `ReorderLayer` step.
  - Esc cancels the drag.
  - Alt+↑ and Alt+↓ move the focused row.
  - The canvas draw order and the export order follow, and the Export dialog's layer table matches.

**Layer Inspector** (M5 §2.1 plus):

```
BLENDING           Mode [Normal ▾]  Opacity [100 %]      (visual only: machines ignore it)
REGIONS            ◉ Own shapes only (default)  ○ Interact with all layers (document switch)
                   Cut by       [☐ Linework ☑ Outline ☐ …]
                   Knock out by [☐ Text ☑ Holes …]
                   "Engrave hatch changes: see Machine preview"
STROKE STYLES      (machine layers) ◉ Centreline  ○ Styled
FILLED STROKES     (Draw layers on a plotter) [Outline + hatch ▾]
```

- **Interact with all layers** is the document switch (ADR 0019 decision 6). Turning it on greys out the per-layer lists and says so.
- **Cut by** and **Knock out by** are directional. The marking-menu item **Interact with ▸ layer** sets both directions at once.
- **Undo:** every change is one step. Deleting a layer drops links to it in the same step, and the delete warning lists them.

## 9. Machine output, per style

The full table is ADR 0019 decision 4. In short:

| Style | Draw (pen plotter) | Draw (generic, laser or vinyl profile), SVG / PDF / PNG | Cut / Score / Drag knife | Engrave | DXF / HPGL |
|---|---|---|---|---|---|
| Width, caps, joins | outline + hatch at the pen width; a centreline when the width ≤ the pen | native stroke attributes | Centreline: the path; Styled: the outline | Centreline: the path; Styled: the hatched outline | outline |
| Dashes | one pen-down per dash | `stroke-dasharray` (expanded when aligned to corners) | Styled: perforation | Styled: dashed lines | one entity per dash |
| Multi-strokes | one path per stroke, own pen | one path per stroke | Styled: parallel cuts | Styled: parallel lines | offset paths |
| Width profile, calligraphic, paint | outline + hatch | filled outline | Styled: outline | Styled: hatched outline | outline |
| Art, pattern, scatter brushes | artwork paths, closed art hatched | the mapped artwork | Styled: artwork outlines | Styled: hatched artwork | artwork outlines |
| Gradient fill | variable-density hatch if chosen, else none | native gradient (conic as bands); PNG rasterised | ignored | hatch by luminance if chosen; laser power only with a greyscale profile | hatch if chosen, else none |
| Blend and opacity | ignored | `mix-blend-mode` / PDF `/BM` / `tiny-skia` | ignored | ignored | ignored |
| Knockout and cut-by | changes the hatch areas | changes the fills | n/a | **changes the hatch areas** | n/a |

**Expand to geometry** (Ctrl+Shift+E, Modify, marking menu):
- It opens a small command dialog with **Outline** (filled areas as closed curves) or **Centrelines** (the pen paths: dashes, multi-strokes, art lines), and Keep original (hidden on a Reference layer) on or off.
- The dialog previews the result in the preview colour, with a count ("214 curves, 38 arcs").
- OK is one `ExpandStyle` step. The result is plain, editable curves on the object's layer, with its colour and pen.
- Gradients expand to stepped bands (count field) or to their hatch.
- A recipe instance must be exploded first (ADR 0011), and the dialog says so.

**Machine preview** (the M5 pane) shows Styled layers as the machine makes them. Its legend gains one line per styled layer, for example "Score · Styled: 120 dashes → perforation".

## 10. Auto trace (#461)

Insert ▸ **Auto trace…**, or right-click a raster reference. A modal dialog, because tracing is a one-off conversion (the engine is Locus's own `locus-trace`, written from the published Potrace algorithm, owner decision 2026-11-24):

```
AUTO TRACE                                                  [×]
  Source  [reference.png ▾]   Calibrated: 0.12 mm/px (#391)
  Mode    [Threshold | Edges | Colours | Centreline]
  Threshold [128] ☐ Auto (Otsu) · Colours [4] → 4 layers · Smoothing [1.0] · Speckles < [4 px]
  Corners [100°] · Curve fit [lines+arcs | Béziers] · Tolerance [0.05 mm]
  [ live preview over the image: before | after | overlay slider ]
  "1 284 curves · 3 layers"                 Output layer [Trace ▾]   [Cancel] [Trace]
```

- **Preview:** it refreshes after each change, cancelling any trace still running. A status line shows progress; there is no time limit in core (§3), only a step budget.
- **Bounds:** above the entity limit, Trace is disabled with "Too many curves (240 000 > 200 000): raise smoothing or speckle size".
- **Trace** is one `TraceImage` step. The result is ordinary geometry at the reference's scale. **Colours** mode puts each colour on its own new layer.
- **Esc** cancels the dialog. Tracing never changes the reference.

## 11. References: the same Move / Rotate / Scale handles as shapes (#463)

Owner clarification, 2026-11-24: references use the **same tools and handles as shapes**, the Move / Rotate / Scale tools from #219 (M5 design §6.7), so they are placed just as precisely. Every value can also be typed while dragging, as in Fusion's Move/Copy.

- **Starting a tool:**
  - Select a reference (click its image or geometry; selection doesn't edit it).
  - Then pick **Move** (M), **Rotate** or **Scale** from the ribbon, the marking menu or the Inspector's buttons.
  - The M5 manipulator appears: red X and green Y arrows, a centre square for free moves, a rotation ring and a scale handle.
- **Move:** drag an arrow (one axis) or the centre square, or type the **X / Y distance**. It snaps to points and the grid as shapes do, in 0.1 mm steps (Shift: 1 mm).
- **Rotate:** drag the ring, or type an **angle**. The pivot is the reference's **centre** or a **picked point** (snapping to points). Steps are 1°, and Shift snaps to 15°.
- **Scale:** drag the scale handle (uniform; Shift for free X / Y), or type a **factor**. It scales about **Centre**, **Anchor corner** or a **picked point**: the same choice as Calibrate (#391, #464, #468), and remembered with it.
- **Command dialog:**
  - It docks top right, as in M5: a Move | Rotate | Scale switch (changing mode keeps the selection), then that mode's fields and pivot choice.
  - **Tab** moves between fields. **Enter** (OK) commits one `TransformReference`. **Esc** first resets the values, then leaves the tool (M5 §6.7).
  - The HUD beside the reference shows ΔX/ΔY, the angle or the factor while dragging.
- **Inspector:** a selected reference shows X / Y, Angle and Scale (%) fields. Typing one commits the same command.
- **Locking:** the Reference layer stays locked against accidental edits. Only these tools move a reference, so a stray click or drag can't. Calibrate still works the same.
- **Undo:** one `TransformReference` per commit (§11). Images gain a stored rotation (a format bump). Vector references transform their entities.

## 12. Bézier handles (#470)

- **Size:** control handles and anchors are drawn at **9 px** (theme `HANDLE_PX`) at every zoom, with a 1.5 px outline in the canvas colour, so they read over fills.
- **Hit area:** **16 px** (`HANDLE_HIT_PX`), larger than the drawn handle.
- **Hover:** a ring in the selection colour, and the cursor changes.
- **Shapes:** anchors are squares, control handles are circles, and the same sizes apply to the M5 offset, pattern and fillet handles.
- **Theme:** the sizes live in `theme.rs`, not in the tools.

## 13. Fonts from the OS (#477)

There is no new UI. The M5 font picker's **All | System | Custom** filter now includes OS-registered fonts (Adobe Creative Cloud, FontBase…). The picker's help tooltip says: "Adobe Fonts appear while Creative Cloud is running and the font is activated." A document whose font later goes missing falls back as today, with the existing notice.

## 14. Suggested build order (posted on #424)

At most two build sessions at a time. Spikes first, because they decide engine details (ADR 0019).

**Track A: the style engine** (one session at a time, since every ticket here touches `locus-stroke` and the Style section):

| Step | Ticket | Needs | Why here |
|---|---|---|---|
| A0 | **Spikes S1 + S2 + S3** (variable-width outline, **biarcs only vs kept cubic Béziers**, calligraphic accuracy, canvas cost) | — | They decide #426/#430/#434, the outline representation and the canvas budget |
| A1 | **#428 caps and joins** + the `locus-stroke` crate, `Style.line`, the first format bump, `ExpandStyle`, the **Stroke styles** layer option, Machine preview and page bounds (#450) | A0 (S3) | Exact, smallest style; lands the whole pipeline once |
| A2 | **#427 dashes** | A1 | Exact; perforation is a real machine win |
| A3 | **#429 multi-strokes** | A1, A2 | Offsets plus per-stroke dash |
| A4 | **#426 width profiles and taper** + Width tool | A1, S1 | The variable-width engine |
| A5 | **#430 calligraphic** + **#435 brushes pane and library** (brush storage, format) | A4 | First brush type and the brush plumbing together |
| A6 | **#431 art brush** | A5 | Path mapping |
| A7 | **#433 pattern brush** | A6 | Reuses path mapping and adds corner tiles |
| A8 | **#432 scatter brush** | A5 | Seeded placement (can swap with A7) |
| A9 | **#434 paint and contour brush** | A4, touch (#250/#251) | Freehand to path plus profile |

**Track B: independent work** (the second session, in this order to avoid file clashes):

| Step | Ticket | Needs | Notes |
|---|---|---|---|
| B1 | **#470 Bézier handles** | — | Small, `locus-app` only; quick win |
| B2 | **#462 drag-reorder layers** | — | Browser view plus `ReorderLayer` |
| B3 | **Spike S4** then **#477 OS fonts** | — | `locus-app` and `locus-text` trait only |
| B4 | **#463 reference handles** | — | `TransformReference`, image rotation (format) |
| B5 | **#446 history and timeline** | — | `Editor::history()`, pane, `SetRecipeSuppressed` (format) |
| B6 | **#436 gradient fills** | B5 not needed; **after A1** (shares the export paint code) | Region fill (format), canvas handles |
| B7 | **#459 blending and layer interaction** | **after B6** (the same region resolver and export paint) | Layer fields (format), resolver links |
| B8 | **#461 auto trace** (benchmark S5 inside the ticket) | — | New `locus-trace` crate, implemented from the Potrace paper (owner decision) |

**Last of all:** the egui 0.37 upgrade (#397, §13), once both tracks are merged.

**Clash rules:**
- A1 and B6/B7 both touch `locus-export` paint code, so B6 starts only after A1 merges.
- Every ticket that stores data takes the **next free format version when it merges** (ADR 0019 decision 2), and rebases its migration test if another bump lands first.

## 15. What the mockup shows

**Flows:**
1. Style section.
2. Width profile (canvas handles plus graph).
3. Dashes, caps and joins.
4. Multi-strokes.
5. Brushes pane (library, new from selection, edit).
6. Gradient fill.
7. History and timeline.
8. Layer blending and interaction.
9. Drag-reorder layers.
10. Reference handles.
11. Auto trace.
12. Bézier handles (before and after).
13. Expand to geometry.
14. Machine preview with the style-to-operation mapping.

**Not in it:** the paint brush's freehand input (it uses the same Style and profile UI) and the OS font list (no new UI).
