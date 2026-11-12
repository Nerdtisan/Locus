// SPDX-License-Identifier: AGPL-3.0-or-later

//! The template and first-party example plugins, end to end (#185, §15.8):
//! build with Cargo for wasm32, pack, install, run, and compare with golden
//! output.
#![allow(clippy::unwrap_used, clippy::expect_used)]

#[path = "common/examples.rs"]
mod examples;

use std::fmt::Write as _;
use std::path::PathBuf;

use examples::{build_example, example_manifest, golden};
use locus_plugin_host::archive::Archive;
use locus_plugin_host::pack::{PackError, componentize, pack};
use locus_plugin_host::registry::Registry;
use locus_plugin_host::types::*;
use locus_plugin_host::{Host, validate};

fn golden_dir() -> PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden")
}

/// Every parameter at its schema default.
fn defaults(schema: &[ParamSpec]) -> Vec<Param> {
    schema
        .iter()
        .filter_map(|s| {
            let value = match &s.kind {
                ParamKind::Number(n) => ParamValue::Number(n.default),
                ParamKind::Integer(n) => ParamValue::Integer(n.default),
                ParamKind::Boolean(b) => ParamValue::Boolean(*b),
                ParamKind::Choice(c) => ParamValue::Choice(c.default),
                ParamKind::Text(t) => ParamValue::Text(t.default.clone()),
                ParamKind::DocParameter => return None,
            };
            Some(Param {
                key: s.key.clone(),
                value,
            })
        })
        .collect()
}

fn dump(g: &Generated) -> String {
    let mut s = String::new();
    for (i, p) in g.points.iter().enumerate() {
        let _ = writeln!(s, "p{i} {:.6} {:.6}", p.x, p.y);
    }
    for (i, e) in g.entities.iter().enumerate() {
        let _ = writeln!(s, "e{i} {e:?}");
    }
    for (i, c) in g.constraints.iter().enumerate() {
        let _ = writeln!(s, "c{i} {c:?}");
    }
    s
}

#[test]
fn template_builds_a_working_locusplugin() {
    let Some(core) = build_example("plugin-template") else {
        return;
    };
    let host = Host::new().unwrap();
    let bytes = pack(
        &host,
        &example_manifest("plugin-template"),
        &core,
        None,
        None,
    )
    .unwrap();

    let dir = std::env::temp_dir().join(format!("locus-template-{}", std::process::id()));
    let reg = Registry::new(&dir);
    let installed = reg.install(&host, &bytes).unwrap();
    assert_eq!(installed.manifest.id, "com.example.my-plugin");
    let (_, plugin) = reg.load(&host, "com.example.my-plugin").unwrap();
    let schema = plugin.generator_schema().unwrap();
    let g = plugin.generate(&defaults(&schema)).unwrap();
    let _ = std::fs::remove_dir_all(&dir);

    assert_eq!(g.points.len(), 4);
    assert_eq!(g.entities.len(), 4);
    assert_eq!(g.points[2], Vec2 { x: 40.0, y: 25.0 });
    assert_eq!(
        g.constraints,
        vec![
            Constraint::Horizontal(0),
            Constraint::Vertical(1),
            Constraint::Horizontal(2),
            Constraint::Vertical(3)
        ]
    );
}

#[test]
fn finger_box_golden() {
    let Some(core) = build_example("finger-box") else {
        return;
    };
    let host = Host::new().unwrap();
    let bytes = pack(&host, &example_manifest("finger-box"), &core, None, None).unwrap();
    let plugin = host.load(&Archive::parse(&bytes).unwrap().wasm).unwrap();
    assert_eq!(plugin.info().id, "io.locus.finger-box");
    assert_eq!(plugin.kinds(), vec![Kind::Generator]);

    let schema = plugin.generator_schema().unwrap();
    validate::schema(&schema).unwrap();
    assert_eq!(schema.len(), 6);

    let mut params = defaults(&schema);
    let g = plugin.generate(&params).unwrap();
    // Five closed panels, all lines; each point is used by exactly two lines.
    assert!(g.entities.iter().all(|e| matches!(e, Entity::Line(..))));
    let mut uses = vec![0; g.points.len()];
    for e in &g.entities {
        if let Entity::Line(a, b) = e {
            uses[*a as usize] += 1;
            uses[*b as usize] += 1;
        }
    }
    assert!(
        uses.iter().all(|&u| u == 2),
        "every panel outline is closed"
    );
    golden(&golden_dir(), "finger-box.txt", dump(&g).as_bytes());

    // Deterministic (§15.5).
    assert_eq!(plugin.generate(&params).unwrap(), g);

    // With a lid there is a sixth panel.
    let lid = params.iter_mut().find(|p| p.key == "lid").unwrap();
    lid.value = ParamValue::Boolean(true);
    let with_lid = plugin.generate(&params).unwrap();
    assert!(with_lid.entities.len() > g.entities.len());

    // Bad parameters are reported, not trapped.
    let t = params.iter_mut().find(|p| p.key == "thickness").unwrap();
    t.value = ParamValue::Number(24.0);
    assert!(matches!(
        plugin.generate(&params),
        Err(locus_plugin_host::CallError::Plugin(
            PluginFailure::BadParameter(_)
        ))
    ));
}

