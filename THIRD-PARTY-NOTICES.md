# Third-party notices

Fonts bundled in the Locus binary (SPEC §9). Rust dependencies are covered by
`cargo deny check` (SPEC §2) and are not listed here.

| Font | Used as | Licence | Full text |
|------|---------|---------|-----------|
| Hershey fonts (`futural`, `rowmans`), A. V. Hershey, U.S. National Bureau of Standards; `.jhf` format by James Hurt, Cognition, Inc. | `hershey-sans`, `hershey-serif` single-line fonts | Public domain data with an acknowledgement requirement | [`crates/locus-text/fonts/hershey/LICENSE-HERSHEY.txt`](crates/locus-text/fonts/hershey/LICENSE-HERSHEY.txt) |
| Figtree, © 2022 The Figtree Project Authors | `figtree` outline font | SIL Open Font License 1.1 | [`crates/locus-text/fonts/figtree/OFL.txt`](crates/locus-text/fonts/figtree/OFL.txt) |
| Libre Baskerville, © 2012 The Libre Baskerville Project Authors (Reserved Font Name "Libre Baskerville") | `libre-baskerville` outline font | SIL Open Font License 1.1 | [`crates/locus-text/fonts/librebaskerville/OFL.txt`](crates/locus-text/fonts/librebaskerville/OFL.txt) |

The font files are embedded unmodified. Text laid out with them becomes
geometry in the user's document; the OFL permits that, and fonts are never
embedded in `.locus` files (SPEC §10).

The Hershey data comes from the `kamalmostafa/hershey-fonts` distribution of
James Hurt's files (<http://emergent.unpythonic.net/software/hershey>).
