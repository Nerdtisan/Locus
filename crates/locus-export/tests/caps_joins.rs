// SPDX-License-Identifier: AGPL-3.0-or-later

//! Caps, joins and alignment in export (#428, ADR 0019 decision 4): SVG
//! writes them natively, page bounds include them, inside/outside strokes
//! are filled outlines, and a Styled machine layer cuts the outline.

#![allow(clippy::unwrap_used)]

use locus_doc::{
    Align, Cap, Command, Document, Editor, Geom, Join, LayerId, LayerProps, Operation,
    StrokeStyles, Style, StylePatch,
};
use locus_export::{prepare, prepare_for, to_svg_model};
use locus_geom::Vec2;

/// A 20 × 10 rectangle of four lines, 2 mm wide, on a new layer.
fn rect(op: Operation) -> (Editor, LayerId, Vec<locus_doc::EntityId>) {
    let mut ed = Editor::new(Document::new());
    let layer = ed
        .apply(&Command::AddLayer {
            name: "L".to_owned(),
            colour: [255, 0, 0],
            operation: op,
        })
        .unwrap()
        .layers[0];
    let mut pts = Vec::new();
    for (x, y) in [(0.0, 0.0), (20.0, 0.0), (20.0, 10.0), (0.0, 10.0)] {
        pts.push(
            ed.apply(&Command::AddEntity {
                layer,
                construction: false,
                style: Style::default(),
                geom: Geom::Point {
                    pos: Vec2::new(x, y),
                },
            })
            .unwrap()
            .entities[0],
        );
    }
    let style = Style {
        width: 2.0,
        ..Style::default()
    };
    let mut lines = Vec::new();
    for i in 0..4 {
        lines.push(
            ed.apply(&Command::AddEntity {
                layer,
                construction: false,
                style: style.clone(),
                geom: Geom::Line {
                    start: pts[i],
                    end: pts[(i + 1) % 4],
                },
            })
            .unwrap()
            .entities[0],
        );
    }
    (ed, layer, lines)
}

fn set(ed: &mut Editor, ids: &[locus_doc::EntityId], patch: StylePatch) {
    ed.apply(&Command::SetStyle {
        entities: ids.to_vec(),
        patch,
    })
    .unwrap();
}

#[test]
fn svg_writes_caps_joins_and_the_miter_limit() {
    let (mut ed, _, ids) = rect(Operation::Draw);
    set(
        &mut ed,
        &ids,
        StylePatch {
            cap: Some(Cap::Square),
            join: Some(Join::Miter { limit: 6.0 }),
            ..StylePatch::default()
        },
    );
    let svg = String::from_utf8(to_svg_model(&prepare(ed.document(), false).model)).unwrap();
    assert!(svg.contains("stroke-linecap=\"square\""), "{svg}");
    assert!(
        svg.contains("stroke-linejoin=\"miter\" stroke-miterlimit=\"6\""),
        "{svg}"
    );
}

#[test]
fn page_bounds_include_miters() {
    let (mut ed, _, ids) = rect(Operation::Draw);
    let round = prepare(ed.document(), false).model.visual_bounds().unwrap();
    set(
        &mut ed,
        &ids,
        StylePatch {
            join: Some(Join::Miter { limit: 10.0 }),
            ..StylePatch::default()
        },
    );
    let miter = prepare(ed.document(), false).model.visual_bounds().unwrap();
    // Round: the rectangle ± 1 mm. Mitred square corners reach the same box.
    assert!((round.min.x + 1.0).abs() < 1e-9 && (round.max.y - 11.0).abs() < 1e-9);
    assert!((miter.min.x + 1.0).abs() < 1e-9 && (miter.max.x - 21.0).abs() < 1e-9);
}

#[test]
fn an_inside_stroke_exports_as_a_filled_outline() {
    let (mut ed, _, ids) = rect(Operation::Draw);
    set(
        &mut ed,
        &ids,
        StylePatch {
            align: Some(Align::Inside),
            ..StylePatch::default()
        },
    );
    let svg = String::from_utf8(to_svg_model(&prepare(ed.document(), false).model)).unwrap();
    assert!(svg.contains("fill=\"#ff0000\" stroke=\"none\""), "{svg}");
    assert!(!svg.contains("stroke-width=\"2\""), "{svg}");
}

