// SPDX-License-Identifier: AGPL-3.0-or-later
//! The saved dock layout (#294, M5 design §7.4).
//!
//! The layout is stored per user in app settings, never in documents, as
//! `locus.layout.v2`: `{ "version": 2, "dock": DockState<Tab>, "windows": [] }`.
//! `windows` will record panes undocked into their own OS windows (#285).
//!
//! Loading, in order:
//! 1. v2, if it parses, is version 2 and still holds the canvas;
//! 2. otherwise v1 (`locus.layout.v1`, the bare `DockState` older builds
//!    wrote), migrated tab by tab;
//! 3. otherwise the default layout.
//!
//! A newer version than this build knows falls back to the default with a
//! warning. The v1 value is never overwritten, so an older build still opens
//! with its own layout. Stored layouts are untrusted input: nothing here
//! panics on them (CLAUDE.md).

use egui_dock::{DockState, NodeIndex};
use serde::{Deserialize, Serialize};

pub const LAYOUT_V1_KEY: &str = "locus.layout.v1";
pub const LAYOUT_V2_KEY: &str = "locus.layout.v2";
const VERSION: u32 = 2;

/// The panes the dock can hold.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Tab {
    Canvas,
    Properties,
    Layers,
    Constraints,
    /// Regions and boolean recipes (#124).
    Regions,
    /// Named parameters for dimension expressions (#171).
    Parameters,
    /// Linear and circular patterns (#168, #169).
    Patterns,
}

impl Tab {
    pub fn title(self) -> &'static str {
        match self {
            Tab::Canvas => "Sketch",
            Tab::Properties => "Properties",
            Tab::Layers => "Layers",
            Tab::Constraints => "Constraints",
            Tab::Regions => "Regions",
            Tab::Parameters => "Parameters",
            Tab::Patterns => "Patterns",
        }
    }
}

/// The tabs as v1 stored them. Frozen: v1 layouts on users' machines use
/// these names whatever [`Tab`] becomes.
mod v1 {
    use serde::Deserialize;

    #[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
    #[cfg_attr(test, derive(serde::Serialize))]
    pub enum Tab {
        Canvas,
        Properties,
        Layers,
        Constraints,
        Regions,
        Parameters,
        Patterns,
    }
}

/// Where each v1 tab lives now. When the M5 panes land (design §2: Layers,
/// Regions and Patterns into Browser, Properties into Inspector), this is
/// the one place that changes, along with a version bump.
fn migrate_tab(t: v1::Tab) -> Tab {
    match t {
        v1::Tab::Canvas => Tab::Canvas,
        v1::Tab::Properties => Tab::Properties,
        v1::Tab::Layers => Tab::Layers,
        v1::Tab::Constraints => Tab::Constraints,
        v1::Tab::Regions => Tab::Regions,
        v1::Tab::Parameters => Tab::Parameters,
        v1::Tab::Patterns => Tab::Patterns,
    }
}

/// A pane undocked into its own OS window (#285).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Window {
    pub tab: Tab,
    pub pos: [f32; 2],
    pub size: [f32; 2],
    pub monitor: Option<String>,
}

#[derive(Serialize, Deserialize)]
struct Saved {
    version: u32,
    dock: DockState<Tab>,
    #[serde(default)]
    windows: Vec<Window>,
}

/// Only the version, so a newer layout can be told from a corrupt one.
#[derive(Deserialize)]
struct Version {
    version: u32,
}

pub fn default_layout() -> DockState<Tab> {
    let mut dock = DockState::new(vec![Tab::Canvas]);
    let tree = dock.main_surface_mut();
    let [_canvas, right] = tree.split_right(
        NodeIndex::root(),
        0.76,
        vec![
            Tab::Properties,
            Tab::Layers,
            Tab::Regions,
            Tab::Parameters,
            Tab::Patterns,
        ],
    );
    tree.split_below(right, 0.55, vec![Tab::Constraints]);
    dock
}

/// egui_dock stores each leaf's last on-screen rectangle. A leaf that has
/// not been laid out yet has non-finite coordinates, which JSON writes as
/// `null` and can't read back, so such a layout would otherwise be thrown
/// away. The rectangles are recomputed every frame, so zero is a safe
/// stand-in.
fn sanitize(json: &str) -> String {
    json.replace("\"x\":null", "\"x\":0.0")
        .replace("\"y\":null", "\"y\":0.0")
}

fn has_canvas(d: &DockState<Tab>) -> bool {
    d.iter_all_tabs().any(|(_, t)| *t == Tab::Canvas)
}

/// The layout to open with, from the stored v2 and v1 values (§7.4).
pub fn load(v2: Option<&str>, v1: Option<&str>) -> DockState<Tab> {
    if let Some(json) = v2 {
        match serde_json::from_str::<Saved>(&sanitize(json)) {
            Ok(s) if s.version == VERSION && has_canvas(&s.dock) => {
                // No pane can be undocked yet (#285), so every saved window
                // is re-docked by being dropped: its tab is still in `dock`.
                return s.dock;
            }
            _ => match serde_json::from_str::<Version>(json) {
                Ok(v) if v.version > VERSION => {
                    tracing::warn!("saved layout is from a newer Locus, using the default");
                    return default_layout();
                }
                _ => tracing::warn!("saved layout unusable, trying the older one"),
            },
        }
    }
    if let Some(json) = v1
        && let Some(d) = migrate_v1(json)
    {
        return d;
    }
    default_layout()
}

