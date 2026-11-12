// SPDX-License-Identifier: AGPL-3.0-or-later

//! Browser side of file handling: open via file picker, save/export as a
//! download, and debounced autosave of the current document to OPFS with an
//! IndexedDB fallback, restored on reload (M1 #75). The JS lives in `web.js`.
//!
//! Autosave is per-browser storage only; the UI says so. Nothing here logs
//! document contents or file names.

use locus_doc::Document;
use std::sync::mpsc::{Receiver, Sender, channel};
use wasm_bindgen::prelude::*;

#[wasm_bindgen(module = "/src/file_io/web.js")]
extern "C" {
    fn download(name: &str, data: &[u8], mime: &str);
    #[wasm_bindgen(catch)]
    async fn pick_text(accept: &str) -> Result<JsValue, JsValue>;
    #[wasm_bindgen(catch)]
    async fn autosave_write(text: &str) -> Result<JsValue, JsValue>;
    #[wasm_bindgen(catch)]
    async fn autosave_read() -> Result<JsValue, JsValue>;
}

/// Wait this long after the last change before writing the autosave (seconds).
const AUTOSAVE_DELAY: f64 = 1.0;
/// Back-off after a failed autosave write.
const AUTOSAVE_RETRY: f64 = 10.0;

/// Results of async browser work, delivered to the UI thread.
#[derive(Debug)]
pub enum Msg {
    /// `Ok(None)` = the picker was cancelled.
    Picked(Result<Option<(String, String)>, String>),
    /// File ▸ Import reference…: an SVG or DXF, name and text (#243).
    PickedReference(Result<Option<(String, String)>, String>),
    /// The autosaved document text found at startup, if any.
    Restore(Option<String>),
    AutosaveDone(Result<(), String>),
}

#[derive(Debug)]
pub struct Web {
    tx: Sender<Msg>,
    rx: Receiver<Msg>,
    ctx: egui::Context,
    /// What the autosave currently holds.
    last_saved: Option<Document>,
    /// The snapshot being written.
    in_flight: Option<Document>,
    changed_at: Option<f64>,
}

impl Web {
    pub fn new(ctx: &egui::Context) -> Self {
        let (tx, rx) = channel();
        let web = Self {
            tx,
            rx,
            ctx: ctx.clone(),
            last_saved: None,
            in_flight: None,
            changed_at: None,
        };
        web.spawn(async {
            // A failed read just means nothing to restore.
            let text = autosave_read().await.ok().and_then(|v| v.as_string());
            Msg::Restore(text)
        });
        web
    }

    fn spawn(&self, fut: impl std::future::Future<Output = Msg> + 'static) {
        let tx = self.tx.clone();
        let ctx = self.ctx.clone();
        wasm_bindgen_futures::spawn_local(async move {
            // The receiver only disappears when the app is torn down.
            let _ = tx.send(fut.await);
            ctx.request_repaint();
        });
    }

    /// Show the browser's file picker.
    pub fn pick(&self) {
        self.spawn(async {
            let res = match pick_text(".locus").await {
                Ok(v) => match v.as_string() {
                    None => Ok(None),
                    Some(json) => parse_picked(&json).map(Some),
                },
                Err(_) => Err("Could not open the file picker.".to_owned()),
            };
            Msg::Picked(res)
        });
    }

    /// Show the file picker for an SVG or DXF to import as a reference.
    pub fn pick_reference(&self) {
        self.spawn(async {
            let res = match pick_text(".svg,.dxf").await {
                Ok(v) => match v.as_string() {
                    None => Ok(None),
                    Some(json) => parse_picked(&json).map(Some),
                },
                Err(_) => Err("Could not open the file picker.".to_owned()),
            };
            Msg::PickedReference(res)
        });
    }

    pub fn download(name: &str, data: &[u8], mime: &str) {
        download(name, data, mime);
    }

    /// Drain finished async work. Autosave bookkeeping is handled here; the
    /// rest is returned for the caller.
    pub fn poll(&mut self, now: f64) -> Vec<Msg> {
        let mut out = Vec::new();
        while let Ok(msg) = self.rx.try_recv() {
            match msg {
                Msg::AutosaveDone(res) => {
                    let snapshot = self.in_flight.take();
                    match res {
                        Ok(()) => self.last_saved = snapshot,
                        // Keep the old copy and try again later.
                        Err(_) => self.changed_at = Some(now + AUTOSAVE_RETRY - AUTOSAVE_DELAY),
                    }
                }
                other => out.push(other),
            }
        }
        out
    }

    /// Record that the autosave now matches `doc` (after a restore).
    pub fn mark_saved(&mut self, doc: &Document) {
        self.last_saved = Some(doc.clone());
    }

    /// Debounced autosave: write once the document has been stable for a moment.
    pub fn tick_autosave(&mut self, now: f64, doc: &Document) {
        if self.in_flight.is_some() {
            return;
        }
        // Until a restore settles, the autosave holds something unknown; don't overwrite it.
        let Some(saved) = &self.last_saved else {
            return;
        };
        if saved == doc {
            self.changed_at = None;
            return;
        }
        let since = *self.changed_at.get_or_insert(now);
        if now - since < AUTOSAVE_DELAY {
            self.ctx
                .request_repaint_after(std::time::Duration::from_secs_f64(AUTOSAVE_DELAY));
            return;
        }
        let Ok(text) = locus_doc::file::save(doc) else {
            return;
        };
        self.in_flight = Some(doc.clone());
        self.changed_at = None;
        self.spawn(async move {
            Msg::AutosaveDone(
                autosave_write(&text)
                    .await
                    .map(|_| ())
                    .map_err(|_| "autosave failed".to_owned()),
            )
        });
    }
}

fn parse_picked(json: &str) -> Result<(String, String), String> {
    #[derive(serde::Deserialize)]
    struct Picked {
        name: Option<String>,
        text: Option<String>,
        error: Option<String>,
    }
    let p: Picked =
        serde_json::from_str(json).map_err(|_| "Could not read the file.".to_owned())?;
    match (p.name, p.text, p.error) {
        (Some(n), Some(t), _) => Ok((n, t)),
        (_, _, Some(e)) => Err(e),
        _ => Err("Could not read the file.".to_owned()),
    }
}