#[test]
fn a_widened_plain_line_on_a_styled_layer_cuts_its_centreline_once() {
    // Owner decision 2026-11-25: width, caps and joins alone don't make an
    // object follow its outline on a Styled layer; only line styles do.
    let (mut ed, layer, ids) = rect(Operation::Cut(Default::default()));
    set(
        &mut ed,
        &ids,
        StylePatch {
            join: Some(Join::Miter { limit: 4.0 }),
            ..StylePatch::default()
        },
    );
    ed.apply(&Command::SetLayerProps {
        id: layer,
        props: LayerProps {
            stroke_styles: Some(StrokeStyles::Styled),
            ..LayerProps::default()
        },
    })
    .unwrap();
    let lb = locus_export::profile::find("lightburn", &[]).unwrap();
    let styled = prepare_for(ed.document(), false, Some(&lb));
    let paths = &styled.model.layers[0].paths;
    assert_eq!(paths.len(), 1, "one centreline cut");
    let bb = paths[0].bbox().unwrap();
    assert!(bb.min.x.abs() < 1e-9 && (bb.max.x - 20.0).abs() < 1e-9);
}

fn dash(pattern: &[f64], phase: f64, align: locus_doc::DashAlign) -> StylePatch {
    StylePatch {
        dash: Some(Some(locus_doc::Dash::new(pattern, phase, align).unwrap())),
        ..StylePatch::default()
    }
}

#[test]
fn exact_dashes_are_native_in_svg_and_pdf() {
    let (mut ed, _, ids) = rect(Operation::Draw);
    set(
        &mut ed,
        &ids,
        dash(&[3.0, 2.0], 1.0, locus_doc::DashAlign::Exact),
    );
    let model = prepare(ed.document(), false).model;
    let svg = String::from_utf8(to_svg_model(&model)).unwrap();
    assert!(
        svg.contains("stroke-dasharray=\"3 2\" stroke-dashoffset=\"1\""),
        "{svg}"
    );
    let pdf = String::from_utf8_lossy(&locus_export::to_pdf_model(&model)).into_owned();
    assert!(pdf.contains(" d\n"), "PDF sets a dash");
}

#[test]
fn corner_dashes_and_dxf_write_one_path_per_dash() {
    let (mut ed, _, ids) = rect(Operation::Draw);
    // 60 mm round the rectangle; corners aligned: a dash on every corner.
    set(
        &mut ed,
        &ids,
        dash(&[2.0, 2.0], 0.0, locus_doc::DashAlign::Corners),
    );
    let model = prepare(ed.document(), false).model;
    let svg = String::from_utf8(to_svg_model(&model)).unwrap();
    assert!(!svg.contains("stroke-dasharray"), "{svg}");
    let paths = svg.matches("<path").count();
    assert!(paths > 4, "{paths} paths: {svg}");
    // DXF: every dash its own entity, exact or not.
    let (mut ed2, _, ids2) = rect(Operation::Draw);
    set(
        &mut ed2,
        &ids2,
        dash(&[3.0, 2.0], 0.0, locus_doc::DashAlign::Exact),
    );
    let dxf = String::from_utf8(locus_export::to_dxf_model(
        &prepare(ed2.document(), false).model,
    ))
    .unwrap();
    // 60 mm, period 5: 12 dashes, the ones over a corner as two lines each.
    let lines = dxf.matches("\nLINE\n").count();
    assert!(lines >= 12, "{lines} lines");
}

#[test]
fn a_styled_score_layer_perforates_dashes() {
    let (mut ed, layer, ids) = rect(Operation::Score);
    set(
        &mut ed,
        &ids,
        dash(&[3.0, 2.0], 0.0, locus_doc::DashAlign::Exact),
    );
    ed.apply(&Command::SetLayerProps {
        id: layer,
        props: LayerProps {
            stroke_styles: Some(StrokeStyles::Styled),
            ..LayerProps::default()
        },
    })
    .unwrap();
    let lb = locus_export::profile::find("lightburn", &[]).unwrap();
    let p = prepare_for(ed.document(), false, Some(&lb));
    let paths = &p.model.layers[0].paths;
    assert_eq!(paths.len(), 12, "one cut per dash");
    assert!(paths.iter().all(|x| !x.closed));
}

fn extra(layers: Vec<locus_doc::StrokeLayer>) -> StylePatch {
    StylePatch {
        extra: Some(layers),
        ..StylePatch::default()
    }
}

#[test]
fn extra_strokes_are_their_own_svg_paths() {
    let (mut ed, _, ids) = rect(Operation::Draw);
    set(
        &mut ed,
        &ids,
        extra(vec![locus_doc::StrokeLayer {
            offset: -3.0,
            width: 0.5,
            colour: Some([0, 0, 255]),
            pen: Some(3),
            ..locus_doc::StrokeLayer::default()
        }]),
    );
    let model = prepare(ed.document(), false).model;
    let svg = String::from_utf8(to_svg_model(&model)).unwrap();
    // The rectangle runs counter-clockwise, so -3 is outside: a second,
    // blue path with pen 3, round-cornered 26 × 16.
    assert!(
        svg.contains("stroke=\"#0000ff\" stroke-width=\"0.5\""),
        "{svg}"
    );
    assert!(svg.contains("data-pen=\"3\""), "{svg}");
    let b = model.visual_bounds().unwrap();
    assert!((b.min.x + 3.25).abs() < 1e-9, "{b:?}");
}

