# Kerf check

A first-party example **validator** plugin for Locus (SPEC §15.2, §8).

Before export it checks every Cut layer against that layer's kerf width:

- **error:** a circle or arc with a radius under half the kerf. The cut is wider than the hole, so the hole disappears.
- **warning:** a closed shape whose narrower side is under *min-feature* × the kerf (default 3). Parts that small tend to burn away or fall out.

Its findings appear next to the built-in checks in the export dialog and in `locus-cli validate`. A validator only reads the export model and reports; it can't change the document. Build and pack it like the template (`examples/plugin-template/README.md`). MIT OR Apache-2.0.
