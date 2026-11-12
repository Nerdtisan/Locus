# ADR-0014: Stroke-edge dimensions

- **Status:** Accepted
- **Date:** 2026-11-12
- **Spec:** `docs/SPEC.md §5` (Stroke-edge constraint reference), `§7` (operations), `§10` (format v12)
- **Issues:** Nerdtisan/Locus#239 (model + solver), #240 (UI + preview)

## Context

§5 wants dimensional constraints to optionally reference the edge of a pen line or laser cut rather than the zero-width centreline: a Draw layer's stroke is its pen width wide, a Cut layer's is its kerf (§7). Changing the pen width or kerf must re-solve.

## Decision

* `Dimension` gains `edge: Edge` (`Centre` default, `Inside`, `Outside`), serialised only when not `Centre` (so v11 files load unchanged; format **v12** records the change, the migration is the identity).
* The dimension's `value` is measured **to the edge**; the solver is given the **centreline** value: `value + Document::edge_offset(c)`. The offset is `−half` for `Outside` and `+half` for `Inside`, where `half` is the sum of the half stroke widths of the operands that have a stroke:
  * `Distance` line–line and point–line: lines contribute `width / 2` each (a point has no stroke). `Outside` is the extent across the strokes, `Inside` the gap between them.
  * `Radius`: `width / 2`; `Diameter`: `width`. `Outside` is the circle's larger edge, `Inside` the smaller.
* Not available (refused on add / edit): point-to-point `Distance`, horizontal / vertical distance and `Angle`, which have no edge to measure to.
* Stroke width of an entity = its layer's pen width (Draw) or kerf (Cut); every other operation has none, so there every edge equals the centreline. Operands on different layers each use their own layer.
* **Re-solve.** `Sketcher::apply` already re-solves after every command, so `SetLayerProps` (pen width / kerf) updates the edge dimensions with no new machinery. If a change would need a negative centreline distance or a non-positive radius, lowering fails with `DocError::Invalid` and the command rolls back whole (nothing is dropped silently).
* **Driven dimensions** report the measurement in edge terms: the solver's centreline measurement minus the offset.
* One command, `SetDimensionEdge { id, edge }`, is one undo step and keeps the value.

## Consequences

* No new solver constraint kinds; the change is confined to lowering (`locus-doc::solve`) and the driven-value read-back.
* Cut-layer compensation (`inside` / `outside`, §7) shifts the *exported* path, not the sketch, so it does not enter these offsets; they use the kerf width only.
* Pattern and offset instances are not solver variables (ADR 0011) and cannot carry constraints, edge or not.

## Measured

`crates/locus-doc/tests/stroke_edge.rs`: edge-to-edge distances are exact to 1e-6 mm after a kerf change, for lines on one or two layers, circles (radius and diameter) and driven dimensions; impossible requests are refused with the history untouched.
