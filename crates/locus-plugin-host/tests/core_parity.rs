// SPDX-License-Identifier: AGPL-3.0-or-later

//! The core-module host (ADR 0016, #253) against the component host (ADR
//! 0013), on the §15.8 fixtures and the example plugins (#254).
//!
//! The web host's canonical-ABI code runs here over wasmtime's core API. Both
//! hosts get the same component and the same limits; every call must give the
//! same validated output, or fail the same way (same `CallError` variant), and
//! a component one host refuses the other refuses too.
#![allow(clippy::unwrap_used, clippy::expect_used)]

#[path = "common/cases.rs"]
mod cases;
mod common;
#[path = "common/examples.rs"]
mod examples;

use std::time::Duration;

use cases::*;
use common::*;
use locus_plugin_host::core_host::native::WasmtimeCore;
use locus_plugin_host::core_host::{CoreModule, CorePlugin, ModuleError};
use locus_plugin_host::types::*;
use locus_plugin_host::{CallError, Host, Limits, Plugin};

const FUEL: u64 = 200_000_000;
const MEMORY: usize = 16 * 1024 * 1024;
fn host() -> Host {
    Host::with_limits(Limits {
        fuel: FUEL,
        memory_bytes: MEMORY,
        timeout: Duration::from_secs(5),
    })
    .unwrap()
}

/// Both hosts' view of one component; `None` where it refused to load.
fn load(wasm: &[u8]) -> (Option<Plugin>, Option<CorePlugin<WasmtimeCore>>) {
    let desktop = host().load(wasm).ok();
    let core = CoreModule::from_component(wasm)
        .ok()
        .and_then(|m| CorePlugin::new(WasmtimeCore::new(&m, FUEL, MEMORY).unwrap(), &m).ok());
    (desktop, core)
}

/// The same result, or the same kind of failure.
fn same<T: std::fmt::Debug + PartialEq>(
    what: &str,
    a: Result<T, CallError>,
    b: Result<T, CallError>,
) {
    match (&a, &b) {
        (Ok(x), Ok(y)) => assert_eq!(x, y, "{what}: different output"),
        (Err(CallError::Plugin(x)), Err(CallError::Plugin(y))) => {
            assert_eq!(x, y, "{what}: different plugin error");
        }
        (Err(x), Err(y)) => assert_eq!(
            std::mem::discriminant(x),
            std::mem::discriminant(y),
            "{what}: desktop {x:?}, core {y:?}"
        ),
        _ => panic!("{what}: desktop {a:?}, core {b:?}"),
    }
}

/// Every call either host can make on the plugin.
fn compare(name: &str, wasm: &[u8]) {
    let (d, c) = load(wasm);
    let (d, c) = match (d, c) {
        (Some(d), Some(c)) => (d, c),
        (None, None) => return,
        (d, c) => panic!(
            "{name}: desktop loaded: {}, core loaded: {}",
            d.is_some(),
            c.is_some()
        ),
    };
    assert_eq!(d.info(), c.info(), "{name}");
    assert_eq!(d.api_version(), c.api_version(), "{name}");
    assert_eq!(d.kinds(), c.kinds(), "{name}");
    for kind in d.kinds() {
        let what = format!("{name} {}", kind.name());
        match kind {
            Kind::Generator => {
                same(&what, d.generator_schema(), c.generator_schema());
                same(&what, d.generate(&params()), c.generate(&params()));
                same(&what, d.generate(&[]), c.generate(&[]));
            }
            Kind::Modifier => {
                same(&what, d.modifier_schema(), c.modifier_schema());
                let input = model().layers[0].paths.clone();
                same(
                    &what,
                    d.modify(&input, &params()),
                    c.modify(&input, &params()),
                );
            }
            Kind::Exporter => {
                same(&what, d.export_format(), c.export_format());
                same(&what, d.export(&model()), c.export(&model()));
                same(
                    &what,
                    d.export(&ExportModel::default()),
                    c.export(&ExportModel::default()),
                );
            }
            Kind::Importer => {
                same(&what, d.import_extensions(), c.import_extensions());
                let plt = b"IN;SP1;PU0,0;PD400,0,400,400;AA200,200,90;PU;CI40;SP0;";
                same(&what, d.import(plt), c.import(plt));
                same(&what, d.import(&[0xff, 0x00]), c.import(&[0xff, 0x00]));
            }
            Kind::Validator => {
                same(&what, d.validator_schema(), c.validator_schema());
                same(&what, d.validate(&model(), &[]), c.validate(&model(), &[]));
            }
        }
    }
}

