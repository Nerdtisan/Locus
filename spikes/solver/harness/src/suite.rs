// SPDX-License-Identifier: AGPL-3.0-or-later

//! The measurement suite and its report table.

use crate::check::{self, SATISFIED};
use crate::fixtures::{self, Drag, Fixture};
use crate::model::Sketch;
use crate::{Backend, Clock};
use std::fmt::Write as _;

/// One measured row for one backend.
#[derive(Clone, Debug, PartialEq)]
pub struct Measure {
    pub label: String,
    pub value: String,
}

/// Everything measured for one backend, in a fixed row order.
#[derive(Clone, Debug, PartialEq)]
pub struct BackendReport {
    pub backend: String,
    pub rows: Vec<Measure>,
    /// Coverage cases that failed, and other notes.
    pub notes: Vec<String>,
}

impl BackendReport {
    fn row(&mut self, label: &str, value: String) {
        self.rows.push(Measure {
            label: label.to_string(),
            value,
        });
    }
}

/// Run the whole suite on one backend. `cells` sizes the generated sketch
/// (72 cells = 504 curves + 720 points).
pub fn run<B: Backend>(b: &B, clock: &dyn Clock, cells: usize) -> BackendReport {
    let mut rep = BackendReport {
        backend: b.name().to_string(),
        rows: Vec::new(),
        notes: Vec::new(),
    };
    coverage(b, &mut rep);

    let rect = fixtures::filleted_rect();
    rep.row(
        "fillet-rect: full solve, ms",
        full_solve(b, clock, &rect, 50),
    );
    rep.row(
        "fillet-rect: DOF (expect 0) / diagnose ms",
        dof(b, clock, &rect),
    );
    rep.row(
        "fillet-rect-under: DOF (expect 1)",
        dof(b, clock, &fixtures::filleted_rect_under()),
    );
    rep.row(
        "fillet-rect-redundant: solved / DOF / redundant named",
        redundant(b, &fixtures::filleted_rect_redundant()),
    );
    rep.row(
        "fillet-rect-conflict: detected / named / ms",
        conflict(b, clock, &fixtures::filleted_rect_conflict()),
    );

    let gen = fixtures::generated(cells);
    let label = format!(
        "{} curves + {} points",
        gen.exact.curves.len(),
        gen.exact.points.len()
    );
    rep.notes.push(format!("generated sketch: {label}"));
    rep.row("generated: full solve, ms", full_solve(b, clock, &gen, 3));
    rep.row(
        &format!("generated: DOF (expect {}) / diagnose ms", gen.expected_dof),
        dof(b, clock, &gen),
    );
    for d in &gen.drags {
        rep.row(
            &format!("generated: {} frame, mean / max ms (frames ok)", d.name),
            drag(b, clock, &gen, d),
        );
    }
    rep.row(
        "generated-conflict: detected / named / ms",
        conflict(b, clock, &fixtures::generated_conflict(cells)),
    );
    rep
}

fn coverage<B: Backend>(b: &B, rep: &mut BackendReport) {
    let cases = fixtures::coverage();
    let mut ok = 0;
    for c in &cases {
        let res = b.load(&c.start).map(|mut st| {
            let claimed = b.solve(&mut st);
            let mut out = c.start.clone();
            b.read(&st, &mut out);
            (claimed, check::satisfied(&out))
        });
        match res {
            Ok((_, true)) => ok += 1,
            Ok((claimed, false)) => rep.notes.push(format!(
                "coverage `{}`: not satisfied (backend claimed {})",
                c.name,
                if claimed { "success" } else { "failure" }
            )),
            Err(e) => rep
                .notes
                .push(format!("coverage `{}`: unsupported ({e})", c.name)),
        }
    }
    rep.row(
        "§5 v1 coverage: cases solved",
        format!("{ok}/{}", cases.len()),
    );
}

fn fmt_ms(ms: f64) -> String {
    if ms < 0.1 {
        format!("{:.4}", ms)
    } else if ms < 10.0 {
        format!("{:.3}", ms)
    } else {
        format!("{:.1}", ms)
    }
}

/// Mean time of a solve from the perturbed start, over `reps` freshly loaded
/// states (loading is not timed). Marked when the result does not check out.
fn full_solve<B: Backend>(b: &B, clock: &dyn Clock, f: &Fixture, reps: usize) -> String {
    let mut states = Vec::with_capacity(reps);
    for _ in 0..reps {
        match b.load(&f.start) {
            Ok(st) => states.push(st),
            Err(e) => return format!("unsupported: {e}"),
        }
    }
    let t0 = clock.now_ms();
    let mut claimed = true;
    for st in &mut states {
        claimed &= b.solve(st);
    }
    let ms = (clock.now_ms() - t0) / reps as f64;
    let mut out = f.start.clone();
    let verified = states.iter().all(|st| {
        b.read(st, &mut out);
        check::satisfied(&out)
    });
    let mut s = fmt_ms(ms);
    if !verified {
        let (v, at) = check::worst(&out);
        let _ = write!(
            s,
            " ✗ unsolved (claimed {claimed}; worst {v:.1e} at {})",
            at.and_then(|i| out.constraints.get(i))
                .map_or("an arc", |k| k.name())
        );
    }
    s
}

