// SPDX-License-Identifier: AGPL-3.0-or-later

//! The plugin worker of the web build (ADR 0016, #253).
//!
//! The app starts one dedicated Web Worker per plugin call and posts it
//! `{component: Uint8Array, request: string}`. The worker checks the
//! component, runs the call on the browser's own WebAssembly engine through
//! `locus_plugin_host::core_host` (canonical ABI and validation), posts back
//! one JSON string, and is then terminated by the app, or earlier if the call
//! runs past its timeout (§15.4).
//!
//! Sandbox: the plugin's core module is instantiated with an **empty import
//! object**, so it has nothing to call; its memory maximum is capped at
//! 256 MB before it is compiled. The worker itself has no access to the
//! document or the page, only to the bytes it was sent.

#[cfg(target_arch = "wasm32")]
mod web;

fn main() {
    #[cfg(target_arch = "wasm32")]
    web::start();
}
