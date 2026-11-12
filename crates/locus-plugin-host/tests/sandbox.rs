// SPDX-License-Identifier: AGPL-3.0-or-later

//! The §15.8 host suite (#186): fixture plugins that behave, panic, loop
//! forever, hog memory, target the wrong API version or return malformed
//! output. Every failure must come back as a structured error, never a crash,
//! and the host must keep working afterwards.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::time::{Duration, Instant};

use common::*;
use locus_plugin_host::types::{Constraint, Entity, Kind, Param, ParamValue, PluginFailure, Vec2};
use locus_plugin_host::{CallError, CancelToken, Host, Limits, LoadError};

/// Small limits so the failure cases finish fast.
fn host() -> Host {
    Host::with_limits(Limits {
        fuel: 200_000_000,
        memory_bytes: 16 * 1024 * 1024,
        timeout: Duration::from_secs(5),
    })
    .unwrap()
}

fn params() -> Vec<Param> {
    vec![Param {
        key: "width".into(),
        value: ParamValue::Number(10.0),
    }]
}

#[test]
fn well_behaved_plugin_loads_and_generates() {
    let host = host();
    let plugin = host.load(&Fixture::well_behaved().build()).unwrap();
    assert_eq!(plugin.info().id, "com.example.fixture");
    assert_eq!(plugin.info().name, "Fixture");
    assert_eq!(plugin.kinds(), vec![Kind::Generator]);
    assert_eq!(plugin.api_version(), "1.0.0");
    assert!(plugin.generator_schema().unwrap().is_empty());

    let g = plugin.generate(&params()).unwrap();
    assert_eq!(g.points.len(), 4);
    assert_eq!(g.points[2], Vec2 { x: 10.0, y: 10.0 });
    assert_eq!(g.entities[3], Entity::Line(3, 0));
    assert_eq!(
        g.constraints,
        vec![Constraint::Horizontal(0), Constraint::Vertical(1)]
    );
}

#[test]
fn output_is_deterministic() {
    let host = host();
    let plugin = host.load(&Fixture::well_behaved().build()).unwrap();
    let a = plugin.generate(&params()).unwrap();
    let b = plugin.generate(&params()).unwrap();
    assert_eq!(a, b);
}

#[test]
fn not_a_plugin_kind_the_host_can_call() {
    let plugin = host().load(&Fixture::well_behaved().build()).unwrap();
    assert!(matches!(
        plugin.export_format(),
        Err(CallError::NotProvided("exporter"))
    ));
}

// ── Panicking ───────────────────────────────────────────────────────────────

#[test]
fn panicking_plugin_is_contained() {
    let host = host();
    let plugin = host
        .load(&Fixture::well_behaved().with_generate("unreachable").build())
        .unwrap();
    let err = plugin.generate(&params()).unwrap_err();
    assert!(matches!(err, CallError::Trap(_)), "{err:?}");

    // The same plugin, and the host, keep working: each call is a fresh instance.
    let ok = host.load(&Fixture::well_behaved().build()).unwrap();
    assert!(ok.generate(&params()).is_ok());
}

#[test]
fn stack_overflow_is_contained() {
    let mut f = Fixture::well_behaved().with_generate("call $recurse");
    f.extra = "  (func $recurse (result i32) call $recurse)\n".into();
    let plugin = host().load(&f.build()).unwrap();
    assert!(matches!(
        plugin.generate(&params()),
        Err(CallError::Trap(_))
    ));
}

#[test]
fn plugin_reported_error_is_passed_on_sanitised() {
    let f = Fixture::well_behaved()
        .with_data(ERROR_MSG, b"bad\x1bwidth".to_vec())
        .with_data(RESULT, err_generated(0, (ERROR_MSG, 9)));
    let plugin = host().load(&f.build()).unwrap();
    match plugin.generate(&params()) {
        Err(CallError::Plugin(PluginFailure::BadParameter(m))) => assert_eq!(m, "bad width"),
        other => panic!("{other:?}"),
    }
}

// ── Infinite loop ───────────────────────────────────────────────────────────

const SPIN: &str = "(loop $l (br $l)) unreachable";

#[test]
fn infinite_loop_runs_out_of_fuel() {
    let plugin = host()
        .load(&Fixture::well_behaved().with_generate(SPIN).build())
        .unwrap();
    assert!(matches!(
        plugin.generate(&params()),
        Err(CallError::CpuLimit)
    ));
}

#[test]
fn infinite_loop_hits_the_timeout() {
    let host = Host::with_limits(Limits {
        fuel: u64::MAX,
        memory_bytes: 16 * 1024 * 1024,
        timeout: Duration::from_millis(200),
    })
    .unwrap();
    let plugin = host
        .load(&Fixture::well_behaved().with_generate(SPIN).build())
        .unwrap();
    let t = Instant::now();
    let err = plugin.generate(&params()).unwrap_err();
    assert!(matches!(err, CallError::Timeout(_)), "{err:?}");
    assert!(t.elapsed() < Duration::from_secs(5), "{:?}", t.elapsed());
}