/// Solve from the exact geometry, then time the backend's diagnosis.
fn dof<B: Backend>(b: &B, clock: &dyn Clock, f: &Fixture) -> String {
    let mut st = match b.load(&f.start) {
        Ok(st) => st,
        Err(e) => return format!("unsupported: {e}"),
    };
    b.solve(&mut st);
    let t0 = clock.now_ms();
    let d = b.diagnose(&mut st);
    let ms = clock.now_ms() - t0;
    let dof = match d.dof {
        Some(n) if n == f.expected_dof => format!("{n} ✓"),
        Some(n) => format!("{n} ✗"),
        None => "not reported".to_string(),
    };
    let note = d.note.map_or(String::new(), |n| format!(" ({n})"));
    format!("{dof}{note} / {}", fmt_ms(ms))
}

fn redundant<B: Backend>(b: &B, f: &Fixture) -> String {
    let mut st = match b.load(&f.start) {
        Ok(st) => st,
        Err(e) => return format!("unsupported: {e}"),
    };
    b.solve(&mut st);
    let mut out = f.start.clone();
    b.read(&st, &mut out);
    let solved = check::satisfied(&out);
    let d = b.diagnose(&mut st);
    let extra = f.exact.constraints.len() - 1;
    let named = match &d.redundant {
        Some(r) if r.contains(&extra) => "yes",
        Some(r) if !r.is_empty() => "another",
        Some(_) => "no",
        None => "not reported",
    };
    format!(
        "{} / {} / {named}",
        if solved { "yes" } else { "NO" },
        d.dof.map_or("-".to_string(), |n| n.to_string())
    )
}

fn conflict<B: Backend>(b: &B, clock: &dyn Clock, f: &Fixture) -> String {
    let mut st = match b.load(&f.start) {
        Ok(st) => st,
        Err(e) => return format!("unsupported: {e}"),
    };
    let t0 = clock.now_ms();
    let claimed = b.solve(&mut st);
    let d = b.diagnose(&mut st);
    let ms = clock.now_ms() - t0;
    let mut out = f.start.clone();
    b.read(&st, &mut out);
    let detected = match (claimed, check::satisfied(&out)) {
        (false, _) => "yes",
        (true, false) => "NO (claimed solved)",
        (true, true) => "NO (?)",
    };
    let named = match &d.conflicting {
        Some(c) if f.conflict.iter().all(|k| c.contains(k)) => format!("both ({} listed)", c.len()),
        Some(c) if f.conflict.iter().any(|k| c.contains(k)) => format!("one ({} listed)", c.len()),
        Some(c) => format!("neither ({} listed)", c.len()),
        None => "not reported".to_string(),
    };
    format!("{detected} / {named} / {}", fmt_ms(ms))
}

/// Drag from the exact geometry along the path; time each frame.
fn drag<B: Backend>(b: &B, clock: &dyn Clock, f: &Fixture, d: &Drag) -> String {
    let mut st = match b.load(&f.exact) {
        Ok(st) => st,
        Err(e) => return format!("unsupported: {e}"),
    };
    b.solve(&mut st);
    let mut out: Sketch = f.exact.clone();
    let (mut total, mut max, mut good) = (0.0f64, 0.0f64, 0);
    for &to in &d.path {
        let t0 = clock.now_ms();
        b.drag(&mut st, d.point, to);
        let ms = clock.now_ms() - t0;
        total += ms;
        max = max.max(ms);
        b.read(&st, &mut out);
        let at = out
            .points
            .get(d.point)
            .map_or(f64::INFINITY, |p| p.dist(to));
        if check::satisfied(&out) && at < SATISFIED.sqrt() {
            good += 1;
        }
    }
    let n = d.path.len();
    let budget = if max < 16.0 { "" } else { " (over 16 ms)" };
    format!(
        "{} / {}{budget} ({good}/{n})",
        fmt_ms(total / n as f64),
        fmt_ms(max)
    )
}

/// Render reports side by side as a Markdown table, notes underneath.
pub fn table(reports: &[BackendReport]) -> String {
    let mut s = String::new();
    let _ = write!(s, "| measure |");
    for r in reports {
        let _ = write!(s, " {} |", r.backend);
    }
    let _ = write!(s, "\n|---|");
    for _ in reports {
        let _ = write!(s, "---|");
    }
    s.push('\n');
    let rows = reports.first().map_or(0, |r| r.rows.len());
    for i in 0..rows {
        let label = reports
            .first()
            .and_then(|r| r.rows.get(i))
            .map_or("", |m| m.label.as_str());
        let _ = write!(s, "| {label} |");
        for r in reports {
            let v = r.rows.get(i).map_or("", |m| m.value.as_str());
            let _ = write!(s, " {v} |");
        }
        s.push('\n');
    }
    for r in reports {
        for n in &r.notes {
            let _ = writeln!(s, "- {}: {n}", r.backend);
        }
    }
    s
}