fn line(ax: f64, ay: f64, bx: f64, by: f64) -> Segment {
    Segment {
        shape: Shape::Line {
            a: Vec2 { x: ax, y: ay },
            b: Vec2 { x: bx, y: by },
        },
        reversed: false,
    }
}

/// A square, an arc slot and a circle on two layers.
fn hpgl_model() -> ExportModel {
    let square = Path {
        segments: vec![
            line(0.0, 0.0, 20.0, 0.0),
            line(20.0, 0.0, 20.0, 20.0),
            line(20.0, 20.0, 0.0, 20.0),
            line(0.0, 20.0, 0.0, 0.0),
        ],
        closed: true,
    };
    // A quarter-ring slot: outer arc, line in, inner arc back (reversed), line out.
    let arc = Path {
        segments: vec![
            Segment {
                shape: Shape::Arc {
                    center: Vec2 { x: 10.0, y: 10.0 },
                    start: Vec2 { x: 15.0, y: 10.0 },
                    end: Vec2 { x: 10.0, y: 15.0 },
                },
                reversed: false,
            },
            line(10.0, 15.0, 10.0, 13.0),
            Segment {
                shape: Shape::Arc {
                    center: Vec2 { x: 10.0, y: 10.0 },
                    start: Vec2 { x: 13.0, y: 10.0 },
                    end: Vec2 { x: 10.0, y: 13.0 },
                },
                reversed: true,
            },
            line(13.0, 10.0, 15.0, 10.0),
        ],
        closed: true,
    };
    let circle = Path {
        segments: vec![Segment {
            shape: Shape::Circle {
                center: Vec2 { x: 40.0, y: 10.0 },
                radius: 7.5,
            },
            reversed: false,
        }],
        closed: true,
    };
    ExportModel {
        layers: vec![
            Layer {
                name: "Cut".into(),
                colour: 0xff0000,
                operation: Operation::Cut {
                    kerf_width: 0.1,
                    compensation: Compensation::None,
                    passes: 1,
                },
                paths: vec![square, circle],
            },
            Layer {
                name: "Pen 3".into(),
                colour: 0x0000ff,
                operation: Operation::Draw {
                    pen_number: 3,
                    pen_width: 0.3,
                },
                paths: vec![arc],
            },
        ],
    }
}

#[test]
fn hpgl_golden() {
    let Some(core) = build_example("hpgl") else {
        return;
    };
    let host = Host::new().unwrap();
    let bytes = pack(&host, &example_manifest("hpgl"), &core, None, None).unwrap();
    let plugin = host.load(&Archive::parse(&bytes).unwrap().wasm).unwrap();
    assert_eq!(plugin.kinds(), vec![Kind::Exporter, Kind::Importer]);
    let format = plugin.export_format().unwrap();
    assert_eq!(
        (format.id.as_str(), format.extension.as_str()),
        ("hpgl", "plt")
    );

    let out = plugin.export(&hpgl_model()).unwrap();
    golden(&golden_dir(), "example.plt", &out);
    assert_eq!(
        plugin.export(&ExportModel::default()).unwrap(),
        b"IN;\nSP0;\n"
    );
}

fn dump_paths(paths: &[Path]) -> String {
    let mut s = String::new();
    let p = |v: &Vec2| format!("({:.4}, {:.4})", v.x, v.y);
    for (i, path) in paths.iter().enumerate() {
        let _ = writeln!(s, "path {i} closed={}", path.closed);
        for seg in &path.segments {
            let shape = match &seg.shape {
                Shape::Line { a, b } => format!("line {} {}", p(a), p(b)),
                Shape::Arc { center, start, end } => {
                    format!("arc {} {} {}", p(center), p(start), p(end))
                }
                Shape::Circle { center, radius } => format!("circle {} {radius:.4}", p(center)),
            };
            let _ = writeln!(s, "  {shape} reversed={}", seg.reversed);
        }
    }
    s
}

