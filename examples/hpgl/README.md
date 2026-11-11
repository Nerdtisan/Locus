# HPGL exporter

A first-party example **exporter** plugin for Locus (SPEC §13 M3, §15.2).

Writes HP-GL (`.plt`) for pen plotters and vinyl cutters:

- one pen per layer: a Draw layer's pen number, otherwise pens 1–8 in layer order;
- lines as `PA`, true arcs and circles as `AA` (no flattening);
- plotter units of 0.025 mm, y up (as in Locus).

It only turns the validated export model into bytes; Locus does the file dialog and the write. Build and pack it like the template (`examples/plugin-template/README.md`). MIT OR Apache-2.0.
