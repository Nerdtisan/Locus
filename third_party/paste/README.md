# `paste` shim (not real code)

This folder is a **shim**: a crate named `paste` whose only content is
`pub use pastey::paste;`. It is not a fork and contains no macro code of its own.

**Why it exists:** the original `paste` crate is archived and flagged unmaintained
(RUSTSEC-2024-0436). Locus doesn't use it directly, but `egui_dock` (ADR 0001) and
`gemm` (via faer → ezpz, ADR 0002) do. Cargo's `[patch.crates-io]` in the root
`Cargo.toml` can only swap a dependency for a crate with the **same name**, so this
crate is called `paste` (version 1.0.x) and forwards to [`pastey`](https://crates.io/crates/pastey),
the maintained drop-in fork the advisory recommends. `deny.toml` needs no exception.

**When to remove it:** once nothing depends on `paste`, e.g. after egui_dock and gemm
switch to `pastey` upstream. Tracked in Nerdtisan/Locus#99, which lists the steps.
Check with `cargo tree -i paste -e normal` in the dev container.
