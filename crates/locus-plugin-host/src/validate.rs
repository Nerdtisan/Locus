// SPDX-License-Identifier: AGPL-3.0-or-later

//! Plugin output is untrusted (§15.4, CLAUDE.md "Untrusted input"): every
//! value a plugin returns is checked here before anything else sees it.
//! Sizes and counts are bounded, numbers must be finite and in range, and
//! indices must point at something. Nothing here panics.

use crate::types::{
    Constraint, Entity, ExportModel, FormatInfo, Generated, ParamKind, ParamSpec, Path, PluginInfo,
    Shape, Vec2,
};

/// Most points, entities, constraints, paths or segments one call may return.
pub const MAX_ITEMS: usize = 100_000;
/// Most parameters in a schema.
pub const MAX_PARAMS: usize = 256;
/// Longest string a plugin may return (ids, names, labels, messages), in bytes.
pub const MAX_TEXT: usize = 4096;
/// Largest |coordinate| or length a plugin may return: 1 km in millimetres.
pub const MAX_COORD: f64 = 1.0e6;
/// Largest file an exporter may return.
pub const MAX_EXPORT_BYTES: usize = 256 * 1024 * 1024;
/// Longest reverse-DNS plugin id.
pub const MAX_ID_LEN: usize = 128;

/// Why plugin output was rejected. Messages name the field, never the data.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct Malformed(pub String);

type Result<T = ()> = std::result::Result<T, Malformed>;

fn bad(msg: impl Into<String>) -> Malformed {
    Malformed(msg.into())
}

fn text(field: &str, s: &str) -> Result {
    if s.len() > MAX_TEXT {
        return Err(bad(format!("{field} is longer than {MAX_TEXT} bytes")));
    }
    if s.chars().any(|c| c.is_control() && c != '\n' && c != '\t') {
        return Err(bad(format!("{field} contains control characters")));
    }
    Ok(())
}

fn count(field: &str, n: usize, max: usize) -> Result {
    if n > max {
        return Err(bad(format!("{field}: {n} items, more than {max}")));
    }
    Ok(())
}

fn number(field: &str, v: f64) -> Result {
    if !v.is_finite() || v.abs() > MAX_COORD {
        return Err(bad(format!(
            "{field} is not a finite number within ±{MAX_COORD} mm"
        )));
    }
    Ok(())
}

fn point(field: &str, p: Vec2) -> Result {
    number(field, p.x)?;
    number(field, p.y)
}

fn positive(field: &str, v: f64) -> Result {
    number(field, v)?;
    if v <= 0.0 {
        return Err(bad(format!("{field} must be positive")));
    }
    Ok(())
}

/// A reverse-DNS id (§15.4): at least two dot-separated labels of lowercase
/// ASCII letters, digits and hyphens, e.g. `com.example.finger-box`.
pub fn is_reverse_dns(id: &str) -> bool {
    let label_ok = |l: &str| {
        !l.is_empty()
            && !l.starts_with('-')
            && !l.ends_with('-')
            && l.bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    };
    id.len() <= MAX_ID_LEN && id.split('.').count() >= 2 && id.split('.').all(label_ok)
}

/// Kebab-case parameter key: lowercase letters, digits, single hyphens.
fn is_kebab(key: &str) -> bool {
    !key.is_empty()
        && key.len() <= 64
        && key.split('-').all(|part| {
            !part.is_empty()
                && part
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        })
}

pub fn info(info: &PluginInfo) -> Result {
    if !is_reverse_dns(&info.id) {
        return Err(bad("plugin id is not a reverse-DNS name"));
    }
    text("plugin name", &info.name)?;
    if info.name.trim().is_empty() {
        return Err(bad("plugin name is empty"));
    }
    text("plugin version", &info.version)?;
    if info.version.trim().is_empty() {
        return Err(bad("plugin version is empty"));
    }
    Ok(())
}

