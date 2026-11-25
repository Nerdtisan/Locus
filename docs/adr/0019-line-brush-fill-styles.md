# ADR-0019: Line, brush and fill styles: style model, stroke-expansion engine, machine output

- **Status:** Proposed (design pass; the owner signs off the mockup before any M6 feature is built)
- **Date:** 2026-11-24
- **Spec:** `docs/SPEC.md §4` (entity style), `§6.2` and `§6.2.1` (offsets, bounded counts), `§6.3` (regions, fills), `§7` (operations), `§8` (export), `§9` (fonts), `§10` (format), `§11` (commands, undo), `§13` M6; adds §12 item 17
- **Issue:** Nerdtisan/Locus#425 (epic #424). Covers #426–#436, #446, #459, #461, #462, #463, #470, #477
- **Builds on:** [ADR 0008](0008-booleans-and-arcs.md) (arrangement), [ADR 0010](0010-offsets.md) (offsets), [ADR 0011](0011-associative-recipes.md) (recipes), [ADR 0017](0017-fusion-style-ui.md) (UI)
- **Design:** [`docs/ux/m6-design.md`](../ux/m6-design.md), clickable mockup [`docs/ux/m6-mockup.html`](../ux/m6-mockup.html)

## Context

M6 adds Illustrator-, Affinity- and VectorStyler-style strokes and brushes to Locus, plus gradient fills and several workflow tickets. The owner's rule for the whole milestone (#424, #425): **every style stays editable and scalable, and every style has a defined machine result**, because Locus drives pen plotters, cutters and lasers.

What exists today (`origin/dev` at `784c91f`, format v21):

