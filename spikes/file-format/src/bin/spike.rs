// SPDX-License-Identifier: AGPL-3.0-or-later

//! Writes the sample outputs under `samples/` and prints the measurements
//! used by ADR-0003 and ADR-0004. Run from the spike directory.

use file_format_spike::*;
use std::fmt::Write as _;
use std::fs;
use uuid::Uuid;

type Res<T> = Result<T, Box<dyn std::error::Error>>;

fn edit<Id: Ord + Clone>(doc: &mut Document<Id>, ids: &[Id]) {
    // A typical small edit: change the width parameter (re-solve moves two
    // points), add one construction point and one constraint.
    if let Some(p) = doc.parameters.get_mut("width") {
        p.expr = "65 mm".into();
        p.value = 65.0;
    }
    for i in [3, 4] {
        if let Some(Entity {
            geom: Geom::Point { x, .. },
            ..
        }) = doc.entities.get_mut(&ids[i])
        {
            *x = 65.0;
        }
    }
    if let Some(Constraint::Distance {
        value: Dim::Expr { value, .. },
        ..
    }) = doc.constraints.get_mut(&ids[17])
    {
        *value = 65.0;
    }
    let p = Entity {
        layer: ids[1].clone(),
        construction: true,
        geom: Geom::Point { x: 32.5, y: 20.0 },
    };
    doc.entities.insert(ids[SAMPLE_IDS].clone(), p);
    doc.constraints.insert(
        ids[SAMPLE_IDS + 1].clone(),
        Constraint::Coincident(ids[SAMPLE_IDS].clone(), ids[10].clone()),
    );
    doc.next_id += 2;
}

fn roundtrip_ron(v: f64) -> Res<f64> {
    Ok(ron::from_str(&to_ron(&v)?)?)
}
fn roundtrip_json(v: f64) -> Res<f64> {
    Ok(serde_json::from_str(&to_json(&v)?)?)
}

fn main() -> Res<()> {
    let mut out = String::new();
    let u64_ids: Vec<u64> = (1..=600).collect();
    // UUIDv7s are time-ordered; generated fresh each run, so only their
    // sizes (fixed width) are reproducible, not their text.
    let uuid_ids: Vec<Uuid> = (0..600).map(|_| Uuid::now_v7()).collect();

    // --- Sample outputs (u64 IDs) and the edit diff input ---
    let doc = sample(&u64_ids);
    let mut edited = doc.clone();
    edit(&mut edited, &u64_ids);
    fs::write("samples/sample.ron", to_ron(&doc)? + "\n")?;
    fs::write("samples/sample.json", to_json(&doc)? + "\n")?;
    fs::write("samples/sample-edited.ron", to_ron(&edited)? + "\n")?;
    fs::write("samples/sample-edited.json", to_json(&edited)? + "\n")?;

    let udoc = sample(&uuid_ids);
    fs::write("samples/sample-uuid.ron", to_ron(&udoc)? + "\n")?;
    fs::write("samples/sample-uuid.json", to_json(&udoc)? + "\n")?;

    // --- Determinism: same doc twice, and write -> read -> write ---
    let ron1 = to_ron(&doc)?;
    let json1 = to_json(&doc)?;
    let ron_back: Document<u64> = ron::from_str(&ron1)?;
    let json_back: Document<u64> = serde_json::from_str(&json1)?;
    writeln!(out, "## Determinism\n")?;
    writeln!(out, "| Check | RON | JSON |\n|---|---|---|")?;
    writeln!(
        out,
        "| serialise twice identical | {} | {} |",
        ron1 == to_ron(&doc)?,
        json1 == to_json(&doc)?
    )?;
    writeln!(
        out,
        "| write -> read -> write byte-identical | {} | {} |",
        ron1 == to_ron(&ron_back)?,
        json1 == to_json(&json_back)?
    )?;
    writeln!(
        out,
        "| read-back equals original | {} | {} |\n",
        ron_back == doc,
        json_back == doc
    )?;

    // --- f64 round-trip ---
    writeln!(out, "## f64 round-trip (bit-exact)\n")?;
    writeln!(
        out,
        "| Value | RON text | RON ok | JSON text | JSON ok |\n|---|---|---|---|---|"
    )?;
    for v in TRICKY_F64 {
        let r = roundtrip_ron(v)
            .map(|b| b.to_bits() == v.to_bits())
            .unwrap_or(false);
        let j = roundtrip_json(v)
            .map(|b| b.to_bits() == v.to_bits())
            .unwrap_or(false);
        writeln!(
            out,
            "| `{v:e}` | `{}` | {r} | `{}` | {j} |",
            to_ron(&v)?,
            to_json(&v)?
        )?;
    }
    for (name, v) in [("NaN", f64::NAN), ("+inf", f64::INFINITY)] {
        let r = match roundtrip_ron(v) {
            Ok(b) => {
                if b.is_nan() == v.is_nan() && (v.is_nan() || b == v) {
                    "true"
                } else {
                    "false"
                }
            }
            Err(_) => "parse error",
        };
        let j = match roundtrip_json(v) {
            Ok(_) => "true",
            Err(_) => "parse error",
        };
        writeln!(
            out,
            "| {name} | `{}` | {r} | `{}` | {j} |",
            to_ron(&v)?,
            to_json(&v)?
        )?;
    }

    // --- Sizes ---
    let grid_u = grid(&u64_ids);
    let grid_uu = grid(&uuid_ids);
    writeln!(out, "\n## File size (bytes, pretty-printed)\n")?;
    writeln!(out, "| Document | RON + u64 | JSON + u64 | RON + UUIDv7 | JSON + UUIDv7 |\n|---|---|---|---|---|")?;
    writeln!(
        out,
        "| sample ({} entities) | {} | {} | {} | {} |",
        doc.entities.len(),
        to_ron(&doc)?.len(),
        to_json(&doc)?.len(),
        to_ron(&udoc)?.len(),
        to_json(&udoc)?.len()
    )?;
    writeln!(
        out,
        "| grid ({} entities) | {} | {} | {} | {} |",
        grid_u.entities.len(),
        to_ron(&grid_u)?.len(),
        to_json(&grid_u)?.len(),
        to_ron(&grid_uu)?.len(),
        to_json(&grid_uu)?.len()
    )?;

    print!("{out}");
    fs::write(
        "samples/results.md",
        format!("# Spike results (generated by `src/bin/spike.rs`)\n\n{out}"),
    )?;
    Ok(())
}
