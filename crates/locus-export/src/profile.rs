// SPDX-License-Identifier: AGPL-3.0-or-later

//! Machine profiles (SPEC §7, #236): one-click export settings for a machine.
//!
//! A profile names an exporter (the `format`, an [`Exporter`](crate::Exporter)
//! id such as `svg`, `dxf` or a plugin's `hpgl`), a colour per layer operation,
//! the bed (size and origin corner) and default operation parameters.
//!
//! Storage (decided 2026-11-11): the built-in presets are compiled in
//! ([`presets`]); user profiles are JSON held by the app; a single profile
//! travels as a standalone `.json` file. Imported JSON is **untrusted**
//! ([`MachineProfile::from_json`] bounds its size and validates every field,
//! and never panics). A document stores only the profile *id* (SPEC §10).

use crate::pipeline::ExportModel;
use locus_doc::{Operation, OperationKind, check_profile_id};
use locus_geom::{Aabb, Vec2};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use thiserror::Error;

/// Largest profile JSON accepted on import.
pub const MAX_PROFILE_JSON_BYTES: usize = 16 * 1024;
/// Longest display name.
pub const MAX_NAME_CHARS: usize = 80;
/// Bed sides are bounded to a sane machine: 1 mm ..= 10 m.
pub const MIN_BED_MM: f64 = 1.0;
pub const MAX_BED_MM: f64 = 10_000.0;
/// Upper bound for a default kerf, blade offset or pen width, mm.
pub const MAX_DEFAULT_MM: f64 = 100.0;

/// Why a profile was refused.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ProfileError {
    #[error("profile file is too large ({0} bytes)")]
    TooLarge(usize),
    #[error("not a valid profile: {0}")]
    Parse(String),
    #[error("invalid profile: {0}")]
    Invalid(String),
}

/// Display units for lengths in the UI (the model itself is millimetres, §4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ProfileUnits {
    #[default]
    Mm,
    Inch,
}

/// The machine's origin corner, as seen looking at the bed (SPEC §7, #237).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Origin {
    #[default]
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

impl Origin {
    pub const ALL: [Origin; 4] = [
        Origin::TopLeft,
        Origin::TopRight,
        Origin::BottomLeft,
        Origin::BottomRight,
    ];

    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Origin::TopLeft => "Top left",
            Origin::TopRight => "Top right",
            Origin::BottomLeft => "Bottom left",
            Origin::BottomRight => "Bottom right",
        }
    }
}

/// The machine bed (SPEC §7).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Bed {
    pub width_mm: f64,
    pub height_mm: f64,
    pub origin: Origin,
}

impl Bed {
    /// The bed in document coordinates (Y up): `0..width` by `0..height`,
    /// where people draw. The origin corner says where the *machine's* zero
    /// is on this rectangle ([`Bed::origin_point`]); it does not move the bed.
    #[must_use]
    pub fn rect(&self) -> Aabb {
        Aabb::from_points(Vec2::ZERO, Vec2::new(self.width_mm, self.height_mm))
    }

    /// The machine origin on the bed, in document coordinates.
    #[must_use]
    pub fn origin_point(&self) -> Vec2 {
        let r = self.rect();
        let x = match self.origin {
            Origin::TopLeft | Origin::BottomLeft => r.min.x,
            Origin::TopRight | Origin::BottomRight => r.max.x,
        };
        let y = match self.origin {
            Origin::BottomLeft | Origin::BottomRight => r.min.y,
            Origin::TopLeft | Origin::TopRight => r.max.y,
        };
        Vec2::new(x, y)
    }

    fn validate(&self) -> Result<(), ProfileError> {
        for (what, v) in [("bed width", self.width_mm), ("bed height", self.height_mm)] {
            if !(v.is_finite() && (MIN_BED_MM..=MAX_BED_MM).contains(&v)) {
                return Err(ProfileError::Invalid(format!(
                    "{what} must be {MIN_BED_MM}..={MAX_BED_MM} mm"
                )));
            }
        }
        Ok(())
    }
}