#[test]
fn fixtures_behave_the_same_on_both_hosts() {
    for (name, f) in cases() {
        compare(name, &f.build());
    }
}

#[test]
fn example_plugins_behave_the_same_on_both_hosts() {
    for name in ["finger-box", "hpgl", "kerf-check", "plugin-template"] {
        let Some(core) = examples::build_example(name) else {
            return;
        };
        let wasm = locus_plugin_host::pack::componentize(&core).unwrap();
        compare(name, &wasm);
    }
}

#[test]
fn the_module_is_checked_and_its_memory_capped() {
    let m = CoreModule::from_component(&Fixture::well_behaved().build()).unwrap();
    assert_eq!(m.api, "1.0.0");
    assert_eq!(m.kinds, vec![Kind::Generator]);
    // The fixture declares no maximum; the copy has 4096 pages (256 MB).
    let mut maximum = None;
    for p in wasmparser::Parser::new(0).parse_all(&m.wasm) {
        if let wasmparser::Payload::MemorySection(s) = p.unwrap() {
            maximum = s.into_iter().next().unwrap().unwrap().maximum;
        }
    }
    assert_eq!(maximum, Some(4096));
    wasmparser::Validator::new().validate_all(&m.wasm).unwrap();

    let mut clock = Fixture::well_behaved();
    clock.import_clock = true;
    assert!(matches!(
        CoreModule::from_component(&clock.build()),
        Err(ModuleError::Imports(_))
    ));
    let mut huge = Fixture::well_behaved();
    huge.memory_pages = 5000;
    assert!(matches!(
        CoreModule::from_component(&huge.build()),
        Err(ModuleError::Memory(_))
    ));
    assert!(matches!(
        CoreModule::from_component(b"\0asm\x01\0\0\0"),
        Err(ModuleError::Invalid(_))
    ));
    assert!(CoreModule::from_component(b"junk").is_err());
}

/// #253: the worker protocol, end to end over the native engine: the reply
/// survives JSON, and a failing call comes back as an error message.
#[test]
fn worker_protocol_round_trips() {
    use locus_plugin_host::core_host::wire::{Reply, Request, handle, parse_reply, reply_json};
    let engine = |m: &CoreModule| WasmtimeCore::new(m, FUEL, MEMORY);
    let good = Fixture::well_behaved().build();

    let r = handle(engine, &good, Request::Describe);
    match parse_reply(&reply_json(&r)).unwrap() {
        Reply::Described { info, api, kinds } => {
            assert_eq!(info.id, "com.example.fixture");
            assert_eq!(api, "1.0.0");
            assert_eq!(kinds, vec![Kind::Generator]);
        }
        other => panic!("{other:?}"),
    }
    let desktop = host().load(&good).unwrap().generate(&params()).unwrap();
    let r = handle(engine, &good, Request::Generate(params()));
    assert_eq!(
        parse_reply(&reply_json(&r)).unwrap(),
        Reply::Generated(desktop)
    );

    let bad = Fixture::well_behaved().with_generate("unreachable").build();
    let r = handle(engine, &bad, Request::Generate(params()));
    let e = parse_reply(&reply_json(&r)).unwrap_err();
    assert_eq!(e.kind, "Trap");
    assert!(e.message.contains("crashed"), "{e:?}");
    let r = handle(engine, &good, Request::Format);
    assert!(parse_reply(&reply_json(&r)).is_err());
    assert!(parse_reply("not json").is_err());
}
