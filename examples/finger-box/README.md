# Finger-joint box

A first-party example **generator** plugin for Locus (SPEC §13 M3, §15.2).

Lays out the flat panels of a box with finger joints, in a row: bottom, front, back, left, right and, optionally, a lid. Edges that meet get complementary finger patterns with the same number of fingers, so they interlock. Sizes are outer dimensions.

| Parameter | Unit | Range | Default |
|---|---|---|---|
| `width` | mm | 20–2000 | 100 |
| `depth` | mm | 20–2000 | 80 |
| `height` | mm | 20–2000 | 60 |
| `thickness` | mm | 0.5–25 | 3 |
| `finger` (target finger width) | mm | 2–200 | 10 |
| `lid` | yes/no | | no |

Build and pack it like the template (`examples/plugin-template/README.md`). MIT OR Apache-2.0.