/// #214: a call running on another thread stops soon after its token is
/// cancelled, long before the timeout, and the plugin still works after.
#[test]
fn cancel_stops_a_running_call_from_another_thread() {
    let host = Host::with_limits(Limits {
        fuel: u64::MAX,
        memory_bytes: 16 * 1024 * 1024,
        timeout: Duration::from_secs(30),
    })
    .unwrap();
    let spinner = host
        .load(&Fixture::well_behaved().with_generate(SPIN).build())
        .unwrap();
    let token = CancelToken::new();
    let call = spinner.with_cancel(token.clone());
    let t = Instant::now();
    let worker = std::thread::spawn(move || call.generate(&params()));
    std::thread::sleep(Duration::from_millis(100));
    token.cancel();
    let err = worker.join().unwrap().unwrap_err();
    assert!(matches!(err, CallError::Cancelled), "{err:?}");
    assert!(t.elapsed() < Duration::from_secs(2), "{:?}", t.elapsed());

    // A cancelled token refuses new calls; the plugin without it still runs.
    let good = host.load(&Fixture::well_behaved().build()).unwrap();
    assert!(matches!(
        good.with_cancel(token).generate(&params()),
        Err(CallError::Cancelled)
    ));
    assert!(good.generate(&params()).is_ok());
    let fresh = good.with_cancel(CancelToken::new());
    assert!(fresh.generate(&params()).is_ok());
}

#[test]
fn timeout_still_works_after_the_host_is_dropped() {
    let host = Host::with_limits(Limits {
        fuel: u64::MAX,
        memory_bytes: 16 * 1024 * 1024,
        timeout: Duration::from_millis(100),
    })
    .unwrap();
    let plugin = host
        .load(&Fixture::well_behaved().with_generate(SPIN).build())
        .unwrap();
    drop(host);
    assert!(matches!(
        plugin.generate(&params()),
        Err(CallError::Timeout(_))
    ));
}

// ── Memory hog ──────────────────────────────────────────────────────────────

#[test]
fn memory_hog_hits_the_cap() {
    // Grows memory until the host refuses, then traps, like a Rust plugin
    // whose allocator fails.
    let hog =
        "(loop $l (br_if $l (i32.ne (memory.grow (i32.const 16)) (i32.const -1)))) unreachable";
    let plugin = host()
        .load(&Fixture::well_behaved().with_generate(hog).build())
        .unwrap();
    assert!(matches!(
        plugin.generate(&params()),
        Err(CallError::MemoryLimit(_))
    ));
}

#[test]
fn oversized_initial_memory_is_refused_at_load() {
    let mut f = Fixture::well_behaved();
    f.memory_pages = 5000; // ~312 MB, over the 16 MB test cap
    match host().load(&f.build()) {
        Err(LoadError::Describe(CallError::MemoryLimit(_))) => {}
        other => panic!("{other:?}"),
    }
}

#[test]
fn default_cap_is_256_mb() {
    assert_eq!(Limits::default().memory_bytes, 256 * 1024 * 1024);
    assert_eq!(Limits::default().timeout, Duration::from_secs(5));
}

// ── Wrong API version ───────────────────────────────────────────────────────

