// SPDX-License-Identifier: AGPL-3.0-or-later

//! Throwaway spike for ADR-0004 (RON vs JSON for `.locus`, SPEC §10) and
//! ADR-0003 (entity IDs, SPEC §4). A cut-down document model, just rich
//! enough to show what each format looks like on disk. Not production code.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Generic over the ID type so the same document can be written with
/// `u64` or UUIDv7 IDs (ADR-0003). `BTreeMap` gives the stable ordering
/// SPEC §10 asks for, independent of insertion order.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Document<Id: Ord> {
    pub format_version: u32,
    pub units: Units,
    pub next_id: u64,
    pub parameters: BTreeMap<String, Parameter>,
    pub layers: BTreeMap<Id, Layer>,
    pub entities: BTreeMap<Id, Entity<Id>>,
    pub constraints: BTreeMap<Id, Constraint<Id>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum Units {
    Mm,
    Inch,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Parameter {
    pub expr: String,
    pub value: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Layer {
    pub name: String,
    pub operation: Operation,
    pub colour: [u8; 3],
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum Operation {
    Cut,
    Score,
    Engrave,
    Draw,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Entity<Id> {
    pub layer: Id,
    pub construction: bool,
    pub geom: Geom<Id>,
}

/// Solved positions are stored alongside the constraints (SPEC §10).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Geom<Id> {
    Point { x: f64, y: f64 },
    Line { start: Id, end: Id },
    Circle { centre: Id, radius: f64 },
    Arc { centre: Id, start: Id, end: Id },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Constraint<Id> {
    Coincident(Id, Id),
    Horizontal(Id),
    Vertical(Id),
    Distance { a: Id, b: Id, value: Dim },
    Radius { circle: Id, value: Dim },
}

/// A dimension: either a literal or an expression over document parameters.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Dim {
    Value(f64),
    Expr { expr: String, value: f64 },
}

/// Hand-built sample: a 60 × 40 plate (4 points, 4 lines) with a Ø10 hole,
/// on a cut layer, plus one construction point on a score layer.
/// `ids` maps sample slot 0..n to whatever ID type is being tested.
pub fn sample<Id: Ord + Clone>(ids: &[Id]) -> Document<Id> {
    let id = |i: usize| ids[i].clone();
    let pt = |x, y| Geom::Point { x, y };
    let mut entities = BTreeMap::new();
    let cut = id(0);
    let score = id(1);
    let corners = [(0.0, 0.0), (60.0, 0.0), (60.0, 40.0), (0.0, 40.0)];
    for (i, (x, y)) in corners.into_iter().enumerate() {
        entities.insert(id(2 + i), ent(&cut, false, pt(x, y)));
    }
    for i in 0..4 {
        let g = Geom::Line {
            start: id(2 + i),
            end: id(2 + (i + 1) % 4),
        };
        entities.insert(id(6 + i), ent(&cut, false, g));
    }
    entities.insert(id(10), ent(&cut, false, pt(30.0, 20.0)));
    let circle = Geom::Circle {
        centre: id(10),
        radius: 5.0,
    };
    entities.insert(id(11), ent(&cut, false, circle));
    // A value that is not exactly representable, to exercise f64 output.
    entities.insert(id(12), ent(&score, true, pt(0.1 + 0.2, 40.0 / 3.0)));

    let mut constraints = BTreeMap::new();
    constraints.insert(id(13), Constraint::Horizontal(id(6)));
    constraints.insert(id(14), Constraint::Vertical(id(7)));
    constraints.insert(id(15), Constraint::Horizontal(id(8)));
    constraints.insert(id(16), Constraint::Vertical(id(9)));
    let width = Dim::Expr {
        expr: "width".into(),
        value: 60.0,
    };
    let dist = Constraint::Distance {
        a: id(2),
        b: id(3),
        value: width,
    };
    constraints.insert(id(17), dist);
    let r = Constraint::Radius {
        circle: id(11),
        value: Dim::Value(5.0),
    };
    constraints.insert(id(18), r);

    let mut parameters = BTreeMap::new();
    let width = Parameter {
        expr: "60 mm".into(),
        value: 60.0,
    };
    parameters.insert("width".into(), width);

    let mut layers = BTreeMap::new();
    let cut_layer = Layer {
        name: "Cut".into(),
        operation: Operation::Cut,
        colour: [255, 0, 0],
    };
    layers.insert(cut, cut_layer);
    let score_layer = Layer {
        name: "Score".into(),
        operation: Operation::Score,
        colour: [0, 0, 255],
    };
    layers.insert(score, score_layer);

    Document {
        format_version: 1,
        units: Units::Mm,
        // IDs in the u64 run start at 1, so the next free one is n + 1.
        next_id: SAMPLE_IDS as u64 + 1,
        parameters,
        layers,
        entities,
        constraints,
    }
}

/// Number of ID slots `sample` uses.
pub const SAMPLE_IDS: usize = 19;

/// Synthetic 500-entity document (a grid of points joined by lines) for
/// size measurements at the SPEC §3 solver-budget scale.
pub fn grid<Id: Ord + Clone>(ids: &[Id]) -> Document<Id> {
    let mut doc = sample(&ids[..SAMPLE_IDS]);
    let mut next = SAMPLE_IDS;
    let layer = ids[0].clone();
    let mut prev: Option<Id> = None;
    while next + 1 < SAMPLE_IDS + 500 {
        let p = ids[next].clone();
        let k = (next - SAMPLE_IDS) as f64;
        let g = Geom::Point {
            x: (k * 1.37).sin() * 100.0,
            y: (k * 0.91).cos() * 100.0,
        };
        doc.entities.insert(p.clone(), ent(&layer, false, g));
        next += 1;
        if let Some(q) = prev {
            let g = Geom::Line {
                start: q,
                end: p.clone(),
            };
            doc.entities
                .insert(ids[next].clone(), ent(&layer, false, g));
            next += 1;
        }
        prev = Some(p);
    }
    doc.next_id = next as u64 + 1;
    doc
}

fn ent<Id: Clone>(layer: &Id, construction: bool, geom: Geom<Id>) -> Entity<Id> {
    Entity {
        layer: layer.clone(),
        construction,
        geom,
    }
}

/// Floats that commonly break naive text round-tripping.
pub const TRICKY_F64: [f64; 10] = [
    0.1 + 0.2,
    1.0 / 3.0,
    std::f64::consts::PI,
    1e-7,
    123_456.789,
    -0.0,
    f64::MIN_POSITIVE,
    5e-324,
    f64::MAX,
    1e21,
];

#[cfg(feature = "ron")]
pub fn to_ron<T: Serialize>(v: &T) -> Result<String, ron::Error> {
    let cfg = ron::ser::PrettyConfig::new()
        .indentor("  ".to_string())
        .struct_names(false);
    ron::ser::to_string_pretty(v, cfg)
}

#[cfg(feature = "json")]
pub fn to_json<T: Serialize>(v: &T) -> Result<String, serde_json::Error> {
    serde_json::to_string_pretty(v)
}

// --- wasm size probes: each export pulls in one parser + serialiser. ---

#[cfg(target_arch = "wasm32")]
type U64Doc = Document<u64>;

#[cfg(all(target_arch = "wasm32", feature = "ron"))]
#[no_mangle]
#[allow(unsafe_code, clippy::not_unsafe_ptr_arg_deref)]
pub extern "C" fn probe_ron(ptr: *const u8, len: usize) -> usize {
    // SAFETY: size probe only; the (never written) JS caller would pass a
    // valid buffer. Nothing here is shipped.
    let bytes = unsafe { std::slice::from_raw_parts(ptr, len) };
    let Ok(s) = std::str::from_utf8(bytes) else {
        return 0;
    };
    match ron::from_str::<U64Doc>(s) {
        Ok(d) => to_ron(&d).map(|s| s.len()).unwrap_or(0),
        Err(_) => 0,
    }
}

#[cfg(all(target_arch = "wasm32", feature = "json"))]
#[no_mangle]
#[allow(unsafe_code, clippy::not_unsafe_ptr_arg_deref)]
pub extern "C" fn probe_json(ptr: *const u8, len: usize) -> usize {
    // SAFETY: as `probe_ron`.
    let bytes = unsafe { std::slice::from_raw_parts(ptr, len) };
    match serde_json::from_slice::<U64Doc>(bytes) {
        Ok(d) => to_json(&d).map(|s| s.len()).unwrap_or(0),
        Err(_) => 0,
    }
}

/// Baseline: the model built in memory, no format. Lets the size table
/// subtract what is common to both.
#[cfg(target_arch = "wasm32")]
#[no_mangle]
pub extern "C" fn probe_base(n: usize) -> usize {
    let ids: Vec<u64> = (1..=n as u64).collect();
    grid(&ids).entities.len()
}

/// UUIDv7 generation on wasm32 needs a clock and an RNG (ADR-0003).
#[cfg(all(target_arch = "wasm32", feature = "uuid"))]
#[no_mangle]
pub extern "C" fn probe_uuid() -> u64 {
    uuid::Uuid::now_v7().as_u64_pair().0
}
