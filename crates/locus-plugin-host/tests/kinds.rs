// SPDX-License-Identifier: AGPL-3.0-or-later

//! The §15.8 suite for the M4 kinds: importers (#233) and validators (#234).
//! Same rules as for generators: every failure is a structured error, output
//! is validated and bounded, and nothing reaches the document unless the
//! call succeeded.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::time::Duration;

use common::*;
use locus_plugin_host::types::{
    ExportModel, Finding, Kind, Layer, Operation, Path, Segment, Severity, Shape, Vec2,
};
use locus_plugin_host::{CallError, Host, Limits, LoadError, validators};

fn host() -> Host {
    Host::with_limits(Limits {
        fuel: 200_000_000,
        memory_bytes: 16 * 1024 * 1024,
        timeout: Duration::from_millis(500),
    })
    .unwrap()
}

const SPIN: &str = "(loop $l (br $l)) unreachable";
const HOG: &str =
    "(loop $l (br_if $l (i32.ne (memory.grow (i32.const 16)) (i32.const -1)))) unreachable";

/// A one-layer model with one closed path, as the host would send it.
fn model() -> ExportModel {
    ExportModel {
        layers: vec![Layer {
            name: "Cut".into(),
            colour: 0xff0000,
            operation: Operation::Score,
            paths: vec![Path {
                segments: vec![Segment {
                    shape: Shape::Circle {
                        center: Vec2 { x: 0.0, y: 0.0 },
                        radius: 1.0,
                    },
                    reversed: false,
                }],
                closed: true,
            }],
        }],
    }
}

// ── Importers ───────────────────────────────────────────────────────────────

#[test]
fn well_behaved_importer() {
    let p = host().load(&Fixture::importer().build()).unwrap();
    assert_eq!(p.kinds(), vec![Kind::Importer]);
    assert_eq!(p.import_extensions().unwrap(), vec!["plt".to_owned()]);
    let paths = p.import(b"IN;PU0,0;").unwrap();
    assert_eq!(paths.len(), 1);
    assert!(paths[0].closed);
    assert_eq!(
        paths[0].segments[0].shape,
        Shape::Circle {
            center: Vec2 { x: 10.0, y: 10.0 },
            radius: 5.0
        }
    );
    // A generator call on an importer is refused, not attempted.
    assert!(matches!(p.generate(&[]), Err(CallError::NotProvided(_))));
}

fn import_err(f: Fixture) -> CallError {
    host().load(&f.build()).unwrap().import(b"x").unwrap_err()
}

#[test]
fn importer_failures_are_contained() {
    assert!(matches!(
        import_err(Fixture::importer().with_generate("unreachable")),
        CallError::Trap(_)
    ));
    let e = import_err(Fixture::importer().with_generate(SPIN));
    assert!(
        matches!(e, CallError::CpuLimit | CallError::Timeout(_)),
        "{e:?}"
    );
    assert!(matches!(
        import_err(Fixture::importer().with_generate(HOG)),
        CallError::MemoryLimit(_)
    ));
}

#[test]
fn malformed_importer_output_is_rejected() {
    // NaN centre.
    let nan = Fixture::importer().with_data(POINTS, circle_segment(f64::NAN, 0.0, 1.0));
    assert!(matches!(import_err(nan), CallError::Malformed(_)));
    // Negative radius.
    let neg = Fixture::importer().with_data(POINTS, circle_segment(0.0, 0.0, -1.0));
    assert!(matches!(import_err(neg), CallError::Malformed(_)));
    // Nothing at all.
    let empty = Fixture::importer().with_data(RESULT, ok_list(ENTITIES, 0));
    assert!(matches!(import_err(empty), CallError::Malformed(_)));
    // A list pointer outside memory.
    let oob = Fixture::importer().with_data(RESULT, ok_list(0xFFFF_0000, 4));
    assert!(matches!(import_err(oob), CallError::Malformed(_)));
}

#[test]
fn oversize_importer_output_is_rejected() {
    // 200 001 paths, each pointing at the same one segment: over the bound.
    let mut f = Fixture::importer().with_data(RESULT, ok_list(HEAP, 200_001));
    f.memory_pages = 64;
    f.extra = format!(
        "  (func $fill (local $i i32)\n    (loop $l\n      (i32.store (i32.add (i32.const {HEAP}) (i32.mul (local.get $i) (i32.const 12))) (i32.const {POINTS}))\n      (i32.store (i32.add (i32.const {}) (i32.mul (local.get $i) (i32.const 12))) (i32.const 1))\n      (local.set $i (i32.add (local.get $i) (i32.const 1)))\n      (br_if $l (i32.lt_u (local.get $i) (i32.const 200001)))))\n",
        HEAP + 4
    );
    f.generate = format!("call $fill i32.const {RESULT}");
    assert!(matches!(import_err(f), CallError::Malformed(_)));
}

#[test]
fn bad_extensions_are_rejected() {
    // "P.T": not lowercase letters/digits.
    let f = Fixture::importer().with_data(1200, b"P.T".to_vec());
    let p = host().load(&f.build()).unwrap();
    assert!(matches!(
        p.import_extensions(),
        Err(CallError::Malformed(_))
    ));
}

#[test]
fn importer_with_wrong_api_major_is_refused() {
    let mut f = Fixture::importer();
    f.api_version = "2.0.0".into();
    assert!(matches!(
        host().load(&f.build()),
        Err(LoadError::UnsupportedApi { .. })
    ));
}