/// A v1 layout in today's tabs, or `None` if it doesn't parse or has no
/// canvas. A tab that two v1 tabs map onto is kept once, where the first
/// one was.
fn migrate_v1(json: &str) -> Option<DockState<Tab>> {
    let old = serde_json::from_str::<DockState<v1::Tab>>(&sanitize(json)).ok()?;
    let mut seen = Vec::new();
    let dock = old.filter_map_tabs(|t| {
        let t = migrate_tab(*t);
        (!seen.contains(&t)).then(|| {
            seen.push(t);
            t
        })
    });
    has_canvas(&dock).then_some(dock)
}

/// The v2 value to store.
pub fn save(dock: &DockState<Tab>) -> Option<String> {
    let saved = Saved {
        version: VERSION,
        dock: dock.clone(),
        windows: Vec::new(),
    };
    serde_json::to_string(&saved).ok()
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn tabs(d: &DockState<Tab>) -> Vec<Tab> {
        d.iter_all_tabs().map(|(_, t)| *t).collect()
    }

    /// What v1 builds wrote for their default layout.
    fn v1_default() -> String {
        serde_json::to_string(&default_layout().map_tabs(|t| match t {
            Tab::Canvas => v1::Tab::Canvas,
            Tab::Properties => v1::Tab::Properties,
            Tab::Layers => v1::Tab::Layers,
            Tab::Constraints => v1::Tab::Constraints,
            Tab::Regions => v1::Tab::Regions,
            Tab::Parameters => v1::Tab::Parameters,
            Tab::Patterns => v1::Tab::Patterns,
        }))
        .unwrap()
    }

    #[test]
    fn v1_default_migrates_to_the_v2_default() {
        let d = load(None, Some(&v1_default()));
        assert_eq!(tabs(&d), tabs(&default_layout()));
    }

    #[test]
    fn a_rearranged_v1_layout_loses_no_tab() {
        // Every tab moved: one stack, reversed, Canvas last.
        let all = vec![
            v1::Tab::Patterns,
            v1::Tab::Parameters,
            v1::Tab::Regions,
            v1::Tab::Constraints,
            v1::Tab::Layers,
            v1::Tab::Properties,
            v1::Tab::Canvas,
        ];
        let json = serde_json::to_string(&DockState::new(all)).unwrap();
        let d = load(None, Some(&json));
        assert_eq!(tabs(&d).len(), 7);
        assert_eq!(tabs(&d).last(), Some(&Tab::Canvas));
    }

    #[test]
    fn v2_round_trips_and_wins_over_v1() {
        let mine = DockState::new(vec![Tab::Canvas, Tab::Layers]);
        let json = save(&mine).unwrap();
        assert!(json.contains("\"version\":2"));
        let d = load(Some(&json), Some(&v1_default()));
        assert_eq!(tabs(&d), vec![Tab::Canvas, Tab::Layers]);
    }

    #[test]
    fn corrupt_values_fall_back() {
        for bad in [
            "",
            "{",
            "null",
            "42",
            "{\"surfaces\":[]}",
            "{\"version\":2}",
        ] {
            let d = load(Some(bad), Some(bad));
            assert_eq!(tabs(&d), tabs(&default_layout()), "{bad:?}");
        }
        // A corrupt v2 still lets a good v1 through.
        let d = load(Some("{"), Some(&v1_default()));
        assert_eq!(tabs(&d), tabs(&default_layout()));
    }

    #[test]
    fn a_layout_without_the_canvas_falls_back() {
        let v1 = serde_json::to_string(&DockState::new(vec![v1::Tab::Layers])).unwrap();
        assert!(has_canvas(&load(None, Some(&v1))));
        let v2 = save(&DockState::new(vec![Tab::Layers])).unwrap();
        assert!(has_canvas(&load(Some(&v2), None)));
    }

    #[test]
    fn a_newer_version_falls_back_to_the_default() {
        let mine = DockState::new(vec![Tab::Canvas]);
        let json = save(&mine)
            .unwrap()
            .replace("\"version\":2", "\"version\":9");
        let d = load(Some(&json), Some(&v1_default()));
        assert_eq!(tabs(&d), tabs(&default_layout()));
    }

    /// A layout saved before its panes were drawn has `null` rectangles;
    /// it still loads (it used to fall back to the default silently).
    #[test]
    fn a_layout_with_unset_rectangles_still_loads() {
        let json = save(&DockState::new(vec![Tab::Canvas, Tab::Parameters])).unwrap();
        assert!(json.contains("null"));
        assert_eq!(
            tabs(&load(Some(&json), None)),
            vec![Tab::Canvas, Tab::Parameters]
        );
    }

    #[test]
    fn nothing_stored_is_the_default() {
        assert_eq!(tabs(&load(None, None)), tabs(&default_layout()));
    }
}
