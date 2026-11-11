# Locus

Locus is a constrained 2D vector editor for pen plotters, vinyl cutters and laser cutters. You sketch with Fusion-360-style geometric and dimensional constraints instead of eyeballing and snapping, assign layers to machine operations (cut, score, engrave, draw), and export validated SVG, PDF and DXF that machine software accepts first time. It is written in Rust and targets **Windows, macOS, Linux and the web** (the full editor runs in the browser on GitHub Pages; plugins and the MCP server are desktop-only at first). Locus is at the pre-code stage: Milestone 0 (spikes and scaffolding) is next.

- Specification: [`docs/SPEC.md`](docs/SPEC.md)
- Working agreement for contributors and Claude Code: [`CLAUDE.md`](CLAUDE.md)
- Architecture decisions: [`docs/adr/`](docs/adr/)
- Writing a plugin: [`examples/plugin-template/`](examples/plugin-template/) (plus the finger-joint box and HPGL examples next to it)

## Licence

The core is licensed under the [GNU AGPL-3.0-or-later](LICENSE). The plugin API crate (`locus-plugin-api`) and the plugin template and examples in `examples/` are MIT OR Apache-2.0, with a plugin exception for plugins that talk to Locus only through the published interface (spec §2, §15.6). Contributions require a CLA.
