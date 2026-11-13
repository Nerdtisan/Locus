// SPDX-License-Identifier: AGPL-3.0-or-later

//! `locus-cli plugin …` integration tests (SPEC §15.7, §15.8, #185).
#![allow(clippy::unwrap_used, clippy::expect_used)]

#[path = "../../locus-plugin-host/tests/common/examples.rs"]
mod examples;

use std::path::PathBuf;
use std::process::{Command, Output};

use examples::{build_example, repo_root};

fn cli(args: &[&std::ffi::OsStr]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_locus-cli"))
        .args(args)
        .output()
        .unwrap()
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("locus-cli-plugin-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(name)
}

/// Builds `examples/<name>` and packs it with the CLI; `None` without wasm32.
pub fn packed(name: &str) -> Option<PathBuf> {
    let core = build_example(name)?;
    let wasm = scratch(&format!("{name}.wasm"));
    std::fs::write(&wasm, core).unwrap();
    let out = scratch(&format!("{name}.locusplugin"));
    let manifest = repo_root().join("examples").join(name).join("plugin.toml");
    let readme = repo_root().join("examples").join(name).join("README.md");
    let res = cli(&[
        "plugin".as_ref(),
        "pack".as_ref(),
        "--manifest".as_ref(),
        manifest.as_os_str(),
        "--wasm".as_ref(),
        wasm.as_os_str(),
        "--readme".as_ref(),
        readme.as_os_str(),
        "--out".as_ref(),
        out.as_os_str(),
    ]);
    assert!(
        res.status.success(),
        "pack failed: {}",
        String::from_utf8_lossy(&res.stderr)
    );
    Some(out)
}

#[test]
fn pack_builds_the_template() {
    let Some(file) = packed("plugin-template") else {
        return;
    };
    let bytes = std::fs::read(file).unwrap();
    let archive = locus_plugin_host::archive::Archive::parse(&bytes).unwrap();
    assert_eq!(archive.manifest.id, "com.example.my-plugin");
    assert!(archive.readme.is_some());
}

#[test]
fn pack_usage_errors() {
    let out = cli(&["plugin".as_ref(), "pack".as_ref()]);
    assert_eq!(out.status.code(), Some(2));
    let out = cli(&["plugin".as_ref(), "frobnicate".as_ref()]);
    assert_eq!(out.status.code(), Some(2));
}

#[test]
fn pack_refuses_garbage() {
    let wasm = scratch("garbage.wasm");
    std::fs::write(&wasm, b"not wasm").unwrap();
    let manifest = repo_root().join("examples/plugin-template/plugin.toml");
    let out_file = scratch("garbage.locusplugin");
    let out = cli(&[
        "plugin".as_ref(),
        "pack".as_ref(),
        "--manifest".as_ref(),
        manifest.as_os_str(),
        "--wasm".as_ref(),
        wasm.as_os_str(),
        "--out".as_ref(),
        out_file.as_os_str(),
    ]);
    assert_eq!(out.status.code(), Some(1));
    assert!(!out_file.exists());
}

// ── Exporter plugins (#184) ─────────────────────────────────────────────────

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

#[test]
fn hpgl_plugin_exports_the_plate_golden() {
    let Some(hpgl) = packed("hpgl") else {
        return;
    };
    let out_file = scratch("plate.plt");
    let input = fixture("plate.locus");
    let out = cli(&[
        "export".as_ref(),
        input.as_os_str(),
        "--format".as_ref(),
        "hpgl".as_ref(),
        "--plugin".as_ref(),
        hpgl.as_os_str(),
        "--out".as_ref(),
        out_file.as_os_str(),
    ]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let bytes = std::fs::read(&out_file).unwrap();
    examples::golden(&fixture(""), "plate.plt", &bytes);
    let text = String::from_utf8(bytes).unwrap();
    assert!(text.starts_with("IN;\n") && text.ends_with("SP0;\n"));
}

#[test]
fn plugin_formats_need_their_plugin() {
    let input = fixture("plate.locus");
    let out_file = scratch("nope.plt");
    let out = cli(&[
        "export".as_ref(),
        input.as_os_str(),
        "--format".as_ref(),
        "hpgl".as_ref(),
        "--out".as_ref(),
        out_file.as_os_str(),
    ]);
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("svg, pdf, dxf"));
}

// ── plugin run (#187) ───────────────────────────────────────────────────────

fn run(args: &[&std::ffi::OsStr]) -> Output {
    let mut all: Vec<&std::ffi::OsStr> = vec!["plugin".as_ref(), "run".as_ref()];
    all.extend_from_slice(args);
    cli(&all)
}

#[test]
fn run_finger_box_matches_golden() {
    let Some(fb) = packed("finger-box") else {
        return;
    };
    let params = fixture("finger-box.params.ron");
    let out_file = scratch("finger-box.svg");
    let out = run(&[
        fb.as_os_str(),
        "--params".as_ref(),
        params.as_os_str(),
        "--out".as_ref(),
        out_file.as_os_str(),
    ]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let svg = std::fs::read(&out_file).unwrap();
    examples::golden(&fixture(""), "finger-box.svg", &svg);
    assert!(String::from_utf8_lossy(&svg).contains("<svg"));

    // Defaults only, and DXF by extension.
    let dxf = scratch("finger-box.dxf");
    let out = run(&[fb.as_os_str(), "--out".as_ref(), dxf.as_os_str()]);
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&std::fs::read(&dxf).unwrap()).contains("SECTION"));
}