pub fn schema(specs: &[ParamSpec]) -> Result {
    count("parameter schema", specs.len(), MAX_PARAMS)?;
    for (i, s) in specs.iter().enumerate() {
        let field = |f: &str| format!("parameter {i} {f}");
        if !is_kebab(&s.key) {
            return Err(bad(field("key is not kebab-case")));
        }
        if specs[..i].iter().any(|o| o.key == s.key) {
            return Err(bad(field("key is a duplicate")));
        }
        text(&field("label"), &s.label)?;
        text(&field("help"), &s.help)?;
        match &s.kind {
            ParamKind::Number(n) => {
                text(&field("unit"), &n.unit)?;
                for v in [Some(n.default), n.min, n.max].into_iter().flatten() {
                    if !v.is_finite() {
                        return Err(bad(field("has a non-finite bound or default")));
                    }
                }
                let lo = n.min.unwrap_or(f64::NEG_INFINITY);
                let hi = n.max.unwrap_or(f64::INFINITY);
                if lo > hi || n.default < lo || n.default > hi {
                    return Err(bad(field("default is outside its range")));
                }
            }
            ParamKind::Integer(n) => {
                let lo = n.min.unwrap_or(i64::MIN);
                let hi = n.max.unwrap_or(i64::MAX);
                if lo > hi || n.default < lo || n.default > hi {
                    return Err(bad(field("default is outside its range")));
                }
            }
            ParamKind::Choice(c) => {
                count(&field("options"), c.options.len(), MAX_PARAMS)?;
                if c.options.is_empty() || c.default as usize >= c.options.len() {
                    return Err(bad(field("has no options or a bad default")));
                }
                for o in &c.options {
                    text(&field("option"), o)?;
                }
            }
            ParamKind::Text(t) => {
                if t.max_length as usize > MAX_TEXT || t.default.len() > t.max_length as usize {
                    return Err(bad(field("default is longer than its max length")));
                }
                text(&field("default"), &t.default)?;
            }
            ParamKind::Boolean(_) | ParamKind::DocParameter => {}
        }
    }
    Ok(())
}

pub fn generated(g: &Generated) -> Result {
    count("points", g.points.len(), MAX_ITEMS)?;
    count("entities", g.entities.len(), MAX_ITEMS)?;
    count("constraints", g.constraints.len(), MAX_ITEMS)?;
    for (i, p) in g.points.iter().enumerate() {
        point(&format!("point {i}"), *p)?;
    }
    let pt = |i: usize, idx: u32| -> Result<Vec2> {
        g.points
            .get(idx as usize)
            .copied()
            .ok_or_else(|| bad(format!("entity {i} refers to a missing point")))
    };
    for (i, e) in g.entities.iter().enumerate() {
        match *e {
            Entity::Point(p) => {
                pt(i, p)?;
            }
            Entity::Line(a, b) => {
                if pt(i, a)? == pt(i, b)? {
                    return Err(bad(format!("entity {i} is a zero-length line")));
                }
            }
            Entity::Circle { center, radius } => {
                pt(i, center)?;
                positive(&format!("entity {i} radius"), radius)?;
            }
            Entity::Arc { center, start, end } => {
                let (c, s, e) = (pt(i, center)?, pt(i, start)?, pt(i, end)?);
                let (rs, re) = ((s.x - c.x).hypot(s.y - c.y), (e.x - c.x).hypot(e.y - c.y));
                if rs <= 0.0 || re <= 0.0 {
                    return Err(bad(format!("entity {i} is a zero-radius arc")));
                }
            }
        }
    }
    let ent = |i: usize, idx: u32| -> Result {
        if (idx as usize) < g.entities.len() {
            Ok(())
        } else {
            Err(bad(format!("constraint {i} refers to a missing entity")))
        }
    };
    for (i, c) in g.constraints.iter().enumerate() {
        match *c {
            Constraint::Horizontal(a) | Constraint::Vertical(a) => ent(i, a)?,
            Constraint::Coincident(a, b)
            | Constraint::Parallel(a, b)
            | Constraint::Perpendicular(a, b)
            | Constraint::Tangent(a, b)
            | Constraint::Equal(a, b) => {
                ent(i, a)?;
                ent(i, b)?;
            }
            Constraint::Distance(a, b, d) => {
                ent(i, a)?;
                ent(i, b)?;
                number(&format!("constraint {i} distance"), d)?;
                if d < 0.0 {
                    return Err(bad(format!("constraint {i} distance is negative")));
                }
            }
            Constraint::Radius(a, r) => {
                ent(i, a)?;
                positive(&format!("constraint {i} radius"), r)?;
            }
        }
    }
    Ok(())
}

fn shape(field: &str, s: &Shape) -> Result {
    match *s {
        Shape::Line { a, b } => {
            point(field, a)?;
            point(field, b)
        }
        Shape::Arc { center, start, end } => {
            point(field, center)?;
            point(field, start)?;
            point(field, end)
        }
        Shape::Circle { center, radius } => {
            point(field, center)?;
            positive(field, radius)
        }
    }
}

pub fn paths(paths: &[Path]) -> Result {
    count("paths", paths.len(), MAX_ITEMS)?;
    let segments: usize = paths.iter().map(|p| p.segments.len()).sum();
    count("segments", segments, MAX_ITEMS)?;
    for (i, p) in paths.iter().enumerate() {
        if p.segments.is_empty() {
            return Err(bad(format!("path {i} is empty")));
        }
        for (j, s) in p.segments.iter().enumerate() {
            shape(&format!("path {i} segment {j}"), &s.shape)?;
        }
    }
    Ok(())
}