/// Default operation parameters a profile suggests for new layers.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Defaults {
    pub kerf_mm: Option<f64>,
    pub blade_offset_mm: Option<f64>,
    pub pen_width_mm: Option<f64>,
}

impl Defaults {
    /// An operation of `kind` with this profile's defaults filled in.
    #[must_use]
    pub fn operation(&self, kind: OperationKind) -> Operation {
        let mut op = kind.default_operation();
        match &mut op {
            Operation::Cut(p) => {
                if let Some(k) = self.kerf_mm {
                    p.kerf_width = k;
                }
            }
            Operation::DragKnife(p) => {
                if let Some(b) = self.blade_offset_mm {
                    p.blade_offset = b;
                }
            }
            Operation::Draw(p) => {
                if let Some(w) = self.pen_width_mm {
                    p.pen_width = w;
                }
            }
            _ => {}
        }
        op
    }

    fn validate(&self) -> Result<(), ProfileError> {
        for (what, v, min) in [
            ("kerf", self.kerf_mm, 0.0),
            ("blade offset", self.blade_offset_mm, 0.0),
            ("pen width", self.pen_width_mm, f64::MIN_POSITIVE),
        ] {
            if let Some(v) = v
                && !(v.is_finite() && (min..=MAX_DEFAULT_MM).contains(&v))
            {
                return Err(ProfileError::Invalid(format!("{what} out of range")));
            }
        }
        Ok(())
    }
}

/// One machine's export settings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MachineProfile {
    /// Slug (`a-z 0-9 . _ -`), stored in documents.
    pub id: String,
    pub name: String,
    /// [`Exporter`](crate::Exporter) id: `svg`, `pdf`, `dxf`, or a plugin's.
    pub format: String,
    #[serde(default)]
    pub units: ProfileUnits,
    /// Exported colour per operation; operations not listed keep the layer's own colour.
    #[serde(default)]
    pub colours: BTreeMap<OperationKind, [u8; 3]>,
    #[serde(default)]
    pub bed: Option<Bed>,
    #[serde(default)]
    pub defaults: Defaults,
}

impl MachineProfile {
    /// Check every field. Run on import; built-ins are checked by a test.
    ///
    /// # Errors
    /// [`ProfileError::Invalid`] naming the first bad field.
    pub fn validate(&self) -> Result<(), ProfileError> {
        check_profile_id(&self.id).map_err(|_| ProfileError::Invalid("bad id".into()))?;
        check_profile_id(&self.format).map_err(|_| ProfileError::Invalid("bad format".into()))?;
        if self.name.trim().is_empty()
            || self.name.chars().count() > MAX_NAME_CHARS
            || self.name.chars().any(char::is_control)
        {
            return Err(ProfileError::Invalid("bad name".into()));
        }
        if self.colours.contains_key(&OperationKind::Reference) {
            return Err(ProfileError::Invalid(
                "Reference layers are never exported".into(),
            ));
        }
        if let Some(b) = &self.bed {
            b.validate()?;
        }
        self.defaults.validate()
    }

    /// Parse and validate an imported profile (untrusted input).
    ///
    /// # Errors
    /// [`ProfileError`] when the text is too large, malformed or invalid.
    pub fn from_json(text: &str) -> Result<Self, ProfileError> {
        if text.len() > MAX_PROFILE_JSON_BYTES {
            return Err(ProfileError::TooLarge(text.len()));
        }
        let p: Self = serde_json::from_str(text).map_err(|e| ProfileError::Parse(e.to_string()))?;
        p.validate()?;
        Ok(p)
    }

    /// Pretty JSON for export to a `.json` file; `None` only if serialising fails.
    #[must_use]
    pub fn to_json(&self) -> Option<String> {
        serde_json::to_string_pretty(self).ok()
    }

    /// Apply the colour mapping to an export model (before the writer runs).
    pub fn apply(&self, model: &mut ExportModel) {
        for layer in &mut model.layers {
            if let Some(c) = self.colours.get(&layer.operation.kind()) {
                layer.colour = *c;
            }
        }
    }
}

