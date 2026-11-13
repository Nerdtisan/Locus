// SPDX-License-Identifier: AGPL-3.0-or-later

//! Modifier plugins (#184, §15.2): the runtime call, and the registry runner
//! the app hands the document so modifier objects re-run when their sources
//! change.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use common::Fixture;
use locus_doc::generated::{GenEntity, PluginRunner, input_hash, is_stale, modifier_input};
use locus_doc::{Command, Document, Editor, Geom, RecipeKind};
use locus_geom::Vec2;
use locus_plugin_host::archive::Archive;
use locus_plugin_host::doc::{paths_to_cache, plugin_ref, to_paths};
use locus_plugin_host::registry::Registry;
use locus_plugin_host::runner::RegistryRunner;
use locus_plugin_host::types::{Kind, Path, Segment, Shape, Vec2 as HVec2};
use locus_plugin_host::{CallError, Host};
use std::sync::Arc;

const MANIFEST: &str = "id = \"com.example.identity\"\nname = \"Identity\"\nversion = \"1.0.0\"\n\
    author = \"Tests\"\nlicence = \"MIT\"\nkinds = [\"modifier\"]\napi = \"1.0.0\"\n";

fn paths() -> Vec<Path> {
    let p = |x: f64, y: f64| HVec2 { x, y };
    let line = |a, b| Segment {
        shape: Shape::Line { a, b },
        reversed: false,
    };
    vec![Path {
        segments: vec![
            line(p(0.0, 0.0), p(5.0, 0.0)),
            line(p(5.0, 0.0), p(5.0, 5.0)),
            Segment {
                shape: Shape::Arc {
                    center: p(5.0, 0.0),
                    start: p(5.0, 5.0),
                    end: p(0.0, 0.0),
                },
                reversed: true,
            },
        ],
        closed: true,
    }]
}

#[test]
fn identity_modifier_round_trips_paths() {
    let host = Host::new().unwrap();
    let plugin = host.load(&Fixture::modifier_identity().build()).unwrap();
    assert_eq!(plugin.kinds(), vec![Kind::Modifier]);
    assert!(plugin.modifier_schema().unwrap().is_empty());
    assert_eq!(plugin.modify(&paths(), &[]).unwrap(), paths());
    assert!(matches!(
        plugin.generate(&[]),
        Err(CallError::NotProvided(_))
    ));
}

#[test]
fn output_becomes_a_connected_cache() {
    let g = paths_to_cache(&paths());
    // (0,0), (5,0), (5,5): end points and the arc's centre are shared.
    assert_eq!(g.points.len(), 3);
    assert_eq!(g.entities.len(), 3);
    assert!(matches!(g.entities[2], GenEntity::Arc(..)));
    g.validate().unwrap();
}

#[test]
fn registry_runner_reruns_an_installed_modifier() {
    let host = Host::new().unwrap();
    let dir = std::env::temp_dir().join(format!("locus-modifier-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let reg = Registry::new(&dir);
    reg.install(
        &host,
        &Archive::write(MANIFEST, &Fixture::modifier_identity().build(), None, None).unwrap(),
    )
    .unwrap();

    let mut ed = Editor::new(Document::new());
    ed.set_plugin_runner(Some(Arc::new(RegistryRunner::new(Registry::new(&dir)))));
    let mut pt = |x: f64, y: f64| {
        ed.apply(&Command::AddEntity {
            layer: Document::default_layer(),
            construction: false,
            style: Default::default(),
            geom: Geom::Point {
                pos: Vec2::new(x, y),
            },
        })
        .unwrap()
        .entities[0]
    };
    let (a, b) = (pt(0.0, 0.0), pt(10.0, 0.0));
    let line = ed
        .apply(&Command::AddEntity {
            layer: Document::default_layer(),
            construction: false,
            style: Default::default(),
            geom: Geom::Line { start: a, end: b },
        })
        .unwrap()
        .entities[0];

    // What the app does: run once, store the result with the input's hash.
    let (_, plugin) = reg.load(&host, "com.example.identity").unwrap();
    let input = modifier_input(ed.document(), &[line]).unwrap();
    let out = plugin.modify(&to_paths(&input), &[]).unwrap();
    let rid = ed
        .apply(&Command::AddRecipe {
            kind: RecipeKind::Modified {
                plugin: plugin_ref("com.example.identity", "1.0.0", &[]),
                input_hash: input_hash(&input),
                output: paths_to_cache(&out),
            },
            sources: vec![line],
        })
        .unwrap()
        .recipes[0];

    // Moving the source re-runs the installed plugin in the same step.
    ed.apply(&Command::ModifyEntity {
        id: b,
        geom: Geom::Point {
            pos: Vec2::new(25.0, 3.0),
        },
    })
    .unwrap();
    assert!(!is_stale(ed.document(), rid));
    let r = ed.document().recipe(rid).unwrap();
    let RecipeKind::Modified { output, .. } = &r.kind else {
        panic!("not a modifier")
    };
    assert!(output.points.contains(&Vec2::new(25.0, 3.0)));

    // A different stored version is never silently re-run (§15.5).
    let runner = RegistryRunner::new(Registry::new(&dir));
    let old = plugin_ref("com.example.identity", "0.9.0", &[]);
    assert!(runner.modify(&old, &input).is_err());
    let _ = std::fs::remove_dir_all(&dir);
}
