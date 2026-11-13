// SPDX-License-Identifier: AGPL-3.0-or-later

//! Per-layer export (#291; SPEC §8, §7): one file per exported layer, all on
//! the same origin and canvas. Built from the `layers.locus` fixture, which
//! has Cut, Score and Vinyl layers that export, an engrave layer with export
//! off and a Reference layer that never does.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use locus_export::{
    LayerFile, file_names, prepare, split_layers, to_dxf_model, to_pdf_model, to_svg_model, zip,
};
use std::path::PathBuf;

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

fn files() -> (Vec<LayerFile>, locus_export::ExportModel) {
    let text = std::fs::read_to_string(fixture("layers.locus")).unwrap();
    let doc = locus_doc::file::load(&text).unwrap();
    let model = prepare(&doc, false).model;
    (split_layers(&model), model)
}

fn golden(name: &str, got: &[u8]) {
    let path = fixture("per-layer").join(name);
    if std::env::var_os("LOCUS_UPDATE_GOLDEN").is_some() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, got).unwrap();
    }
    let want = std::fs::read(&path).unwrap_or_else(|_| panic!("missing golden {name}"));
    let norm = |b: &[u8]| String::from_utf8_lossy(b).replace("\r\n", "\n");
    assert_eq!(norm(got), norm(&want), "{name} differs");
}

#[test]
fn exported_layers_only_each_get_a_file_named_after_them() {
    let (fs, _) = files();
    let names = file_names("layers", &fs, "svg");
    assert_eq!(
        names,
        ["layers-Layer 1.svg", "layers-Vinyl.svg", "layers-Score.svg"],
        "export order; engrave (export off) and Reference skipped"
    );
}

#[test]
fn every_file_shares_the_whole_designs_canvas() {
    let (fs, whole) = files();
    let want = whole.bounds().unwrap();
    for f in &fs {
        assert_eq!(f.model.bounds(), Some(want), "{}", f.layer_name);
        assert_eq!(f.model.layers.len(), 1);
    }
    // The same size line in the SVG header of every file and of the whole.
    let header = |svg: &[u8]| {
        String::from_utf8_lossy(svg)
            .lines()
            .find(|l| l.contains("viewBox"))
            .unwrap()
            .to_owned()
    };
    let whole_header = header(&to_svg_model(&whole));
    for f in &fs {
        assert_eq!(header(&to_svg_model(&f.model)), whole_header);
    }
}

#[test]
fn per_layer_files_match_golden() {
    let (fs, _) = files();
    for (f, name) in fs.iter().zip(file_names("layers", &fs, "svg")) {
        golden(&name, &to_svg_model(&f.model));
    }
    for (f, name) in fs.iter().zip(file_names("layers", &fs, "dxf")) {
        golden(&name, &to_dxf_model(&f.model));
    }
    // PDF carries a timestamp-free body, so its bytes are stable too.
    for (f, name) in fs.iter().zip(file_names("layers", &fs, "pdf")) {
        golden(&name, &to_pdf_model(&f.model));
    }
}

#[test]
fn zip_holds_every_file_with_its_bytes() {
    let (fs, _) = files();
    let named: Vec<(String, Vec<u8>)> = fs
        .iter()
        .zip(file_names("layers", &fs, "svg"))
        .map(|(f, n)| (n, to_svg_model(&f.model)))
        .collect();
    let z = zip(&named).unwrap();
    assert_eq!(&z[..4], b"PK\x03\x04");
    for (name, bytes) in &named {
        let find = |needle: &[u8]| z.windows(needle.len()).any(|w| w == needle);
        assert!(find(name.as_bytes()) && find(bytes));
    }
}