/// #233: the HPGL importer reads back what the exporter wrote, as reference
/// geometry, and lands it as one undo step on a locked layer.
#[test]
fn hpgl_import_golden() {
    let Some(core) = build_example("hpgl") else {
        return;
    };
    let host = Host::new().unwrap();
    let bytes = pack(&host, &example_manifest("hpgl"), &core, None, None).unwrap();
    let plugin = host.load(&Archive::parse(&bytes).unwrap().wasm).unwrap();
    assert_eq!(
        plugin.import_extensions().unwrap(),
        vec!["plt", "hpgl", "hgl"]
    );
    let plt = plugin.export(&hpgl_model()).unwrap();
    let paths = plugin.import(&plt).unwrap();
    golden(
        &golden_dir(),
        "hpgl-import.txt",
        dump_paths(&paths).as_bytes(),
    );
    // Same number of shapes as were exported.
    let exported: usize = hpgl_model()
        .layers
        .iter()
        .flat_map(|l| &l.paths)
        .map(|p| p.segments.len())
        .sum();
    let imported: usize = paths.iter().map(|p| p.segments.len()).sum();
    assert_eq!(imported, exported);

    let mut ed = locus_doc::Editor::new(locus_doc::Document::new());
    let cmd = locus_plugin_host::doc::import_command(ed.document(), "example.plt", &paths).unwrap();
    ed.apply(&cmd).unwrap();
    assert_eq!(ed.undo_label(), Some("Import example.plt"));

    // Not HP-GL: a structured error, not a crash.
    assert!(matches!(
        plugin.import(&[0xff, 0xfe, 0x00]),
        Err(locus_plugin_host::CallError::Plugin(
            PluginFailure::Unsupported(_)
        ))
    ));
    assert!(plugin.import(b"IN;SP1;PU10,10;SP0;").is_err());
}

/// #234: the kerf-check validator, on a Cut layer with a 0.2 mm kerf.
#[test]
fn kerf_check_golden() {
    let Some(core) = build_example("kerf-check") else {
        return;
    };
    let host = Host::new().unwrap();
    let bytes = pack(&host, &example_manifest("kerf-check"), &core, None, None).unwrap();
    let plugin = host.load(&Archive::parse(&bytes).unwrap().wasm).unwrap();
    assert_eq!(plugin.kinds(), vec![Kind::Validator]);
    let square = |x: f64, s: f64| Path {
        segments: vec![
            line(x, 0.0, x + s, 0.0),
            line(x + s, 0.0, x + s, s),
            line(x + s, s, x, s),
            line(x, s, x, 0.0),
        ],
        closed: true,
    };
    let circle = |r: f64| Path {
        segments: vec![Segment {
            shape: Shape::Circle {
                center: Vec2 { x: 50.0, y: 50.0 },
                radius: r,
            },
            reversed: false,
        }],
        closed: true,
    };
    let model = ExportModel {
        layers: vec![
            Layer {
                name: "Cut".into(),
                colour: 0xff0000,
                operation: Operation::Cut {
                    kerf_width: 0.2,
                    compensation: Compensation::None,
                    passes: 1,
                },
                paths: vec![square(0.0, 20.0), square(30.0, 0.4), circle(0.05)],
            },
            Layer {
                name: "Score".into(),
                colour: 0x0000ff,
                operation: Operation::Score,
                paths: vec![square(0.0, 0.1)],
            },
        ],
    };
    let schema = plugin.validator_schema().unwrap();
    let found = plugin.validate(&model, &defaults(&schema)).unwrap();
    let mut s = String::new();
    for f in &found {
        let _ = writeln!(
            s,
            "{:?} layer={:?} path={:?}: {}",
            f.severity, f.layer, f.path, f.message
        );
    }
    golden(&golden_dir(), "kerf-check.txt", s.as_bytes());
    assert_eq!(found.len(), 2);
}

#[test]
fn pack_refuses_what_install_would() {
    assert!(matches!(componentize(b"nope"), Err(PackError::NotWasm)));
    let Some(core) = build_example("plugin-template") else {
        return;
    };
    let host = Host::new().unwrap();
    // The template's component says "com.example.my-plugin"; this manifest doesn't.
    let wrong = example_manifest("plugin-template").replace("my-plugin", "other");
    assert!(matches!(
        pack(&host, &wrong, &core, None, None),
        Err(PackError::Check(_))
    ));
    // Packing an already-made component works too.
    let component = componentize(&core).unwrap();
    assert!(
        pack(
            &host,
            &example_manifest("plugin-template"),
            &component,
            None,
            None
        )
        .is_ok()
    );
}