/// Is `id` one of the compiled-in presets?
#[must_use]
pub fn is_builtin(id: &str) -> bool {
    presets().iter().any(|p| p.id == id)
}

/// Find a profile by id. Presets win, and the app refuses to import a profile
/// that uses a preset's id, so a user profile can never shadow a preset.
#[must_use]
pub fn find(id: &str, user: &[MachineProfile]) -> Option<MachineProfile> {
    presets()
        .into_iter()
        .find(|p| p.id == id)
        .or_else(|| user.iter().find(|p| p.id == id).cloned())
}

const IN: f64 = 25.4;

fn preset(
    id: &str,
    name: &str,
    format: &str,
    units: ProfileUnits,
    colours: &[(OperationKind, [u8; 3])],
    bed: Option<Bed>,
) -> MachineProfile {
    MachineProfile {
        id: id.to_owned(),
        name: name.to_owned(),
        format: format.to_owned(),
        units,
        colours: colours.iter().copied().collect(),
        bed,
        defaults: Defaults::default(),
    }
}

fn bed(w: f64, h: f64, origin: Origin) -> Option<Bed> {
    Some(Bed {
        width_mm: w,
        height_mm: h,
        origin,
    })
}

/// The built-in presets, in menu order (SPEC §7, #236).
#[must_use]
pub fn presets() -> Vec<MachineProfile> {
    use OperationKind::{Cut, DragKnife, Draw, Engrave, Score};
    // LightBurn assigns a cut layer per colour: red cuts, blue scores, black engraves.
    let lightburn = [
        (Cut, [255, 0, 0]),
        (Score, [0, 0, 255]),
        (Engrave, [0, 0, 0]),
        (Draw, [0, 128, 0]),
        (DragKnife, [255, 0, 255]),
    ];
    // Vinyl cutters take the colour per shape as the operation.
    let vinyl = [
        (Cut, [0, 0, 0]),
        (Score, [255, 0, 0]),
        (Engrave, [0, 0, 255]),
        (Draw, [0, 128, 0]),
        (DragKnife, [0, 0, 0]),
    ];
    let lb_bed = bed(400.0, 400.0, Origin::TopLeft);
    let mat12 = bed(12.0 * IN, 12.0 * IN, Origin::TopLeft);
    let mm = ProfileUnits::Mm;
    let inch = ProfileUnits::Inch;
    vec![
        preset(
            "lightburn",
            "LightBurn (SVG)",
            "svg",
            mm,
            &lightburn,
            lb_bed,
        ),
        preset(
            "lightburn-dxf",
            "LightBurn (DXF)",
            "dxf",
            mm,
            &lightburn,
            lb_bed,
        ),
        preset(
            "cricut",
            "Cricut Design Space (SVG)",
            "svg",
            inch,
            &vinyl,
            mat12,
        ),
        preset(
            "silhouette-basic-12",
            "Silhouette Studio Basic, Cameo 12\" mat (DXF R12)",
            "dxf",
            inch,
            &vinyl,
            mat12,
        ),
        preset(
            "silhouette-basic-24",
            "Silhouette Studio Basic, Cameo 24\" mat (DXF R12)",
            "dxf",
            inch,
            &vinyl,
            bed(12.0 * IN, 24.0 * IN, Origin::TopLeft),
        ),
        preset(
            "silhouette-designer-plus",
            "Silhouette Designer+ (SVG)",
            "svg",
            inch,
            &vinyl,
            mat12,
        ),
        preset(
            "hpgl",
            "Generic HPGL plotter (HPGL exporter plugin)",
            "hpgl",
            mm,
            &[],
            bed(420.0, 297.0, Origin::BottomLeft),
        ),
        preset("generic-svg", "Generic SVG", "svg", mm, &[], None),
        preset("generic-dxf", "Generic DXF (R12)", "dxf", mm, &[], None),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presets_are_valid_unique_and_complete() {
        let ps = presets();
        let mut ids: Vec<&str> = ps.iter().map(|p| p.id.as_str()).collect();
        for p in &ps {
            p.validate().unwrap();
        }
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), ps.len(), "ids are unique");
        // Both Silhouette tiers are separate presets.
        for id in [
            "lightburn",
            "cricut",
            "silhouette-basic-12",
            "silhouette-designer-plus",
            "hpgl",
            "generic-svg",
            "generic-dxf",
        ] {
            assert!(is_builtin(id), "{id}");
        }
        let lb = find("lightburn", &[]).unwrap();
        assert_eq!(lb.bed.unwrap().width_mm, 400.0, "generic 400 mm laser bed");
    }

    #[test]
    fn json_round_trips() {
        for p in presets() {
            let back = MachineProfile::from_json(&p.to_json().unwrap()).unwrap();
            assert_eq!(back, p);
        }
    }

    #[test]
    fn hostile_imports_are_refused_without_panicking() {
        let good = find("cricut", &[]).unwrap().to_json().unwrap();
        for (from, to) in [("\"cricut\"", "\"../x\""), ("\"svg\"", "\"SVG!\"")] {
            let bad = good.replacen(from, to, 1);
            assert_ne!(bad, good);
            assert!(MachineProfile::from_json(&bad).is_err(), "{to}");
        }
        assert!(matches!(
            MachineProfile::from_json(&"x".repeat(MAX_PROFILE_JSON_BYTES + 1)),
            Err(ProfileError::TooLarge(_))
        ));
        for junk in ["", "{", "null", "[]", "{\"id\":1}"] {
            assert!(MachineProfile::from_json(junk).is_err(), "{junk}");
        }
        let mut p = find("generic-svg", &[]).unwrap();
        for w in [f64::NAN, 1e9, -5.0, 0.0] {
            p.bed = Some(Bed {
                width_mm: w,
                height_mm: 10.0,
                origin: Origin::TopLeft,
            });
            assert!(p.validate().is_err(), "{w}");
        }
        p.bed = None;
        p.name = "bad\u{7}name".into();
        assert!(p.validate().is_err());
        p.name = "ok".into();
        p.colours.insert(OperationKind::Reference, [0, 0, 0]);
        assert!(p.validate().is_err());
        p.colours.clear();
        p.defaults.kerf_mm = Some(f64::INFINITY);
        assert!(p.validate().is_err());
    }

    #[test]
    fn bed_sits_in_the_drawing_quadrant_and_the_origin_marks_a_corner() {
        let mk = |origin| Bed {
            width_mm: 10.0,
            height_mm: 4.0,
            origin,
        };
        for o in Origin::ALL {
            let r = mk(o).rect();
            assert_eq!((r.min, r.max), (Vec2::ZERO, Vec2::new(10.0, 4.0)));
        }
        // Y is up: the top of the bed is y = height.
        assert_eq!(mk(Origin::TopLeft).origin_point(), Vec2::new(0.0, 4.0));
        assert_eq!(mk(Origin::TopRight).origin_point(), Vec2::new(10.0, 4.0));
        assert_eq!(mk(Origin::BottomLeft).origin_point(), Vec2::ZERO);
        assert_eq!(mk(Origin::BottomRight).origin_point(), Vec2::new(10.0, 0.0));
    }

    #[test]
    fn defaults_fill_operation_params() {
        let d = Defaults {
            kerf_mm: Some(0.2),
            blade_offset_mm: Some(0.5),
            pen_width_mm: Some(0.7),
        };
        assert!(
            matches!(d.operation(OperationKind::Cut), Operation::Cut(p) if p.kerf_width == 0.2)
        );
        assert!(
            matches!(d.operation(OperationKind::DragKnife), Operation::DragKnife(p) if p.blade_offset == 0.5)
        );
        assert!(
            matches!(d.operation(OperationKind::Draw), Operation::Draw(p) if p.pen_width == 0.7)
        );
        assert!(matches!(
            Defaults::default().operation(OperationKind::Score),
            Operation::Score
        ));
    }
}
