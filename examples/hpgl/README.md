# HPGL exporter and importer

A first-party example **exporter** and **importer** plugin for Locus (SPEC §13 M3, §15.2).

Exports HP-GL (`.plt`) for pen plotters and vinyl cutters:

- one pen per layer: a Draw layer's pen number, otherwise pens 1–8 in layer order;
- lines as `PA`, true arcs and circles as `AA` (no flattening);
- plotter units of 0.025 mm, y up (as in Locus).

Imports `.plt`, `.hpgl` and `.hgl` files as reference geometry on a new locked layer: `PU`, `PD`, `PA`, `PR`, `AA` and `CI` are read; labels, line types and other instructions are skipped. Each pen-down run becomes one path.

It only turns the validated export model into bytes, and bytes into paths; Locus does the file dialogs, reads and writes. Build and pack it like the template (`examples/plugin-template/README.md`). MIT OR Apache-2.0.