#[test]
fn a_styled_cut_layer_cuts_every_stroke_in_parallel() {
    let (mut ed, layer, ids) = rect(Operation::Cut(Default::default()));
    set(
        &mut ed,
        &ids,
        extra(vec![
            locus_doc::StrokeLayer {
                offset: 1.0,
                ..locus_doc::StrokeLayer::default()
            },
            locus_doc::StrokeLayer {
                offset: -1.0,
                ..locus_doc::StrokeLayer::default()
            },
        ]),
    );
    ed.apply(&Command::SetLayerProps {
        id: layer,
        props: LayerProps {
            stroke_styles: Some(StrokeStyles::Styled),
            ..LayerProps::default()
        },
    })
    .unwrap();
    let lb = locus_export::profile::find("lightburn", &[]).unwrap();
    let p = prepare_for(ed.document(), false, Some(&lb));
    // The path, 1 mm inside and 1 mm outside: three closed cuts.
    assert_eq!(p.model.layers[0].paths.len(), 3);
    assert!(p.model.layers[0].paths.iter().all(|x| x.closed));
}

fn taper() -> StylePatch {
    StylePatch {
        profile: Some(Some(locus_doc::ProfilePreset::TaperBoth.profile(0))),
        ..StylePatch::default()
    }
}

#[test]
fn a_width_profile_exports_as_a_filled_outline_of_curves() {
    // #426: no format strokes a variable width, so SVG, PDF and PNG fill the
    // outline; its tapered sides are fitted cubics (ADR 0019 option (b)).
    let (mut ed, _, ids) = rect(Operation::Draw);
    set(&mut ed, &ids, taper());
    let model = prepare(ed.document(), false).model;
    let svg = String::from_utf8(to_svg_model(&model)).unwrap();
    assert!(svg.contains("fill=\"#ff0000\" stroke=\"none\""), "{svg}");
    assert!(svg.contains(" C "), "{svg}");
    assert!(!svg.contains("stroke-width=\"2\""), "{svg}");
    let pdf = locus_export::to_pdf_model(&model);
    assert!(!pdf.is_empty());
    // The bulge of a 2 mm stroke stays inside the page bounds.
    let b = model.visual_bounds().unwrap();
    assert!(b.min.x <= -0.999 && b.max.y >= 10.999, "{b:?}");
}

#[test]
fn a_styled_cut_layer_cuts_a_profiles_outline() {
    let (mut ed, layer, ids) = rect(Operation::Cut(Default::default()));
    set(&mut ed, &ids, taper());
    ed.apply(&Command::SetLayerProps {
        id: layer,
        props: LayerProps {
            stroke_styles: Some(StrokeStyles::Styled),
            ..LayerProps::default()
        },
    })
    .unwrap();
    let lb = locus_export::profile::find("lightburn", &[]).unwrap();
    let styled = prepare_for(ed.document(), false, Some(&lb));
    let paths = &styled.model.layers[0].paths;
    // The ring's outline, not the centreline: closed, and past the path.
    assert!(paths.iter().all(|p| p.closed));
    let bb = paths
        .iter()
        .filter_map(|p| p.bbox())
        .reduce(locus_geom::Aabb::union)
        .unwrap();
    assert!(bb.min.x < -0.5 && bb.max.y > 10.5, "{bb:?}");
}

#[test]
fn a_this_curve_profile_exports_each_side_as_its_own_outline() {
    // #527: connected lines with one profile are one stroke, one chained
    // path; with "This curve" each side strokes alone, four outlines.
    let paths = |alone: bool| {
        let (mut ed, _, ids) = rect(Operation::Draw);
        let mut p = locus_doc::ProfilePreset::TaperBoth.profile(0);
        p.alone = alone;
        set(
            &mut ed,
            &ids,
            StylePatch {
                profile: Some(Some(p)),
                ..StylePatch::default()
            },
        );
        prepare(ed.document(), false).model.layers[0].paths.len()
    };
    let (chained, alone) = (paths(false), paths(true));
    assert_eq!(alone, 4, "one outline per side");
    assert!(chained < alone, "{chained} chained vs {alone} alone");
}