pub fn format_info(f: &FormatInfo) -> Result {
    if !is_kebab(&f.id) {
        return Err(bad("format id is not kebab-case"));
    }
    text("format name", &f.name)?;
    if f.extension.is_empty()
        || f.extension.len() > 16
        || !f.extension.bytes().all(|b| b.is_ascii_alphanumeric())
    {
        return Err(bad("format extension must be 1-16 ASCII letters or digits"));
    }
    let mime_ok = f.mime.len() <= 128
        && f.mime.split_once('/').is_some_and(|(a, b)| {
            let tok = |s: &str| {
                !s.is_empty()
                    && s.bytes()
                        .all(|c| c.is_ascii_alphanumeric() || b"+-.".contains(&c))
            };
            tok(a) && tok(b)
        });
    if !mime_ok {
        return Err(bad("format MIME type is malformed"));
    }
    Ok(())
}

pub fn export_bytes(bytes: &[u8]) -> Result {
    if bytes.len() > MAX_EXPORT_BYTES {
        return Err(bad(format!(
            "export is larger than {MAX_EXPORT_BYTES} bytes"
        )));
    }
    Ok(())
}

/// Bounds a model the host sends in; the host builds it, so this is a sanity
/// check that keeps a huge document from exceeding what plugins are promised.
pub fn export_model(m: &ExportModel) -> Result {
    count("layers", m.layers.len(), MAX_ITEMS)?;
    let all: Vec<&Path> = m.layers.iter().flat_map(|l| &l.paths).collect();
    count("paths", all.len(), MAX_ITEMS)
}

/// Plugin-reported messages are shown to the user: cap their length and strip
/// control characters rather than reject the error itself.
pub fn sanitise_message(msg: &str) -> String {
    let mut out: String = msg
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .take(512)
        .collect();
    if msg.chars().count() > 512 {
        out.push('…');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::*;

    fn v(x: f64, y: f64) -> Vec2 {
        Vec2 { x, y }
    }

    #[test]
    fn reverse_dns_ids() {
        assert!(is_reverse_dns("com.example.finger-box"));
        assert!(is_reverse_dns("io.locus.hpgl"));
        for bad in [
            "finger-box",
            "Com.Example",
            "com..x",
            "com.-x",
            "com.x/",
            "",
        ] {
            assert!(!is_reverse_dns(bad), "{bad}");
        }
    }

    #[test]
    fn generated_rejects_bad_indices_and_numbers() {
        let ok = Generated {
            points: vec![v(0.0, 0.0), v(10.0, 0.0)],
            entities: vec![Entity::Line(0, 1)],
            constraints: vec![Constraint::Horizontal(0)],
        };
        assert!(generated(&ok).is_ok());

        let mut g = ok.clone();
        g.entities.push(Entity::Line(0, 7));
        assert!(generated(&g).is_err());

        let mut g = ok.clone();
        g.points[1].x = f64::NAN;
        assert!(generated(&g).is_err());

        let mut g = ok.clone();
        g.points[1].x = 2.0e6;
        assert!(generated(&g).is_err());

        let mut g = ok.clone();
        g.constraints.push(Constraint::Equal(0, 1));
        assert!(generated(&g).is_err());

        let mut g = ok;
        g.entities.push(Entity::Circle {
            center: 0,
            radius: -1.0,
        });
        assert!(generated(&g).is_err());
    }

    #[test]
    fn schema_rejects_bad_defaults_and_duplicates() {
        let num = |key: &str, default| ParamSpec {
            key: key.into(),
            label: "L".into(),
            help: String::new(),
            kind: ParamKind::Number(NumberSpec {
                unit: "mm".into(),
                min: Some(0.0),
                max: Some(10.0),
                default,
            }),
        };
        assert!(schema(&[num("width", 5.0)]).is_ok());
        assert!(schema(&[num("width", 11.0)]).is_err());
        assert!(schema(&[num("Width", 5.0)]).is_err());
        assert!(schema(&[num("width", 5.0), num("width", 1.0)]).is_err());
    }

    #[test]
    fn format_info_checks() {
        let f = FormatInfo {
            id: "hpgl".into(),
            name: "HPGL".into(),
            extension: "plt".into(),
            mime: "application/vnd.hp-hpgl".into(),
        };
        assert!(format_info(&f).is_ok());
        assert!(
            format_info(&FormatInfo {
                extension: "../x".into(),
                ..f.clone()
            })
            .is_err()
        );
        assert!(
            format_info(&FormatInfo {
                mime: "nope".into(),
                ..f
            })
            .is_err()
        );
    }

    #[test]
    fn messages_are_capped() {
        let long = "x".repeat(10_000);
        assert!(sanitise_message(&long).chars().count() <= 513);
        assert_eq!(sanitise_message("a\u{1b}[2Jb"), "a [2Jb");
    }
}
