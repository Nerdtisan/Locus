// SPDX-License-Identifier: AGPL-3.0-or-later

//! Exports the §15.8 fixture suite for the web host's browser run (#254).
//!
//! Writes each fixture component to `<dir>/<n>.wasm` and `<dir>/expected.json`:
//! for every call, what the **desktop** host (the reference, ADR 0013)
//! answered. The browser page (`crates/locus-plugin-worker/web-tests`) makes
//! the same calls through the real worker and must get the same answers.
//!
//! Successes must match exactly (the same JSON). Failures must match by
//! class, because the hosts stop a runaway plugin differently by design
//! (ADR 0016): desktop runs out of fuel where the browser times out, and
//! desktop's memory limiter reports what the browser sees as a trap once the
//! capped memory can't grow.
//!
//! `LOCUS_WEB_FIXTURES=<dir> cargo test -p locus-plugin-host --test web_fixtures`
//! writes the files; without it the export goes to a temporary folder.
#![allow(clippy::unwrap_used, clippy::expect_used)]

#[path = "common/cases.rs"]
mod cases;
mod common;

use std::time::Duration;

use cases::*;
use locus_plugin_host::core_host::wire::{Reply, Request};
use locus_plugin_host::types::*;
use locus_plugin_host::{CallError, Host, Limits, LoadError, Plugin};
use serde_json::{Value, json};

/// The class a failure belongs to; the browser page has the same mapping.
fn class(kind: &str) -> &'static str {
    match kind {
        "CpuLimit" | "Timeout" => "limit",
        "Trap" | "MemoryLimit" => "crash",
        "Malformed" => "malformed",
        "Plugin" => "plugin",
        "NotProvided" => "not-provided",
        "Refused" => "refused",
        _ => "host",
    }
}

fn load_class(e: &LoadError) -> &'static str {
    match e {
        LoadError::Describe(c) => class(c.kind()),
        _ => "refused",
    }
}

fn expect(r: Result<Reply, CallError>) -> Value {
    match r {
        Ok(reply) => json!({ "Ok": reply }),
        Err(e) => json!({ "Err": class(e.kind()) }),
    }
}

/// Every call the browser makes for one plugin, with the desktop's answers.
fn calls(p: &Plugin) -> Vec<(Request, Value)> {
    let mut out = vec![(
        Request::Describe,
        json!({ "Ok": Reply::Described {
            info: p.info().clone(),
            api: p.api_version().to_owned(),
            kinds: p.kinds(),
        }}),
    )];
    let input = model().layers[0].paths.clone();
    let plt = b"IN;SP1;PU0,0;PD400,0,400,400;AA200,200,90;PU;CI40;SP0;".to_vec();
    for kind in p.kinds() {
        match kind {
            Kind::Generator => {
                out.push((
                    Request::Schema(kind),
                    expect(p.generator_schema().map(Reply::Schema)),
                ));
                out.push((
                    Request::Generate(params()),
                    expect(p.generate(&params()).map(Reply::Generated)),
                ));
            }
            Kind::Modifier => {
                out.push((
                    Request::Schema(kind),
                    expect(p.modifier_schema().map(Reply::Schema)),
                ));
                out.push((
                    Request::Modify {
                        input: input.clone(),
                        params: params(),
                    },
                    expect(p.modify(&input, &params()).map(Reply::Paths)),
                ));
            }
            Kind::Exporter => {
                out.push((
                    Request::Format,
                    expect(p.export_format().map(Reply::Format)),
                ));
                out.push((
                    Request::Export(model()),
                    expect(p.export(&model()).map(Reply::Bytes)),
                ));
            }
            Kind::Importer => {
                out.push((
                    Request::Extensions,
                    expect(p.import_extensions().map(Reply::Extensions)),
                ));
                out.push((
                    Request::Import(plt.clone()),
                    expect(p.import(&plt).map(Reply::Paths)),
                ));
            }
            Kind::Validator => {
                out.push((
                    Request::Schema(kind),
                    expect(p.validator_schema().map(Reply::Schema)),
                ));
                out.push((
                    Request::Validate {
                        model: model(),
                        params: Vec::new(),
                    },
                    expect(p.validate(&model(), &[]).map(Reply::Findings)),
                ));
            }
        }
    }
    out
}

#[test]
fn export_the_suite_for_the_browser() {
    let dir = std::env::var_os("LOCUS_WEB_FIXTURES").map_or_else(
        || std::env::temp_dir().join(format!("locus-web-fixtures-{}", std::process::id())),
        std::path::PathBuf::from,
    );
    std::fs::create_dir_all(&dir).unwrap();
    // The desktop reference. Fuel stops a loop fast; the browser's worker
    // timeout stands in for it there.
    let host = Host::with_limits(Limits {
        fuel: 200_000_000,
        memory_bytes: 256 * 1024 * 1024,
        timeout: Duration::from_secs(5),
    })
    .unwrap();

    let mut entries = Vec::new();
    for (i, (name, fixture)) in cases().into_iter().enumerate() {
        let file = format!("{i:02}.wasm");
        let wasm = fixture.build();
        std::fs::write(dir.join(&file), &wasm).unwrap();
        let calls = match host.load(&wasm) {
            Ok(p) => calls(&p),
            Err(e) => vec![(Request::Describe, json!({ "Err": load_class(&e) }))],
        };
        for (request, expect) in calls {
            entries.push(json!({
                "name": name,
                "file": file,
                "request": serde_json::to_string(&request).unwrap(),
                "expect": expect,
            }));
        }
    }
    let n = entries.len();
    std::fs::write(
        dir.join("expected.json"),
        serde_json::to_string_pretty(&entries).unwrap(),
    )
    .unwrap();
    // Every fixture, and both outcomes, are in the suite.
    assert!(n > 30, "{n} calls");
    let text = std::fs::read_to_string(dir.join("expected.json")).unwrap();
    for class in ["limit", "crash", "malformed", "plugin", "refused"] {
        assert!(text.contains(&format!("\"{class}\"")), "no {class} case");
    }
    assert!(text.contains("\"Ok\""));
}
