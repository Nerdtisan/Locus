// SPDX-License-Identifier: AGPL-3.0-or-later

//! The core-module host (ADR 0016, #253) against the component host (ADR
//! 0013), on the §15.8 fixtures and the example plugins (#254).
//!
//! The web host's canonical-ABI code runs here over wasmtime's core API. Both
//! hosts get the same component and the same limits; every call must give the
//! same validated output, or fail the same way (same `CallError` variant), and
//! a component one host refuses the other refuses too.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;
#[path = "common/examples.rs"]
mod examples;

use std::time::Duration;

use common::*;
use locus_plugin_host::core_host::native::WasmtimeCore;
use locus_plugin_host::core_host::{CoreModule, CorePlugin, ModuleError};
use locus_plugin_host::types::*;
use locus_plugin_host::{CallError, Host, Limits, Plugin};

const FUEL: u64 = 200_000_000;
const MEMORY: usize = 16 * 1024 * 1024;
const SPIN: &str = "(loop $l (br $l)) unreachable";
const HOG: &str =
    "(loop $l (br_if $l (i32.ne (memory.grow (i32.const 16)) (i32.const -1)))) unreachable";

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

fn params() -> Vec<Param> {
    vec![Param {
        key: "width".into(),
        value: ParamValue::Number(10.0),
    }]
}

fn model() -> ExportModel {
    ExportModel {
        layers: vec![Layer {
            name: "Cut".into(),
            colour: 0xff0000,
            operation: Operation::Cut {
                kerf_width: 0.2,
                compensation: Compensation::Inside,
                passes: 2,
            },
            paths: vec![
                Path {
                    segments: vec![Segment {
                        shape: Shape::Circle {
                            center: Vec2 { x: 5.0, y: 5.0 },
                            radius: 0.05,
                        },
                        reversed: false,
                    }],
                    closed: true,
                },
                Path {
                    segments: vec![
                        Segment {
                            shape: Shape::Line {
                                a: Vec2 { x: 0.0, y: 0.0 },
                                b: Vec2 { x: 0.4, y: 0.0 },
                            },
                            reversed: false,
                        },
                        Segment {
                            shape: Shape::Arc {
                                center: Vec2 { x: 0.4, y: 0.2 },
                                start: Vec2 { x: 0.4, y: 0.0 },
                                end: Vec2 { x: 0.4, y: 0.4 },
                            },
                            reversed: true,
                        },
                        Segment {
                            shape: Shape::Line {
                                a: Vec2 { x: 0.4, y: 0.4 },
                                b: Vec2 { x: 0.0, y: 0.0 },
                            },
                            reversed: false,
                        },
                    ],
                    closed: true,
                },
            ],
        }],
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
    let msg = (ERROR_MSG, FINDING_MSG.len() as u32);
    let mut clock = Fixture::well_behaved();
    clock.import_clock = true;
    let mut wrong_api = Fixture::well_behaved();
    wrong_api.api_version = "2.0.0".into();
    let mut minor = Fixture::well_behaved();
    minor.api_version = "1.3.0".into();
    let mut bad_utf8 = Fixture::well_behaved();
    bad_utf8.name = vec![0xff, 0xfe];
    let mut big = Fixture::well_behaved().with_data(
        RESULT,
        ok_generated((HEAP, 200_000), (ENTITIES, 0), (CONSTRAINTS, 0)),
    );
    big.memory_pages = 64;
    let mut bad_disc = line(0, 1);
    bad_disc[0] = 9;

    let cases: Vec<(&str, Fixture)> = vec![
        ("well-behaved", Fixture::well_behaved()),
        (
            "panicking",
            Fixture::well_behaved().with_generate("unreachable"),
        ),
        ("infinite loop", Fixture::well_behaved().with_generate(SPIN)),
        ("memory hog", Fixture::well_behaved().with_generate(HOG)),
        ("imports a clock", clock),
        ("wrong API major", wrong_api),
        ("newer minor", minor),
        ("bad UTF-8 name", bad_utf8),
        (
            "NaN point",
            Fixture::well_behaved()
                .with_data(POINTS, f64s(&[0.0, 0.0, f64::NAN, 0.0, 1.0, 1.0, 0.0, 1.0])),
        ),
        (
            "dangling index",
            Fixture::well_behaved().with_data(
                ENTITIES,
                [line(0, 1), line(1, 2), line(2, 3), line(3, 99)].concat(),
            ),
        ),
        (
            "pointer out of memory",
            Fixture::well_behaved().with_data(
                RESULT,
                ok_generated((0xFFFF_0000, 4), (ENTITIES, 4), (CONSTRAINTS, 2)),
            ),
        ),
        (
            "bad discriminant",
            Fixture::well_behaved().with_data(
                ENTITIES,
                [bad_disc, line(1, 2), line(2, 3), line(3, 0)].concat(),
            ),
        ),
        ("too many items", big),
        (
            "plugin error",
            Fixture::well_behaved()
                .with_data(ERROR_MSG, b"no\x07pe".to_vec())
                .with_data(RESULT, err_generated(2, (ERROR_MSG, 5))),
        ),
        ("identity modifier", Fixture::modifier_identity()),
        ("importer", Fixture::importer()),
        (
            "panicking importer",
            Fixture::importer().with_generate("unreachable"),
        ),
        (
            "importer NaN",
            Fixture::importer().with_data(POINTS, circle_segment(f64::NAN, 0.0, 1.0)),
        ),
        (
            "importer empty",
            Fixture::importer().with_data(RESULT, ok_list(ENTITIES, 0)),
        ),
        (
            "importer bad extension",
            Fixture::importer().with_data(1200, b"P.T".to_vec()),
        ),
        ("validator", Fixture::validator()),
        (
            "looping validator",
            Fixture::validator().with_generate(SPIN),
        ),
        (
            "validator bad layer",
            Fixture::validator().with_data(POINTS, finding(1, msg, Some(7), None)),
        ),
        (
            "validator bad severity",
            Fixture::validator().with_data(POINTS, finding(5, msg, None, None)),
        ),
    ];
    for (name, f) in cases {
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
    assert!(
        parse_reply(&reply_json(&r))
            .unwrap_err()
            .contains("crashed")
    );
    let r = handle(engine, &good, Request::Format);
    assert!(parse_reply(&reply_json(&r)).is_err());
    assert!(parse_reply("not json").is_err());
}