#[test]
fn wrong_api_major_is_refused() {
    let mut f = Fixture::well_behaved();
    f.api_version = "2.0.0".into();
    match host().load(&f.build()) {
        Err(LoadError::UnsupportedApi { found, .. }) => assert_eq!(found, "2.0.0"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn compatible_minor_is_accepted() {
    let mut f = Fixture::well_behaved();
    f.api_version = "1.2.0".into();
    let plugin = host().load(&f.build()).unwrap();
    assert_eq!(plugin.api_version(), "1.2.0");
}

// ── Sandbox: imports ────────────────────────────────────────────────────────

#[test]
fn plugin_that_imports_anything_is_refused() {
    let mut f = Fixture::well_behaved();
    f.import_clock = true;
    match host().load(&f.build()) {
        Err(LoadError::Imports(names)) => assert_eq!(names, vec!["now".to_owned()]),
        other => panic!("{other:?}"),
    }
}

#[test]
fn garbage_and_core_modules_are_refused() {
    let host = host();
    assert!(matches!(
        host.load(b"not wasm at all"),
        Err(LoadError::Invalid(_))
    ));
    // A core module is not a component.
    let core = wat::parse_str("(module)").unwrap();
    assert!(matches!(host.load(&core), Err(LoadError::Invalid(_))));
    // WAT text is refused too: installed plugins are binaries.
    assert!(matches!(
        host.load(b"(component)"),
        Err(LoadError::Invalid(_))
    ));
}

// ── Malformed output ────────────────────────────────────────────────────────

fn malformed(f: Fixture) {
    let plugin = host().load(&f.build()).unwrap();
    let err = plugin.generate(&params()).unwrap_err();
    assert!(matches!(err, CallError::Malformed(_)), "{err:?}");
}

#[test]
fn nan_coordinate_is_rejected() {
    malformed(Fixture::well_behaved().with_data(
        POINTS,
        f64s(&[0.0, 0.0, f64::NAN, 0.0, 10.0, 10.0, 0.0, 10.0]),
    ));
}

#[test]
fn dangling_point_index_is_rejected() {
    let entities = [line(0, 1), line(1, 2), line(2, 3), line(3, 99)].concat();
    malformed(Fixture::well_behaved().with_data(ENTITIES, entities));
}

#[test]
fn dangling_constraint_index_is_rejected() {
    let constraints = [constraint1(1, 0), constraint1(2, 40)].concat();
    malformed(Fixture::well_behaved().with_data(CONSTRAINTS, constraints));
}

#[test]
fn out_of_bounds_list_pointer_is_rejected() {
    malformed(Fixture::well_behaved().with_data(
        RESULT,
        ok_generated((0xFFFF_0000, 4), (ENTITIES, 4), (CONSTRAINTS, 2)),
    ));
}

#[test]
fn bad_variant_discriminant_is_rejected() {
    let mut e = line(0, 1);
    e[0] = 9;
    let entities = [e, line(1, 2), line(2, 3), line(3, 0)].concat();
    malformed(Fixture::well_behaved().with_data(ENTITIES, entities));
}

#[test]
fn too_many_items_are_rejected() {
    // 200 000 zero points fit in memory but exceed the output bound.
    let mut f = Fixture::well_behaved().with_data(
        RESULT,
        ok_generated((HEAP, 200_000), (ENTITIES, 0), (CONSTRAINTS, 0)),
    );
    f.memory_pages = 64;
    malformed(f);
}

#[test]
fn invalid_utf8_in_describe_fails_the_load() {
    let mut f = Fixture::well_behaved();
    f.id = b"com.example.\xff".to_vec();
    assert!(matches!(
        host().load(&f.build()),
        Err(LoadError::Describe(CallError::Malformed(_)))
    ));
}

#[test]
fn bad_plugin_id_fails_the_load() {
    let mut f = Fixture::well_behaved();
    f.id = b"NotReverseDns".to_vec();
    assert!(matches!(
        host().load(&f.build()),
        Err(LoadError::Describe(CallError::Malformed(_)))
    ));
}

// ── Document unchanged after any failure (#181, #186) ───────────────────────

/// What the app and CLI do: run the generator, and only on success turn the
/// output into one `AddGenerated` command.
fn insert(ed: &mut locus_doc::Editor, plugin: &locus_plugin_host::Plugin) -> Result<(), CallError> {
    use locus_plugin_host::doc::{plugin_ref, to_cache};
    let g = plugin.generate(&params())?;
    let info = plugin.info();
    ed.apply(&locus_doc::Command::AddGenerated {
        layer: locus_doc::Document::default_layer(),
        origin: locus_geom::Vec2::new(5.0, 5.0),
        plugin: plugin_ref(&info.id, &info.version, &params()),
        output: to_cache(&g),
    })
    .map_err(|e| CallError::Host(e.to_string()))?;
    Ok(())
}

#[test]
fn every_failure_leaves_the_document_unchanged() {
    let host = Host::with_limits(Limits {
        fuel: 200_000_000,
        memory_bytes: 16 * 1024 * 1024,
        timeout: Duration::from_millis(300),
    })
    .unwrap();
    let mut ed = locus_doc::Editor::new(locus_doc::Document::new());
    let good = host.load(&Fixture::well_behaved().build()).unwrap();
    insert(&mut ed, &good).unwrap();
    let before = ed.document().clone();

    let hog =
        "(loop $l (br_if $l (i32.ne (memory.grow (i32.const 16)) (i32.const -1)))) unreachable";
    let bad = [
        Fixture::well_behaved().with_generate("unreachable"),
        Fixture::well_behaved().with_generate(SPIN),
        Fixture::well_behaved().with_generate(hog),
        Fixture::well_behaved()
            .with_data(POINTS, f64s(&[0.0, 0.0, f64::NAN, 0.0, 1.0, 1.0, 0.0, 1.0])),
        Fixture::well_behaved().with_data(
            RESULT,
            ok_generated((0xFFFF_0000, 4), (ENTITIES, 4), (CONSTRAINTS, 2)),
        ),
        Fixture::well_behaved()
            .with_data(ERROR_MSG, b"no".to_vec())
            .with_data(RESULT, err_generated(2, (ERROR_MSG, 2))),
    ];
    for f in bad {
        let plugin = host.load(&f.build()).unwrap();
        assert!(insert(&mut ed, &plugin).is_err());
        assert_eq!(ed.document(), &before, "a failed call changed the document");
    }
    // The good one still inserts, as one more undo step.
    insert(&mut ed, &good).unwrap();
    assert_eq!(ed.document().recipes().len(), 2);
    ed.undo().unwrap();
    assert_eq!(ed.document().recipes().len(), 1);
}