#[test]
fn run_rejects_bad_params_and_non_generators() {
    let (Some(fb), Some(hpgl)) = (packed("finger-box"), packed("hpgl")) else {
        return;
    };
    let out_file = scratch("bad.svg");
    for (name, text) in [
        ("unknown.ron", "(colour: 3)"),
        ("type.ron", "(lid: 3.0)"),
        ("range.ron", "(thickness: 99.0)"),
        ("syntax.ron", "(width: "),
        ("notmap.ron", "[1, 2]"),
    ] {
        let p = scratch(name);
        std::fs::write(&p, text).unwrap();
        let out = run(&[
            fb.as_os_str(),
            "--params".as_ref(),
            p.as_os_str(),
            "--out".as_ref(),
            out_file.as_os_str(),
        ]);
        assert_eq!(out.status.code(), Some(1), "{name}");
        assert!(!out_file.exists(), "{name}");
    }
    // An exporter is not a generator.
    let out = run(&[hpgl.as_os_str(), "--out".as_ref(), out_file.as_os_str()]);
    assert_eq!(out.status.code(), Some(1));
    // Usage errors.
    assert_eq!(run(&[]).status.code(), Some(2));
    assert_eq!(run(&[fb.as_os_str()]).status.code(), Some(2));
    let txt = scratch("x.txt");
    assert_eq!(
        run(&[fb.as_os_str(), "--out".as_ref(), txt.as_os_str()])
            .status
            .code(),
        Some(2)
    );
}

#[test]
fn run_by_id_needs_a_plugins_folder() {
    let out_file = scratch("id.svg");
    let out = run(&[
        "io.locus.finger-box".as_ref(),
        "--out".as_ref(),
        out_file.as_os_str(),
    ]);
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("--plugins"));
}

// ── Validator plugins (#234) ────────────────────────────────────────────────

/// A Cut layer with a 0.4 mm kerf holding a 0.1 mm-radius hole (too small).
fn kerf_doc() -> PathBuf {
    use locus_doc::{Command as C, CutParams, Document, Editor, EntityId, Geom, Operation};
    let mut ed = Editor::new(Document::new());
    let layer = ed
        .apply(&C::AddLayer {
            name: "Cut".into(),
            colour: [255, 0, 0],
            operation: Operation::Cut(CutParams {
                kerf_width: 0.4,
                ..CutParams::default()
            }),
        })
        .unwrap()
        .layers[0];
    let center: EntityId = ed
        .apply(&C::AddEntity {
            layer,
            construction: false,
            style: Default::default(),
            geom: Geom::Point {
                pos: locus_geom::Vec2::new(5.0, 5.0),
            },
        })
        .unwrap()
        .entities[0];
    ed.apply(&C::AddEntity {
        layer,
        construction: false,
        style: Default::default(),
        geom: Geom::Circle {
            center,
            radius: 0.1,
        },
    })
    .unwrap();
    let file = scratch("kerf.locus");
    std::fs::write(&file, locus_doc::file::save(ed.document()).unwrap()).unwrap();
    file
}

#[test]
fn validator_plugin_findings_join_validate_and_block_export() {
    let Some(kerf) = packed("kerf-check") else {
        return;
    };
    let doc = kerf_doc();
    // Without the plugin, the built-in checks pass.
    let out = cli(&["validate".as_ref(), doc.as_os_str()]);
    assert_eq!(out.status.code(), Some(0));

    let out = cli(&[
        "validate".as_ref(),
        doc.as_os_str(),
        "--plugin".as_ref(),
        kerf.as_os_str(),
    ]);
    let text = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(1), "{text}");
    assert!(text.contains("error (Kerf check)"), "{text}");
    assert!(text.contains("1 error(s)"), "{text}");

    let svg = scratch("kerf.svg");
    let export = |extra: &[&std::ffi::OsStr]| {
        let mut args: Vec<&std::ffi::OsStr> = vec![
            "export".as_ref(),
            doc.as_os_str(),
            "--format".as_ref(),
            "svg".as_ref(),
            "--out".as_ref(),
            svg.as_os_str(),
            "--plugin".as_ref(),
            kerf.as_os_str(),
        ];
        args.extend_from_slice(extra);
        cli(&args)
    };
    assert_eq!(export(&[]).status.code(), Some(1));
    assert!(!svg.exists());
    assert_eq!(export(&["--force".as_ref()]).status.code(), Some(0));
    assert!(svg.exists());
}