- `locus_doc::Style` is a small `Copy` struct on every entity: stroke colour (or by layer), width, pen, a legacy `fill` (always empty since v19), and opacity. A default style isn't written to the file (§10).
- Fills belong to regions (§0, #451): `Document.region_fills: BTreeMap<RegionKey, RegionFill { fill, opacity }>`, resolved by one function for the canvas, the Inspector and export (#457).
- `locus-region` owns exact line/arc offsets of areas and open chains, with round or miter joins (ADR 0010). Cubics are lowered to G1 biarcs within `EXPORT_TOL` (`Cubic::to_pieces`).
- Recipes (ADR 0011) are ordered by a monotonic `RecipeId` and regenerate after every solve.
- Undo is a session-only stack of patch transactions in `Editor` (`command.rs`). It is unbounded, and a new command clears redo.
- Raster references (`RefImage`, #390) have an origin, a scale and an opacity, but no rotation.
- Export runs `prepare_for` (operation transforms) and then the writers (§8). PNG uses `tiny-skia`.

Forces:

- **§3:** core crates stay UI-agnostic and build for `wasm32-unknown-unknown`, with no clock, threads or filesystem.
- **§11:** every change is one command and one undo step.
- **§6.2.1:** generated counts are bounded (10 000 instances, 200 000 entities). Hostile files never panic the app.
- **§2:** licences must pass `cargo deny`. This design pass adds no dependencies.
- **§10:** a format bump comes with a migration test, and saves stay deterministic.
- **ADR 0010 / 0008:** laser and DXF output want true arcs.

## Spec conflicts flagged (§17)

1. **§1 Non-goals vs §13 M6: resolved by the owner, 2026-11-24.** §1 said "no gradients" and "Parametric feature timeline / history tree", but M6 plans gradient fills (#436) and an edit timeline (#446). The owner chose to update the spec. §1 now says that gradients are visual region fills with a defined machine result, and that the timeline is the undo history plus the recipes, not a feature tree. §0 records it as "Owner decision 2026-11-24".
2. **#436 says "fill objects and regions".** Since #451 (§0, §4), objects have no fill. This design puts gradients on **region fills only**. Object fills stay refused.
3. **#459 interacting layers vs §6.3 "Regions are per layer".** This is opt-in and off by default, so #457 stays the default. §6.3 is amended when #459 is built.
4. **Auto trace.** Potrace (GPL-2.0-or-later) is *not* on the `deny.toml` allow list, which has GPL-3.0-only/-or-later but not GPL-2.0-or-later, and it is C. **Owner decision 2026-11-24:** Locus writes its own implementation from the published algorithm. It uses no Potrace code and needs no licence exception (decision 8).

## Decision 1: the style model in `locus-doc`

### Options

**A. Everything inline on each entity.** `Style` grows every field, and brushes are copied into each object.
- Simple ownership.
- Duplicates brush artwork in every object that uses it, so files grow, and editing a brush can't update its users.

**B. Named document styles only.** Each entity references a `StyleId` from a document table, like Illustrator's graphic styles.
- Shared edits come for free.
- Every one-off tweak needs a new table entry. That fights the M5 Inspector model, where you select an object and change its width.

**C. Inline line style, shared brush definitions (chosen).**
- Each entity keeps its own stroke properties, as M5 does.
- Brushes are defined once in the document and referenced by id.
- The user library lives in app settings, like machine profiles.

### Decision: C

```text
Style (per entity, §4)                       // stays omitted from .locus while default
  stroke, width, pen, opacity                // unchanged (M5)
  line: Option<Arc<LineStyle>>               // None = today's plain stroke; Arc keeps Clone cheap

LineStyle
  cap:   Butt | Round | Square               // default Round (today's look and ADR 0010's tube)
  join:  Round | Miter { limit } | Bevel     // default Round; limit 1..=100, default 4
  align: Centre | Inside | Outside           // closed paths only; default Centre
  dash:  Option<Dash>
  profile: Option<WidthProfile>              // #426; also the result of the paint brush (#434)
  brush: Option<BrushUse>                    // #430–#433; replaces profile/dash where they don't apply
  extra: Vec<StrokeLayer>                    // #429 multi-strokes, ≤ MAX_STROKE_LAYERS (16)

Dash        { pattern: Vec<f64> (dash, gap pairs, mm, ≤ 8 pairs), phase: f64 (mm),
              align: Exact | Corners }       // Corners = Illustrator's "align to corners and path ends"
WidthProfile{ points: Vec<WidthPoint> (2..=64, sorted by t) }
WidthPoint  { t: f64 (0..=1 of arc length), left: f64, right: f64 (× width, 0..=20) }
StrokeLayer { offset: f64 (mm, + = left), width: f64, stroke: Stroke, pen: u32,
              dash: Option<Dash>, cap: Option<Cap> }  // the entity's own stroke is layer 0
BrushUse    { brush: BrushId, scale: f64, flip_along: bool, flip_across: bool, seed: u64 }
```

- **Brush definitions:**
  - They live in `Document.brushes: BTreeMap<BrushId, Brush>`.
  - `BrushId` comes from the shared ADR 0003 counter, like layers.
  - The map is omitted while empty.
  - A `Brush` is `{ name, uid, kind, art }`.
  - `kind` is one of:
    - `Calligraphic { angle, roundness, size, variation }`
    - `Art { fit: Stretch | Proportional, start_len, end_len, colour: Keep | Tint }`
    - `Scatter { size, spacing, offset, rotation, rotate_with_path }`, where each value is a `Range { min, max }`
    - `Pattern { fit: Stretch | AddSpace | Approximate, spacing }`, with up to 5 tiles (side, outer corner, inner corner, start, end)
  - `art` is a **snapshot** of Locus geometry: lines, arcs and cubics in the brush's own frame, with +x along the path and +y to its left. Each piece has a colour and a closed/filled flag. The snapshot is copied from the selection when the brush is made, so the brush never links to live entities.
  - Bounds: ≤ 2 000 pieces per artwork, ≤ 5 tiles, ≤ 256 brushes per document, every number finite and in range. Files that break a bound are refused with a `DocError`, never a panic.
- **User library:** app settings `locus.brushes.v1`, as JSON, with import and export as `.locusbrush` files. These are untrusted input, so they are size-bounded and validated like machine profiles (§7).
  - A document carries a **copy** of every brush it uses, so it opens anywhere (as text carries its glyphs, ADR 0012).
  - The `uid` links the copy to its library entry. "Update from library" and "Save to library" are explicit, so a library edit never changes a saved document silently.
- **Gradients on region fills (#436):**
  - `RegionFill` gains `gradient: Option<Gradient>`. `fill` stays the solid colour and is used when the gradient is off, the same "remember the last choice" idea as #388.
  - `Gradient` is `{ kind, from, to, stops, machine, rotate_with_shape, frame_angle, frame_flip }`:
    - `kind`: `Linear | Radial | Elliptical { ratio } | Conic`.
    - `from` and `to`: points normalised 0..1 in the gradient's **frame**.
    - `stops`: `Vec<Stop { pos, colour, opacity }>`, 2..=32 stops.
    - `machine`: `None | Hatch { min_spacing, max_spacing, angle } | Power`.
    - `rotate_with_shape`: `bool`, **default on** (owner decision 2026-11-24, #436).
    - `frame_angle` (radians) and `frame_flip`: omitted while 0 / false.
  - **The frame** is the region's bounding box measured along axes turned by `frame_angle`, and mirrored when `frame_flip` is set. It is always derived from the region's current geometry and never stored.
    - **Any edit** (a solve, a dimension, a drag): the frame is refitted, so the gradient follows moves and scales, including non-uniform ones along its axes.
    - **Rotate and Mirror commands** (#219, #215) that move every entity on the region's key: with **Rotate with shape** on, the same command adds the rotation to `frame_angle` or toggles `frame_flip`, in the same undo step. So the gradient turns and flips with the shape, as one would expect from a transform.
    - **With it off:** `frame_angle` stays as it is, so the frame stays axis-aligned (0 unless the user set it). The gradient keeps its page direction, for example "light from the top" on parts rotated for nesting.
    - Turning the option on or off is one `SetRegionFill` step and leaves the current look unchanged.
    - Regions are derived faces with no transform of their own (§6.3), so only commands that carry a rotation or mirror can say how much the shape turned. A solver-driven reshape that happens to rotate a shape is refitted, not rotated. This is stated in the Inspector's tooltip.
  - **Why default on:**
    - It matches Illustrator and Affinity, where a gradient is part of the object's appearance and turns with it.
    - Rotating a styled shape should look like rotating the object, the same rule as brushes and patterns that follow their path.
    - Users who want a page-fixed direction turn it off per fill.
    - Files gain nothing while the default holds, because `frame_angle` is 0 for every fill written before a rotation.
- **Layers (#459):** `Layer` gains `blend: BlendMode`, which is Normal, Multiply, Screen, Overlay, Darken, Lighten, Difference, Exclusion, Hue, Saturation, Color or Luminosity, all supported by `tiny-skia`, SVG and PDF. It also gains `opacity` and `regions: RegionLinks { cut_by: Vec<LayerId>, knockout_by: Vec<LayerId> }`. `Document` gains `layers_interact: bool`. All are omitted at their defaults (decision 6).
- **References (#463):** `ImagePlacement` gains `rotation` in radians, about the image's centre, omitted while 0. Vector references are entities, so they are transformed by a new command and need no format change.
- **Commands (§11):**
  - Every edit above is one command and one undo step.
  - `SetStyle` already exists and takes the whole style, so it covers line styles too. A multi-selection is still one step.
  - New commands: `AddBrush`, `EditBrush`, `RemoveBrush` (refused while used, unless "replace with plain stroke" is chosen, as one compound step), `SetRegionFill` (extended with the gradient), `SetLayerBlend`, `SetLayerRegions`, `SetLayersInteract`, `ReorderLayer { layer, to }` (#462), `TransformReference { layer, transform }` (#463, generalising `ScaleReference`), `ExpandStyle { ids, mode }`, `SetRecipeSuppressed` (#446) and `TraceImage { layer, params, target_layer }` (#461).
  - The MCP tools (§16) get these through the same `Command` enum.
- **`Style` stops being `Copy`.** The `Arc` keeps cloning cheap. This is mechanical churn in `locus-app` and `locus-export`, and it lands in the first style ticket.

## Decision 2: file format and migration

### Options

**A. One big bump (v22) for all of M6**, landed with the first ticket.
- Every later ticket would edit an already-released format without a version, which breaks the "newer files are refused" guarantee.

**B. One bump per ticket that adds stored data, in merge order (chosen).**
- This is what M5 did (v13–v21). Each PR carries its own migration test.

### Decision: B

The next free version goes to whichever ticket merges first. Every new field is **optional and omitted at its default**, so a v21 file loads unchanged and saves byte-identically until the user styles something. Older apps refuse newer files, as today (`DocError::NewerFormat`). Expected bumps:

| Ticket | Adds | Migration |
|---|---|---|
| #428 (first style ticket) | `Style.line` with cap, join, align | none: absent = Round / Round / Centre, today's look |
| #427 | `LineStyle.dash` | none |
| #429 | `LineStyle.extra` | none |
| #426 | `LineStyle.profile` | none |
| #435 (with #430) | `Document.brushes`, `LineStyle.brush` | none |
| #436 | `RegionFill.gradient` (with `rotate_with_shape`, `frame_angle`, `frame_flip`) | none |
| #459 | `Layer.blend`, `opacity`, `regions`; `Document.layers_interact` | none: absent = #457 behaviour |
| #463 | `ImagePlacement.rotation` | none |
| #446 | `Recipe.suppressed` | none |

Each bump gets the usual tests (§14): load the previous version's fixture, check round-trip byte-identity, and check that hostile values (NaN, out of range, over the bounds) are refused.

## Decision 3: the core stroke-expansion engine

### Options

**A. Inside `locus-region`.** It already has offsets and booleans, but it would grow brush, dash and scatter code that has nothing to do with regions.

**B. Inside `locus-export`.** The canvas and `ExpandStyle` (in `locus-doc`) need the same engine, and `locus-doc` must not depend on the exporters.

**C. A new core crate, `locus-stroke` (chosen).**
- It depends on `locus-geom` and `locus-region`.
- `locus-doc` (for `ExpandStyle` and validation), `locus-export` (writers and Machine preview) and `locus-app` (canvas) use it.
- The parameter types (`LineStyle`, `Dash`, `WidthProfile`, `Brush`…) live in `locus-stroke` with serde derives, and `locus-doc` re-exports them. This avoids a cycle, the same way `locus_region::RegionKey` is shared today.
- It is UI-agnostic and wasm32-clean, with no new dependencies.

### Decision: C, with this shape

```text
expand(path: &[Piece], closed: bool, base: &BaseStroke, style: &LineStyle,
       brushes: &BrushTable, limits: &Limits) -> Result<Expansion, StrokeError>

Expansion {
  pen_paths: Vec<StyledChain>,   // centrelines a pen or blade follows (dashes, multi-stroke lines)
  areas:     Vec<StyledArea>,    // filled outlines (width, caps, joins, profiles, brushes)
  bounds:    BBox,               // visual bounds for §8 page bounds (#450)
}
```

- **Constant width, caps and joins (#428):**
  - Built on ADR 0010: the stroke outline is the open-chain tube (chain ⊕ disc), which gives round caps and joins.
  - Butt and square caps, and bevel and miter joins, swap the end discs and corner discs for rectangles, triangles and kites, the same exact pieces ADR 0010 already uses for miter joins, before one boolean.
  - Lines and arcs stay true arcs.
  - **Exact.**
- **Dashes (#427):**
  - The path is split by arc length: exactly on lines and arcs, and on the biarcs of cubics.
  - `Corners` scales the pattern per corner-to-corner span, so a dash is centred on every corner and on both path ends.
  - Each dash is a `pen_path`. With width shown, each dash is also expanded with the dash's cap.
  - **Exact.**
- **Multi-strokes (#429):** each `StrokeLayer` is the ADR 0010 open-chain offset of the path, then dashed and widened like the base stroke. **Exact.**
- **Variable width (#426), calligraphic (#430) and paint (#434):**
  - The width is a function w(s) of arc length: the profile, or the nib's projection on the path normal, w(θ) = √((a·sin(θ−α))² + (b·cos(θ−α))²), with a the nib size and b = size × roundness. **Superseded by S2:** that offset is off by up to 1.8 mm. Nib sides are C ± p\*(n), the nib's support point (see *Decisions from S1–S3*, item 5).
  - Each side is sampled adaptively until the chord error is under the tolerance and fitted with cubics. The two sides and the caps then go through one `locus-region` boolean, which removes the loops that tight curves make. The boolean works on lines and arcs only (ADR 0008), so the cubics are lowered to biarcs (`to_pieces`) for it.
  - **Arcs or Béziers (owner question, 2026-11-24): measured by S1. Option (b) is chosen and owner-approved (2026-11-24)** (see *Decisions from S1–S3*, item 3).
    - Option **(a) biarcs only:** one representation everywhere, as today.
    - Option **(b) keep the fitted cubics:** for the canvas, SVG/PDF and **Expand to geometry**, with biarcs only where they're needed (region booleans, DXF R12, lasers and other arc-based machine output).
    - In (b), the boolean cleanup still runs on biarcs. Each cleaned piece then maps back to the cubic span it came from, and spans the cleanup cut are refitted.
    - S1 measured both. The decision is recorded below, after the spike results.
  - Spans where the width is constant use the exact constant-width path.
  - **This is an approximation.** The tolerance is a parameter: `EXPORT_TOL` (0.001 mm) for export and Expand, 0.05 screen px for the canvas.
  - **Spike S1** measures whether that is fast enough (below).
- **Path-mapped art (#431, #433):**
  - A point (x, y) of the artwork maps to C(s(x)) + y·k·N(s(x)), where s maps the artwork's x range onto arc length.
  - **Stretch** maps all of it. **Proportional** keeps the aspect. **Start and end lengths** are protected (mapped rigidly).
  - Art pieces are subdivided, mapped, refitted and lowered to biarcs within the tolerance.
  - Pattern brushes tile spans between corners, where a corner is a turn of more than 15° or a path end. They place corner tiles on the bisector, and fit the span by stretching, adding space or approximating the count.
- **Scatter (#432):**
  - Placement uses a **Locus-owned SplitMix64** seeded by `BrushUse.seed`. There is no `rand` dependency, and the result is deterministic for identical inputs, on every target.
  - Each copy draws size, spacing, offset and rotation from its range in a fixed order, so changing one range doesn't reshuffle the others.
  - A new seed is chosen when the brush is applied. "Reseed" is a command.
- **Bounds (§6.2.1):**
  - `Limits` defaults to **10 000 placements** (dashes, tiles, scattered copies) per path and **200 000 output pieces** per expansion.
  - Over a limit, the result is `StrokeError::Limit { what, count }`. The canvas then draws the plain centreline with a warning badge, the Inspector says why, and export reports it in the pre-export panel (§8) as an error, like the instance limit.
  - Inputs are checked finite and in range at the API boundary.
- **Never panics.** There is no `unwrap` or indexing on untrusted data, and every loop is bounded by the limits. A fuzz test over random styles × random paths (proptest) is part of the first ticket.
- **Caching.** The app caches an `Expansion` per entity, keyed by the hashes of its geometry, style and brush. Export always recomputes. Expansion is pure, so recomputing is safe.

### Spikes (measure, don't guess)

| Spike | Question | Measure | Blocks |
|---|---|---|---|
| **S1 variable-width outline** | Is adaptive sampling + cubic fit + boolean cleanup accurate and fast enough? **And should the outline stay biarcs only, or keep cubic Béziers outside the boolean and arc-based outputs** (owner question, 2026-11-24)? | For (a) biarcs only and (b) cubics kept with biarcs only where needed: Hausdorff error against a dense-sampling oracle (ADR 0010 harness style); segment count; **node count after Expand to geometry** (how editable it is); how the boolean cleanup behaves on line/arc-only booleans (ADR 0008), including the cost and error of mapping back to cubics in (b); time per path and for 500 styled paths, native and wasm32 | #426, #430, #434 |
| **S2 calligraphic accuracy** | Is w(θ) close enough to the exact Minkowski sweep of an elliptical or flat nib? | Maximum deviation against an exact sweep built with ADR 0010 pieces, for roundness 0, 0.3 and 1 | #430 (folded into S1) |
| **S3 canvas cost** | Can the egui painter draw expanded areas within the frame budget? | Frame time with 500 styled paths, cached, with the current tessellator; compare with the wgpu callback upgrade path (ADR 0001) | #426 onward |

If S1 misses the budget (expanding 500 styled paths for the canvas in < 50 ms native, so a style edit doesn't stall), the fallback is a coarser canvas tolerance plus exact expansion only at export and Expand time. That is not a different engine.

### Spike results S1–S3 (2026-11-24, Track A step A0)

Harness: [`spikes/stroke/`](../../spikes/stroke/) (standalone workspace; full tables in its [`results.md`](../../spikes/stroke/results.md)). Built in the `locus-dev` container (Rust 1.99), native x86-64 release, single thread, on the owner's Windows 11 host. wasm32 timings: the same code as a `wasm32-unknown-unknown` module (opt-level 3, fat LTO), timed from JS in the desktop app's Chromium pane. Canvas timings: a real eframe 0.35 / wgpu window on the host, vsync off.

**Oracle (S1).** A width-profile stroke is *defined* as the union of the normal segments [R(s), L(s)] over the path, plus the join sectors and caps. The oracle samples that definition every ≤ 2 µm as thin triangles, and adds the sectors exactly. The error of an outline is measured both ways:
- **outline → truth:** from every boundary sample (flattened at 10 nm), the distance along the outline's normal to where the oracle's membership flips, found by bisection;
- **truth → outline:** a 240 × 240 probe grid. Where the outline and the oracle disagree, the probe's distance to the outline boundary.

**Oracle (S2).** The exact sweep path ⊕ nib: the union, over a ≤ 2 µm polyline of the path, of segment ⊕ ellipse. It is tested exactly in the nib's scaled frame, or with a segment test for a flat nib.

**Pipeline measured.** Each side is fitted with Hermite cubics within T/2. Bands are built per span of the centreline. Each span is cut where the tangent has turned 90°, where a side curve has a cusp, and, for nibs, where the tangent is parallel to the nib. Each band is normalised alone, then one `locus_region` union takes the bands, the join sectors (or nib copies) and the caps. Biarcs are lowered within T/2.
- **(a):** keep the cleaned lines and arcs.
- **(b):** map each cleaned run back to the cubic it came from: whole, or a sub-curve split at the run's ends and snapped to them. A run keeps its arcs when the sub-curve strays more than T/2 from it. That happens where the side is itself an arc, so the arcs are the better answer there.

#### S1: variable-width outline (width profile), T = `EXPORT_TOL` = 0.001 mm

| Fixture | Fitted cubics | (a) pieces | (a) error mm | (b) pieces (cubic/line/arc) | (b) error mm | ms per path |
|---|---|---|---|---|---|---|
| taper line 50 mm, w 0.5→4 | 4 | 34 | 0.00014 | 6 (4/0/2) | 0.00001 | 0.9 |
| S-curve cubic, bulge 0.2→5→0.2 | 85 | 230 | 0.00050 | 87 (85/0/2) | 0.00047 | 4.8 |
| arc r 1.5, 270°, w 4 (inside side flips) | 22 | 20 | 0.00025 | 19 (1/0/18) | 0.00025 | 1.8 |
| zigzag, 30–150° corners, varying w | 20 | 86 | 0.00019 | 26 (20/0/6) | 0.00014 | 1.8 |
| arc spiral r 6→2, w 0.5↔3 | 104 | 157 | 0.00048 | 62 (61/0/1) | 0.00048 | 7.9 |
| closed blob (4 cubics), asymmetric L/R | 65 | 254 | 0.00055 | 65 (65/0/0) | 0.00048 | 7.7 |
| self-looping cubic + cusp cubic | 73 | 272 | 0.00055 | 78 (74/1/3) | 0.00053 | 6.6 |
| **500 random 4-cubic strokes, ~100 mm** | | | **0.00071 (all within T)** | | **0.00054 (all within T)** | 7.8 (a), 11.2 (b) |

**Node count after Expand to geometry** ((b) / (a)), and the error at a looser Expand tolerance:

| Fixture | T = 0.001 | T = 0.01 | T = 0.05 |
|---|---|---|---|
| S-curve, bulge | 87 / 230 | 22 / 88 (err 0.004) | 15 / 52 (err 0.023) |
| arc spiral | 62 / 157 | 39 / 85 (err 0.005) | 28 / 51 (err 0.020) |
| closed blob | 65 / 254 | 35 / 124 (err 0.005) | 24 / 84 (err 0.020) |
| self-looping + cusp | 78 / 272 | 45 / 139 (err 0.004) | 32 / 88 (err 0.021) |

**Speed, 500 random styled paths:**

| Mode | Native | wasm32 |
|---|---|---|
| Canvas: sides sampled at 0.013 mm into a triangle strip, no fit, no cleanup | **16.6 ms** (0.033 ms/path) | **44 ms** |
| Canvas tolerance, full fit + biarcs + union | 2.4 s | 9.0 s |
| Export / Expand (a), T = 0.001 | 3.9 s (7.8 ms/path) | 14.0 s |
| Export / Expand (b), T = 0.001 | 5.6 s (11.2 ms/path) | 22.7 s |

About 18 % of the export time is side fitting. The rest is lowering and the arrangement. Normalising each band costs little: a raw union of the same bands takes 3.6 s against 3.9 s. But a raw union is wrong, because a band that crosses a side cusp can wind 0 over points it covers (a 7 × 60 µm false hole on 1 of the first 100 random paths, before cusp splitting).

**Two `locus-region` bugs found and fixed** (Nerdtisan/Locus#485, with regression tests from this spike, `crates/locus-region/tests/stroke_bands.rs`). Both lost a whole loop of a union, silently:
1. `merge_carriers` merged arcs on circles that differed by up to 10 × SNAP, but kept the first circle, so the merged arc ended up to ~8 µm from the shared vertex. The next boolean saw a gap and dropped the loop. The merged arc now runs through its start, the shared point and its end.
2. Two near-tangent arcs crossing next to a shared vertex leave a ~1 µm arrangement edge. Its side probes sit ≤ 0.1 µm off, it was misclassified, and `trace` dropped the open chain with its loop. Edges shorter than `EXPORT_TOL` are now re-classified, only when the kept edges don't balance at a vertex, to whichever state balances it. Changing one moves the boundary by less than the export tolerance.

Before the fixes, 2 of the first 100 random strokes and 2 of the 12 S2 cases lost their outline.

#### S2: calligraphic nib, size 4 mm at 45°, against the exact sweep

| Path | Roundness | ADR formula w(θ) (normal offset) | **Minkowski support point** | (a) / (b) pieces |
|---|---|---|---|---|
| S-curve cubic | 0 / 0.3 / 1 | 1.78 / 0.25 / 0.0005 mm | **0.00025 / 0.00047 / 0.00047** | 180·192·266 / 28·41·118 |
| arc r 5, 330° | 0 / 0.3 / 1 | 1.43 / 0.39 / 0.0005 | **0.00050 / 0.00048 / 0.00050** | 62·197·39 / 55·128·35 |
| zigzag (corners) | 0 / 0.3 / 1 | 1.69 / 0.25 / 0.0000 | **0.00000 / 0.00027 / 0.00001** | 6·113·10 / 6·113·10 |
| looping cubic | 0 / 0.3 / 1 | 0.63 / 0.35 / 0.0005 | **0.00019\* / 0.00047 / 0.00047** | 122·206·103 / 23·108·29 |

\* The worst point is a zero-width spur where the path starts tangent to a flat nib. It has no area to flip across, so its distance to the true sweep is bounded by growing the nib test until it touches.

The w(θ) formula in this ADR offsets along the normal by the nib's support h(n). That is the right *width* but the wrong *point*: the sweep's boundary is C + p\*(n), the nib point farthest along n, which sits off the normal. The error is 0.25–0.39 mm for an oval nib and up to 1.8 mm for a flat one, because the formula can't make the straight nib edges at all.

#### S3: canvas draw cost, 500 styled paths (117 451 triangles), Windows host, eframe 0.35 + wgpu

| Mode | ms/frame wall | ms/frame CPU building shapes |
|---|---|---|
| Baseline: today's plain centrelines (`painter.line`) | 7.60 | 0.22 |
| **Strip meshes built once in world space, `Arc<Mesh>` under a layer transform** | **7.53** | **0.03** |
| Strip meshes transformed to screen every frame | 8.11 | 3.06 |
| Cleaned outlines filled per screen row (today's region-fill method) | 45.7 | 36.3 |

7.5 ms is this machine's present floor with vsync off, so 117 k cached triangles are free on the GPU. The wgpu paint-callback upgrade path isn't needed.

### Decisions from S1–S3

1. **Canvas:**
   - The canvas does **not** run the exact engine. It samples each side at the canvas tolerance (0.05 px) into a triangle strip, with fans for joins and caps. There is no fit and no cleanup.
   - The strip is cached per entity in world space. The key is the hashes of the geometry, the style and the brush, plus a zoom bucket, because the tolerance is in screen pixels.
   - It is drawn as `Arc<Mesh>` under a layer transform, so a pan or zoom inside the bucket rebuilds nothing.
   - This is the fallback the ADR already allowed, and it meets the 50 ms budget with room to spare: 16.6 ms native and 44 ms wasm for 500 paths.
   - **Known limit:** strips overlap at tight inner corners and swallowtails, so a stroke with opacity < 1 looks slightly darker there. The cleaned outline is still used for hit-testing, export and Machine preview.
2. **Export, Expand and Machine preview:**
   - These use the exact pipeline above. It is within T on every fixture and on all 500 random strokes.
   - It takes about 8–11 ms per variable-width stroke natively and about 3.6× that in wasm. It runs off the UI thread on the desktop and in chunks on the web, as auto trace does.
   - Machine preview uses the per-entity cache.
   - Constant-width spans keep the exact ADR 0010 path, as decided. This cost applies only to profiles, nibs and paint.
3. **Outline representation: option (b).**
   - Fitted cubics are kept for the canvas, SVG/PDF and Expand. Biarcs are used only for the cleanup boolean and for arc-based output (DXF R12, lasers, region booleans), as this ADR's option (b) describes.
   - It has the same accuracy as (a) on every case (within T, both ways).
   - It gives **2.5–4× fewer nodes** after Expand, and 1.1–4× fewer at looser tolerances.
   - Map-back costs about 40 % more time.
   - Runs whose side is a true arc keep their arcs automatically.
   - **Owner-approved, 2026-11-24.**
4. **Expand tolerance (owner-approved, 2026-11-24):**
   - At T = 0.001 mm a 50 mm bulging stroke expands to 87 cubic nodes, which is exact but tedious to edit.
   - Proposal: **Expand to geometry** gets a tolerance setting, default **0.01 mm**. That gives 22–45 nodes, with a measured error of 0.004–0.005 mm, still under any pen, kerf or blade width.
   - Export always uses T.
5. **Calligraphic model replaced (spec of #430 changes; relayed to the owner 2026-11-24, no objection):**
   - The side curves are **C ± p\*(n)**, the nib's support point, not C ± n·w(θ).
   - The nib itself is placed at every corner and at both ends. An exact circle is used for a round nib, and a fitted ellipse otherwise.
   - Spans are cut where the tangent is parallel to the nib. There a flat nib's support point jumps, and each side is an exact translate of the path (lines, arcs and cubics stay as they are).
   - This is within T for roundness 0, 0.3 and 1.
6. **Band rules (for the engine):**
   - Split each centreline piece at 90° of turn, at side cusps and at nib flips.
   - Normalise each band alone, then run one union.
   - Derivatives are clamped to the band, so a jump is never differenced across.
7. **Rejected:**
   - A least-squares (Schneider) tangent-length fit gave no fewer cubics at T (87 against 85). Sampled checks can also miss its overshoot: 1.8 mm in one case. Hermite fitting stays.
   - Scanline-filling the cleaned outline on the canvas costs 36 ms of CPU per frame for 500 strokes.

## Decision 4: how each style reaches each export format and machine operation

**Rule:**

- **Machine layers** (Cut, Score, Engrave, Drag knife) follow a new per-layer operation parameter, **Stroke styles: Centreline | Styled**. Centreline is the default, and stays today's behaviour.
  - **Centreline** cuts or marks the real path, as today. Styles are visual only.
  - **Styled** uses the expansion. On Cut, Score and Drag knife, `pen_paths` become separate cuts: dashes become perforations, and multi-strokes become parallel cuts. `areas` become cut outlines. On Engrave, `areas` are hatched like any closed shape. On every machine layer, colour and hairline still come from the profile (M5 rule).
  - **Owner decision 2026-11-25 (option A, refined; #502/#504 follow-up):**
    - Only objects with a **line style** follow their strokes on a Styled layer: dashes, multi-strokes, and later width profiles and brushes (`LineStyle::machine_styled`). Plain objects follow their centreline.
    - **Width alone doesn't count**, and neither do caps, joins or alignment. A widened plain line still cuts once along its centreline.
    - Machine preview draws the paths that follow styled strokes with a **soft halo**, and the legend says so.
    - Option C, a per-object override, is filed for later and is not built.
- **Draw** layers follow the document's machine kind (#419):
  - A **pen plotter** gets machine geometry: each dash is its own pen-down path, each multi-stroke is a path with its own pen, and filled outlines are plotted as **outline + hatch** at the object's pen width, so they read as solid. A Draw-layer setting, **Filled strokes: Outline + hatch | Outline | Hatch**, overrides this.
  - **Generic, laser or vinyl** profiles, and no profile, get the visual output below.
- **Expand to geometry** (`ExpandStyle`, one undo step):
  - It replaces the object's style with plain curves on the same layer: outlines as closed chains, or, with **Centrelines**, the pen paths. They keep the object's colour and pen.
  - Expanded gradients become stepped bands (N = 8 by default) or the chosen hatch.
  - Expanding is allowed on recipe instances only through Explode, as ADR 0011 already requires.

| Style | SVG / PDF (visual) | PNG | DXF R12 / HPGL | Plotter (Draw) | Cut / Score / Drag knife | Engrave |
|---|---|---|---|---|---|---|
| Width, caps, joins (#428) | native `stroke-width`, `stroke-linecap`, `stroke-linejoin`, `stroke-miterlimit` | `tiny-skia` stroke | outline geometry | outline + hatch, or the centreline when the width ≤ the pen | Centreline: path · Styled: outline | Centreline: path · Styled: hatched outline |
| Dashes (#427) | `stroke-dasharray` / PDF dash for `Exact`; `Corners` writes expanded dashes | `tiny-skia` dash or expanded | one entity per dash | one pen-down per dash | Styled: perforation | Styled: dashed engrave lines |
| Multi-strokes (#429) | one `<path>` per layer | per layer | offset paths | one path per layer, own pen | Styled: parallel cuts | Styled: parallel lines |
| Width profile, calligraphic, paint (#426, #430, #434) | filled outline path (no native variable width) | filled outline | outline | outline + hatch | Styled: outline | Styled: hatched outline |
| Art, pattern, scatter brushes (#431–#433) | the mapped artwork (fills and strokes as defined) | same | outline geometry | artwork paths; closed art hatched | Styled: artwork outlines | Styled: hatched artwork |
| Gradient fill (#436) | `linearGradient` / `radialGradient` (elliptical via `gradientTransform`); PDF axial or radial shading; **conic** as 64 wedge bands (no native conic in SVG 1.1 or PDF types 2/3) | rasterised | none, or the hatch when chosen | variable-density hatch when chosen, else none | ignored | variable-density hatch when chosen; **Power** only where the profile declares greyscale support (none of the built-in presets do yet, so the option shows disabled with that reason) |
| Layer blend and opacity (#459) | SVG `mix-blend-mode` and `opacity` on the layer `<g>`; PDF ExtGState `/BM` and `/CA` | `tiny-skia` blend mode | ignored | ignored | ignored | ignored |
| Knockout and interacting regions (#459) | changes the fills drawn | same | n/a | changes hatch areas | n/a | **changes hatch areas, shown in Machine preview** |

**Page bounds (#450).** The visual bounds use `Expansion.bounds`, so caps, miters, profiles and brushes are never clipped. §8's "Cap and join extents follow when M6 adds caps and joins" is met by this.

**Machine preview.** It draws the same `prepare_for` result, so Styled layers show perforations, outlines and hatch exactly as cut. The legend gains a "styled" note per layer.

### As built: gradients, part 1 (#436, 2026-11-24)

- **Model:** `RegionFill.gradient` (format v28): Linear, Radial, Elliptical (ratio) or Conic; ends and up to 32 stops in the region's **unit frame**, the region's box fitted along `frame_angle`. The canvas, PNG and the bands all sample `locus_doc::gradient::paint_at`.
- **Rotate with shape:** a Rotate whose selection holds every boundary of a region adds the angle to that fill's `frame_angle` in the same undo step. Off, the frame stays axis-aligned. Moves and scales follow anyway, because the frame is refitted to the region. Mirror makes copies, so `frame_flip` is stored but no command sets it yet.
- **Export:** SVG writes native `linearGradient` / `radialGradient` in `userSpaceOnUse` with the frame as `gradientTransform`; conic is 64 clipped wedge polygons. PNG is per pixel. PDF writes native smooth shading clipped to the region (#513): axial (Type 2) for linear, radial (Type 3) for radial and elliptical, in the region's frame. More than two stops become a stitching (Type 3) function, and stop opacity is a luminosity soft mask. Conic is 64 clipped bands. DXF/HPGL and machine layers ignore gradients, as the table says.
- **Part 2** (still open on #436): on-canvas gradient handles, the machine hatch and Power results, presets and the library, Expand to bands, and "—" for mixed selections in the Inspector.

## Decision 5: history and timeline (#446) on top of the existing undo stack

### Options

**A. A branching history tree, persisted in `.locus`.** This needs commands to be replayable from the file, which they aren't (patches hold whole entities), and it conflicts with §1.

**B. A linear history panel over the session undo stack, and a timeline that is a view of the recipes (chosen).**

### Decision: B

- **History panel:**
  - `Editor` exposes `history()`: the undo and redo labels plus a cursor.
  - Clicking an entry undoes or redoes to it as **one batch with one re-solve**, instead of N solves.
  - Doing something new after jumping back **truncates** the redo entries, as today and as Photoshop's default. Branching is deferred, and needs an owner request.
  - The history is session-only, as now, and capped at **1 000 steps** (a setting; oldest dropped). Today's stack is unbounded, and the spike for #446 measures the memory per step on a 500-entity document before the cap is fixed.
- **Timeline:**
  - It lists the document's features in `RecipeId` order: recipes (mirror, patterns, offset, booleans, plugin objects), text-on-path bindings and brushed styles.
  - **Edit** opens the feature's existing dialog (`EditRecipe`), and everything downstream regenerates after the solve, as ADR 0011 already does.
  - **Suppress** (`SetRecipeSuppressed`, format) hides a feature's instances and keeps its IDs.
  - **Delete** is `RemoveRecipe`, refused with the reason while something depends on it.
  - **Reorder** is offered only between features that don't depend on each other. It changes their display and regeneration order, and when a dependency exists it is refused with the dependency named. In a single solved sketch, reordering never changes geometry, so this is honest about what it can do.
  - **Failures:** a recipe whose regeneration fails (an offset that collapses, a limit hit, a plugin error) keeps its last good instances, shows red with the error in the timeline and the Inspector, and never stops the solve.

### As built (#446, 2026-11-24)

- **Memory per step, measured** (`command::tests::memory_per_step`, release build):
  - One patch is 536 B. Points and lines own no heap memory, so that is their whole cost.
  - On a 500-entity sketch (250 points, 250 lines), a step that moves every point costs **134 KB**, and a one-object edit costs **0.5 KB**.
  - So **1 000 steps cost under 1 MB for ordinary editing, and 134 MB at most** if every step moved everything.
  - The cap stays at 1 000. Boxing the large `Patch` variants would cut the worst case if it ever matters.
- **Suppress:** `SetRecipeSuppressed` and `Recipe.feature_suppressed` (format v24; the existing `suppressed` field is single pattern copies). Copies go and their IDs stay.
  - **Mirrors:** their copies and owned Symmetric/Equal constraints are removed and come back under the same IDs, reflected from the current sources.
  - **Refused**, with the reason, when a user constraint or another feature uses the copies.
- **Failures:**
  - A feature whose regeneration fails keeps its last good copies. The command that changed its input still applies, and `Editor::recipe_failures()` gives the reason, which the Timeline shows in red.
  - Integrity guards (`EntityInUse`, `RecipeInUse`) still refuse the command.
  - A pattern whose count stops evaluating is now reported, where it used to be skipped silently.
- **Reorder:** `ReorderRecipe { id, to }` and `Document.recipe_order` (format v26, omitted in creation order).
  - Regeneration follows the timeline order.
  - A move that would put a feature before one whose copies it uses is refused, naming both.
  - The Timeline offers Move earlier / Move later. Dragging is not built.

## Decision 6: layer blending and interaction vs regions per layer (#457)

Owner clarification on #459 (2026-11-19): an **Interact / Don't interact** switch (default Don't interact), plus per-layer blending and opacity, plus opt-in links between chosen layers, including cut-by and knockout. All of it is in scope.

### Options

**A. Per-document switch only.** It is simple, but it can't express "this fill layer is cut by the linework layer, nothing else".

**B. Per-layer links only.** This is flexible, but the common "all layers together, as before #457" needs N² ticks.

**C. Both (chosen).**
- A document switch, **Layers interact**, makes every layer cut every other layer, which was the pre-#457 behaviour.
- When it's off (the default), each layer can list layers that **cut** it and layers that **knock out** its fills.

### Decision: C

- **Region detection** for layer L uses L's curves plus the curves of every layer in `cut_by(L)`. The regions still belong to L, and their fills are L's.
- Region keys still name entities, so keys stay unique (§6.3). A key may now name an entity on another layer.
- **Knockout** subtracts the union of the closed areas on `knockout_by(L)` from L's filled regions, with one `locus-region` boolean.
- **One resolver** still serves the canvas, the Inspector and export (#457), so Engrave hatch follows knockouts and cuts.
- Links are **directional**. "A is cut by B" doesn't make B cut by A, and the UI writes both directions when the user picks "Interact with".
- Links to a deleted layer are dropped in the same undo step, and the layer-delete warning lists them (§4 rule 4).
- A link can't point to the layer itself, and **cycles are allowed** because cut-by only reads curves, never derived regions.
- **Blending and opacity are visual only.** They never change machine paths (§7). Machine preview ignores blend modes, but shows knockouts and cut-by because they change hatching.

## Decision 7: OS font enumeration (#477)

### Options

**A. `font-kit`.** One crate for all three OSes, but its Linux path pulls FreeType and it is more than we need.

**B. `fontdb`.** It only scans folders, so it can't see Creative Cloud's session-registered fonts. That is the bug.

**C. Per-OS bindings in `locus-app` behind `SystemFontSource` (chosen, pending spike S4).**
- **Windows:** DirectWrite `GetSystemFontCollection` through the `windows` crate. `windows` 0.62 is already in `Cargo.lock` through the GUI stack; this needs only its `Win32_Graphics_DirectWrite` feature.
- **macOS:** `CTFontManagerCopyAvailableFontURLs` and `CTFontCollectionCreateFromAvailableFonts` through `objc2-core-text`, from the objc2 family already in the tree.
- **Linux:** fontconfig through `yeslogic-fontconfig-sys` with `dlopen` (MIT), falling back to today's folder scan.
- **Web:** unchanged.

### Decision: C

- The results are merged with the folder scan, de-duplicated by family and face name, with the OS's entry winning.
- Font bytes are read from the OS's path or stream under the 64 MB cap.
- Paths are never logged, and Adobe files are never copied or embedded.
- `SystemFontSource` grows to list faces (family, style, weight), not only families.

**Spike S4:** check on Windows and macOS with Creative Cloud running that Adobe fonts are listed and load; measure enumeration time with about 1 000 installed fonts (target < 300 ms at startup, done off the UI thread); run `cargo deny` on the exact features. The added dependencies are chosen in the #477 PR, not here (no new dependencies in this pass).

**Spike S4 results (2026-11-24, #477).** Measured on the owner's Windows 11 PC with Creative Cloud running and 33 Adobe font files activated, using a throwaway probe over DirectWrite (`windows` 0.62):

| Measure | Result |
|---|---|
| System collection | 226 families, 531 real (not simulated) faces; created in 0.8–1.3 ms |
| Family and face names for every face | 25 ms cold, 100 ms warm run (both under the 300 ms target, and off the UI thread) |
| Adobe fonts listed | **22 families, 32 faces**: every activated Adobe font whose file DirectWrite reports under Creative Cloud's `livetype` folder |
| Adobe fonts loaded | **32 of 32** read through DirectWrite's font-file stream as valid sfnt data (7.5 MB) in 5–8 ms |
| Each face's file path as well (for de-duplicating against the folder scan) | another 240–420 ms: **not used** |

What #477 does as a result:

- **Bindings: `dwrote` 0.11 (MPL-2.0) on Windows, `core-text` 22 (MIT OR Apache-2.0) on macOS, and `fc-list` on Linux.** This differs from the crates named in option C. The workspace denies `unsafe_code`, and has no exception anywhere. Raw `windows` or `objc2-core-text` calls are `unsafe`, while these two crates are safe wrappers over the same DirectWrite and Core Text calls. `fc-list` is fontconfig's own command, so no FFI crate is needed. Without fontconfig, the folder scan is used. All licences are on the `deny.toml` allow list. `dwrote` brings `winapi` 0.3 (Windows only).
- **Names:** a face is listed as its family name for the regular face, and as "Family Style" for the others. That is what a document stores.
- **De-duplication:** no file paths are fetched. On Windows and macOS the folders the scan walks are the OS's own, so the OS list already covers them. The OS list is shown whenever it isn't empty, and the first face under a name wins. The folder scan is listed only where the OS lists nothing.
- **Older documents:** the folder scan always stays loadable by file stem, so documents saved before #477 (which store "arial") keep their fonts.
- **Bytes:**
  - Windows reads through DirectWrite's stream, so no path is needed.
  - macOS reads the file at Core Text's URL, and finds the face in a `.ttc` by its names (`locus_text::face_index`).
  - Linux uses fontconfig's index.
  - All under the 64 MB cap. Nothing is copied or embedded, and no path is logged.
- **Not measured:** the macOS check with Creative Cloud (no Mac here). CI builds and tests macOS, and the manual check is listed in #477's test notes.

## Decision 8: auto trace (#461)

### Options

**A. `vtracer` / `visioncortex`.**
- Pure Rust. Licence MIT OR Apache-2.0, to be confirmed by `cargo deny` in the spike.
- It already does colour clustering and spline outlines, and runs in wasm (its demo is a web app).
- It has **no centreline trace**, which plotters and engraving need, and its output is splines, not lines and arcs.

**B. Potrace (C library or bindings).**
- The reference quality, but GPL-2.0-or-later is not on the allow list, and C doesn't build for wasm32. **Rejected.**

**C. Locus-owned, in a new core crate `locus-trace`.**
- Threshold (with Otsu's method), Sobel/Canny edges, k-means colour quantisation, marching-squares contours and a Potrace-style polygon and curve fit implemented **from the published paper** (Selinger, 2003), not the code.
- Zhang–Suen thinning plus graph walking for centrelines.
- Lines, arcs and cubics fitted to the tolerance, then biarcs.
- Wasm32-clean, with no new dependencies.

**D. Hybrid.** A for colour outlines, C for centreline.

### Decision: C (owner decision, 2026-11-24)

Locus writes its own tracer in `locus-trace`, implemented from the published Potrace algorithm (Selinger, 2003) and the other published methods above. No Potrace code is used and no licence exception is needed. C is the only option with centreline output, it keeps line and arc output native, and it adds no dependency. A and D are not chosen.

**Spike S5 becomes a benchmark,** not a choice. It runs on four fixtures: a logo, scanned line art, a photo and a handwritten signature. It measures node count, maximum deviation from the source edge in pixels, time for a 4 MP image native and wasm, and wasm size. These set the default tolerances and the step budget.

**Spike S5 results (2026-11-24, #461).** Measured on `locus-trace` as built for #461 (threshold with Otsu's level, speckles ≤ 4 px, `alpha_max` 1.0, centreline tolerance 0.5 px). The four fixtures are 2000 × 2000 (4 MP), drawn procedurally in the crate's tests so the repo carries no images: a logo (ring, bar, triangle), scanned line art (12 thin rings and two diagonals, with 3 % grain), a photo-like shaded field with noise, and a handwritten-style signature. Native timings are `--release` in the dev container. Wasm timings are a release `wasm32-unknown-unknown` build run in the browser pane, with fixture building subtracted. The probe module is 123 KB including its fixture code.

| Fixture | Mode | Pieces | Max deviation | Native | Wasm | Steps |
|---|---|---|---|---|---|---|
| Logo | Threshold | 138 | 1.06 px | 48 ms | 44 ms | 14 M |
| Logo | Centreline | 739 | n/a | 0.9 s | 0.29 s | 60 M |
| Line art | Threshold | 5 540 | 2.13 px | 80 ms | 74 ms | 33 M |
| Line art | Centreline | 10 413 | n/a | 184 ms | 90 ms | 17 M |
| Photo | Threshold | 167 958 | 3.35 px | 280 ms | 301 ms | 329 M |
| Photo | Centreline | 19 750 | n/a | 13.3 s | 4.8 s | 616 M |
| Signature | Threshold | 117 | 1.03 px | 53 ms | 20 ms | 12 M |
| Signature | Centreline | 423 | n/a | 55 ms | 19 ms | 12 M |

"Max deviation" is the largest distance from the source's pixel boundary (every 4th lattice point) to the traced curves. About 1 px is the corner-rounding the method intends, at the default `alpha_max`. Native numbers vary with the container's load (the centreline rows ran while other builds did), so treat wasm and native as the same order.

What S5 sets:

- **Defaults:**
  - speckles ≤ 4 px
  - Corners `alpha_max` 1.0
  - centreline tolerance 0.5 px
  - **step budget 1 000 M**, which covers every fixture, including a centreline of a noisy photo
- **The dialog needs:**
  - a progress line and Cancel: a noisy 4 MP photo's centreline takes seconds
  - a warning: a photo in Threshold mode makes about 168 k pieces, close to the 200 000 entity limit, so "raise speckles or use Colours"
- **Not yet done:**
  - lines and arcs as an output option (the dialog's "Curve fit")
- **Curve optimisation, as built (#505, 2026-11-25):** the dialog's **Simplify** (px, default 0.2 as Potrace's `opttolerance`, 0 = off) joins runs of smooth curves that turn one way through under 179° into one cubic. The cubic keeps the run's end tangents, has its handle lengths fitted by least squares, and is kept only if every sampled point of the run is within the tolerance. It is greedy rather than the paper's optimal search, and corners stay. On the owner's 12 500-entity test trace (1 612 lines, 2 311 curves with it off), it gives 2 241 curves at 0.2 px, 1 726 at 0.5 px and 1 444 at 1 px. Trace time is unchanged at about 65 ms.

Also:
- Input is the decoded reference, downscaled to ≤ 4 MP for tracing.
- Work is bounded by a step budget, not a clock (§3), and cancellable through a callback.
- On the desktop the trace runs off the UI thread. On the web it runs in chunks.
- The output is in mm at the reference's calibrated scale (#391), on the chosen layer, as **one `TraceImage` command**. Counts are bounded by the §6.2.1 entity limit.

## Decision 9: smaller tickets

- **#462 drag-reorder:**
  - `ReorderLayer { layer, to }` rewrites every layer's `export_order` as one step. The canvas draw order and the export order both follow it (#291).
  - Keyboard: Alt+↑ and Alt+↓ on a focused layer row.
- **#463 reference handles:**
  - **Owner clarification, 2026-11-24:** references use the **same Move / Rotate / Scale tools and manipulator as shapes** (#219), not a separate handle set, so placement is just as precise.
  - Values can be typed while dragging, as in Fusion's Move/Copy:
    - **Move:** X / Y distance, snapping to points and the grid.
    - **Rotate:** an angle, about the centre or a picked point.
    - **Scale:** a factor, about Centre, Anchor corner or a picked point (the Calibrate choice, #464/#468).
  - The command dialog docks top right: Tab between fields, Enter commits, Esc cancels (ADR 0017 tool model).
  - The Inspector also shows X / Y, angle and scale.
  - The tools dispatch on the selection. A reference selection commits one `TransformReference { layer, transform }` (generalising `ScaleReference`) instead of moving entities, because the Reference layer stays locked and only these tools may move it.
  - Images gain `ImagePlacement.rotation` (format). Vector references transform their entities inside the same command.
- **#470 Bézier handles:**
  - Theme constants `HANDLE_PX = 9` and `HANDLE_HIT_PX = 16`, a 1.5 px outline in the canvas colour, and a hover ring.
  - Anchors share the same size.
  - This is purely in `locus-app`.

### Built in #428 (2026-11-24)

- `locus-stroke` holds `LineStyle { cap, join, align }`, the exact constant-width expansion (bands, discs, kites, bevel triangles and square caps, one union), the canvas mesh, analytic bounds and end-to-end chaining. A proptest fuzz runs random paths × random styles.
- **`Style` stays `Copy` for now.** `LineStyle` is three small `Copy` fields, so `Style.line` is a plain field, not `Option<Arc<LineStyle>>`. It becomes the `Arc` when the first heap field arrives (dashes, #427), as this ADR planned. That avoids churn in this ticket.
- **Stroke styles** is a `Layer` field, not an operation parameter, because `Operation::Score` has no parameter struct.
- **DXF / HPGL on Draw layers keep the centreline (flagged).** Decision 4's table says "outline geometry". This ticket keeps the plotter rule, "a centreline when the width ≤ the pen": on a Draw layer the object's width *is* its pen. Writing every Draw path's outline would change every existing DXF. This is open for the owner.
- **Expand to geometry** writes outlines only. The design's dialog, with Outline or Centrelines, Keep original and a preview count, follows with dashes (#427), when Centrelines first differ from the path.

### Built in #427 (2026-11-24)

- **The dash pattern is a fixed-size array** of up to 8 pairs, stored in `.locus` as `pattern: [..]`. So `LineStyle` and `Style` stay `Copy`, and a hostile file can't make a style grow. This replaces the `Arc<LineStyle>` the decision planned. The cost is a bigger `Command` (an app enum gained a `large_enum_variant` allow).
- `locus_stroke::dashes` splits by arc length, exactly on lines and arcs. **Corners** uses turns over 15°, a whole number of repeats per span, and half a dash at each span end, merged across corners. Placements are bounded at 10 000 per path (§6.2.1).
- **Expand** now has the **Centrelines** mode (`ExpandStyle.mode`). The app offers it as a second action, **Expand to centrelines**, instead of the design's dialog with Keep original and a preview count. The dialog stays open for a later pass.

### Built in #429 (2026-11-25)

- **`Style` and `LineStyle` are no longer `Copy`**, as decision 1 planned: the extra-stroke list lives on the heap. The change across crates was mechanical: clones where a style is copied out of a reference. `StrokeLayer` itself stays `Copy`.
- `locus_stroke::layer_strokes` gives every stroke of an object: index 0 is its own, then each extra at its exact ADR 0010 offset. Open chains use `offset_open` with round joins; closed ones offset their area, with + to the left. Expand, the canvas mesh, page bounds and export all use it.
- **Export:** an `ExportPath` can carry its own `Paint`. Extra strokes and split dashes are then plain paths in every writer, with no per-layer bookkeeping.
- The Inspector reorders rows with **Up** and **Down** buttons rather than the design's drag grip, which is left for a polish pass.

### Built in #426 (2026-11-25)

- **Model:** `LineStyle.profile: Option<WidthProfile>` (format v30): 2–64 `WidthPoint { t, left, right }`, the sides as factors of half the width (0–20), eased with a smoothstep between points. A uniform profile is stored as none. Presets: Uniform, Taper both ends, Taper start, Taper end, Bulge, Pinch, and Pressure, a smooth random profile from a seed (SplitMix64, kept in `WidthProfile.seed` so the Inspector can offer Reseed).
- **Engine (`locus_stroke::vary`):** the S1 pipeline as measured. Each side is fitted with Hermite cubics within T/2 per band; bands are cut at 90° of arc turn and at side cusps (where an arc's inner reach crosses its radius). Each band is normalised alone, then one union takes the bands, join pieces (sector, bevel or miter at the outer side's reach) and caps (a quarter disc per side, so each side keeps its reach; Square extends by the mean reach). `expand` returns the cleaned lines and arcs (machine output, booleans). The new `outline` maps runs back to the fitted cubics (option (b)) for SVG/PDF/PNG and **Expand to geometry**, which now writes Bézier entities.
- **The profile reads the whole path:** curves joined into one chain share one profile by arc length. A dashed profile gives each dash its own part of the profile, so a taper runs across the dashes. Extra strokes keep a uniform width.
- **Alignment:** a profiled stroke is always centred. Inside/Outside apply to uniform strokes only.
- **Styled machine layers:** a profile counts as a line style (`machine_styled`), so a Styled layer cuts its cleaned outline.
- **Not built:**
  - **Pen-plotter outline + hatch.** Draw layers write the filled outline (SVG/PDF/PNG). DXF and HPGL on Draw layers still write the centreline, as for #428. This joins the open owner question there.
  - The Inspector graph is read-only. Points are edited on the canvas with the Width tool. The design's docked dialog (editable graph, Save as preset) is left for the Brushes library (#435).
  - A closed path's t = 0 and t = 1 are not forced equal. Its presets taper at the seam.

## Consequences

- **New core crate `locus-stroke`**, and `locus-trace` (#461). Both are UI-agnostic and added to the CI wasm32 check list; editing CI is out of this pass, so the first ticket that adds a crate asks for the check list change.
- `locus-doc` depends on `locus-stroke`, and `locus-export` and `locus-app` use it.
- `Style` stops being `Copy`. Expect mechanical edits in `locus-app` and `locus-export` in the first style ticket.
- §0 records the decisions as proposed. §12 gains item 17 pointing here. §4, §6.3, §7, §8 and §10 are edited by each feature PR as it lands, not now.
- **Owner decisions, 2026-11-24:**
  - §1 is reworded (conflict 1).
  - Gradient fills get **Rotate with shape**, default on (decision 1).
  - Auto trace is Locus's own implementation from the Potrace paper (decision 8).
  - Arcs vs Béziers for variable-width outlines: S1 measured it; (b), kept cubics, is owner-approved (decision 3). **Expand to geometry** gets a tolerance setting, default 0.01 mm, owner-approved; export always uses `EXPORT_TOL`.
- **Defaults still standing unless the owner says otherwise:**
  - **Stroke styles = Centreline** on machine layers.
  - Gradients on regions only.
  - Linear history (truncate) rather than branching.
  - Caps and joins = Round, which matches today's canvas.
- **Revisit if:**
  - S1 shows the variable-width approximation can't meet `EXPORT_TOL` at a usable speed. Then expand to a coarser export tolerance and say so in the UI.
  - S3 shows the canvas can't draw expanded strokes within budget. Then move to the wgpu paint-callback renderer, ADR 0001's upgrade path.