#[test]
fn importer_file_size_is_bounded_by_the_host() {
    let p = host().load(&Fixture::importer().build()).unwrap();
    let big = vec![0u8; locus_plugin_host::validate::MAX_IMPORT_BYTES + 1];
    assert!(matches!(p.import(&big), Err(CallError::Host(_))));
}

/// What the app does: import, then (only on success) apply one command.
fn import_into(ed: &mut locus_doc::Editor, f: Fixture) -> Result<(), String> {
    let p = host().load(&f.build()).map_err(|e| e.to_string())?;
    let paths = p.import(b"data").map_err(|e| e.to_string())?;
    let cmd = locus_plugin_host::doc::import_command(ed.document(), "data.plt", &paths)
        .map_err(|e| e.to_string())?;
    ed.apply(&cmd).map_err(|e| e.to_string())?;
    Ok(())
}

#[test]
fn import_is_one_undo_step_and_failures_change_nothing() {
    let mut ed = locus_doc::Editor::new(locus_doc::Document::new());
    let before = ed.document().clone();
    for bad in [
        Fixture::importer().with_generate("unreachable"),
        Fixture::importer().with_generate(SPIN),
        Fixture::importer().with_data(POINTS, circle_segment(f64::INFINITY, 0.0, 1.0)),
    ] {
        assert!(import_into(&mut ed, bad).is_err());
        assert_eq!(ed.document(), &before);
    }
    import_into(&mut ed, Fixture::importer()).unwrap();
    let layers = ed.document().layers();
    assert_eq!(layers.len(), before.layers().len() + 1);
    let (_, new) = layers.iter().last().unwrap();
    assert!(new.locked);
    assert_eq!(new.operation, locus_doc::Operation::Reference);
    ed.undo().unwrap();
    assert_eq!(ed.document().layers(), before.layers());
    assert_eq!(ed.document().entities().len(), 0);
}

// ── Validators ──────────────────────────────────────────────────────────────

#[test]
fn well_behaved_validator() {
    let p = host().load(&Fixture::validator().build()).unwrap();
    assert_eq!(p.kinds(), vec![Kind::Validator]);
    assert!(p.validator_schema().unwrap().is_empty());
    let found = p.validate(&model(), &[]).unwrap();
    assert_eq!(
        found,
        vec![Finding {
            severity: Severity::Warning,
            message: "fixture finding".into(),
            layer: Some(0),
            path: None,
        }]
    );
}

fn validate_err(f: Fixture) -> CallError {
    host()
        .load(&f.build())
        .unwrap()
        .validate(&model(), &[])
        .unwrap_err()
}

#[test]
fn validator_failures_are_contained() {
    assert!(matches!(
        validate_err(Fixture::validator().with_generate("unreachable")),
        CallError::Trap(_)
    ));
    let e = validate_err(Fixture::validator().with_generate(SPIN));
    assert!(
        matches!(e, CallError::CpuLimit | CallError::Timeout(_)),
        "{e:?}"
    );
    assert!(matches!(
        validate_err(Fixture::validator().with_generate(HOG)),
        CallError::MemoryLimit(_)
    ));
}

#[test]
fn malformed_findings_are_rejected() {
    let msg = (ERROR_MSG, FINDING_MSG.len() as u32);
    // A layer the model doesn't have.
    let f = Fixture::validator().with_data(POINTS, finding(1, msg, Some(7), None));
    assert!(matches!(validate_err(f), CallError::Malformed(_)));
    // A path the layer doesn't have.
    let f = Fixture::validator().with_data(POINTS, finding(1, msg, Some(0), Some(3)));
    assert!(matches!(validate_err(f), CallError::Malformed(_)));
    // A path without a layer.
    let f = Fixture::validator().with_data(POINTS, finding(1, msg, None, Some(0)));
    assert!(matches!(validate_err(f), CallError::Malformed(_)));
    // Control characters in the message.
    let f = Fixture::validator().with_data(ERROR_MSG, b"bad\x07 message!!".to_vec());
    assert!(matches!(validate_err(f), CallError::Malformed(_)));
    // Invalid UTF-8.
    let f = Fixture::validator().with_data(ERROR_MSG, vec![0xff; FINDING_MSG.len()]);
    assert!(matches!(validate_err(f), CallError::Malformed(_)));
    // Bad severity discriminant.
    let f = Fixture::validator().with_data(POINTS, finding(5, msg, None, None));
    assert!(matches!(validate_err(f), CallError::Malformed(_)));
}

#[test]
fn validator_failures_never_hide_other_findings() {
    let host = host();
    let good = host.load(&Fixture::validator().build()).unwrap();
    let bad = host
        .load(&Fixture::validator().with_generate("unreachable").build())
        .unwrap();
    let export = locus_export::ExportModel {
        layers: vec![locus_export::ExportLayer {
            id: locus_doc::LayerId::new(1),
            name: "Score".into(),
            colour: [0, 0, 255],
            operation: locus_doc::Operation::Score,
            paths: Vec::new(),
            degenerate: Vec::new(),
        }],
    };
    let found = validators::run(&[&bad, &good], &export);
    assert_eq!(found.len(), 2);
    assert_eq!(found[0].severity, Severity::Warning);
    assert!(found[0].message.contains("failed"), "{}", found[0].message);
    assert_eq!(found[1].message, "fixture finding");
    assert_eq!(found[1].layer, Some(locus_doc::LayerId::new(1)));
    assert!(!validators::has_errors(&found));
}
