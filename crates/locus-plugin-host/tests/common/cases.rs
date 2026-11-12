// SPDX-License-Identifier: AGPL-3.0-or-later

//! The §15.8 fixture cases, shared by `core_parity.rs` (desktop, both hosts)
//! and `web_fixtures.rs` (exported for the browser suite, #254).
#![allow(dead_code, clippy::unwrap_used, clippy::expect_used)]

use super::common::*;
use locus_plugin_host::types::*;

pub const SPIN: &str = "(loop $l (br $l)) unreachable";
pub const HOG: &str =
    "(loop $l (br_if $l (i32.ne (memory.grow (i32.const 16)) (i32.const -1)))) unreachable";

pub fn params() -> Vec<Param> {
    vec![Param {
        key: "width".into(),
        value: ParamValue::Number(10.0),
    }]
}

pub fn model() -> ExportModel {
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

/// The §15.8 fixtures shared by the parity test and the browser suite.
pub fn cases() -> Vec<(&'static str, Fixture)> {
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

    let cases: Vec<(&'static str, Fixture)> = vec![
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
    cases
}
