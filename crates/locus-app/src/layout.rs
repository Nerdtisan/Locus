// SPDX-License-Identifier: AGPL-3.0-or-later
//! The saved dock layout (#294, #285, M5 design §2, §7.4).
//!
//! The layout is stored per user in app settings, never in documents, as
//! `locus.layout.v3`: `{ "version": 3, "dock": DockState<Tab>, "windows": [...] }`.
//! `windows` records panes undocked into their own OS windows (#285).
//!
//! Loading, in order:
//! 1. v3, if it parses, is version 3 and still holds the canvas;
//! 2. otherwise v2 (`locus.layout.v2`, #294) or else v1 (`locus.layout.v1`,
//!    the bare `DockState` older builds wrote), migrated tab by tab to the
//!    M5 panes: Layers, Regions and Patterns into Browser, Properties into
//!    Inspector, then Palette added above Inspector;
//! 3. otherwise the default layout.
//!
//! A newer version than this build knows falls back to the default with a
//! warning. Older values are never overwritten, so an older build still
//! opens with its own layout. Stored layouts are untrusted input: nothing
//! here panics on them, and window geometry is bounded (CLAUDE.md).

use egui_dock::{DockState, NodeIndex, NodePath};
use serde::{Deserialize, Serialize};

pub const LAYOUT_V1_KEY: &str = "locus.layout.v1";
pub const LAYOUT_V2_KEY: &str = "locus.layout.v2";
pub const LAYOUT_V3_KEY: &str = "locus.layout.v3";
const VERSION: u32 = 3;

/// The panes the dock can hold (design §2).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Tab {
    Canvas,
    /// Layers, recipes (patterns) and regions in one tree.
    Browser,
    /// The context-sensitive properties (design §2.1).
    Inspector,
    /// Linetype, display toggles and the active tool's options.
    Palette,
    /// Named parameters for dimension expressions (#171).
    Parameters,
    Constraints,
    /// The shortcut map (#299, design §5.1). Not in the default layout;
    /// `?` adds it.
    Shortcuts,
}

impl Tab {
    /// Every pane but the canvas, in View ▸ Panes order.
    pub const PANES: [Tab; 6] = [
        Tab::Browser,
        Tab::Palette,
        Tab::Inspector,
        Tab::Parameters,
        Tab::Constraints,
        Tab::Shortcuts,
    ];

    pub fn title(self) -> &'static str {
        match self {
            Tab::Canvas => "Sketch",
            Tab::Browser => "Browser",
            Tab::Inspector => "Inspector",
            Tab::Palette => "Sketch palette",
            Tab::Parameters => "Parameters",
            Tab::Constraints => "Constraints",
            Tab::Shortcuts => "Shortcuts",
        }
    }

    /// The pane a re-docked tab joins, if it is open (design §2).
    pub fn neighbour(self) -> Option<Tab> {
        match self {
            Tab::Canvas => None,
            Tab::Browser => Some(Tab::Parameters),
            Tab::Parameters => Some(Tab::Browser),
            Tab::Inspector | Tab::Shortcuts => Some(Tab::Constraints),
            Tab::Constraints => Some(Tab::Inspector),
            Tab::Palette => Some(Tab::Inspector),
        }
    }
}

/// The tabs as v1 and v2 stored them (v2 added Shortcuts, #299). Frozen:
/// layouts on users' machines use these names whatever [`Tab`] becomes.
mod v2 {
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
        Shortcuts,
    }
}

/// Where each v1/v2 tab lives now (design §7.4).
fn migrate_tab(t: v2::Tab) -> Tab {
    match t {
        v2::Tab::Canvas => Tab::Canvas,
        v2::Tab::Properties => Tab::Inspector,
        v2::Tab::Layers | v2::Tab::Regions | v2::Tab::Patterns => Tab::Browser,
        v2::Tab::Parameters => Tab::Parameters,
        v2::Tab::Constraints => Tab::Constraints,
        v2::Tab::Shortcuts => Tab::Shortcuts,
    }
}

/// Bounds on stored window geometry, in points (untrusted input).
const MIN_SIZE: f32 = 120.0;
const MAX_SIZE: f32 = 8000.0;
const MAX_POS: f32 = 50_000.0;

/// A pane undocked into its own OS window (#285).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Window {
    pub tab: Tab,
    /// Outer top-left on the desktop, in points.
    #[serde(deserialize_with = "lenient_pair")]
    pub pos: [f32; 2],
    #[serde(deserialize_with = "lenient_pair")]
    pub size: [f32; 2],
    /// The monitor it was on, as its size ("2560x1440"); informational.
    pub monitor: Option<String>,
}

/// A coordinate pair where `null` (how JSON writes a non-finite float)
/// reads as NaN, so one bad window can't make the whole layout unreadable;
/// [`Window::sanitized`] then re-docks it.
fn lenient_pair<'de, D: serde::Deserializer<'de>>(d: D) -> Result<[f32; 2], D::Error> {
    let [a, b] = <[Option<f32>; 2]>::deserialize(d)?;
    Ok([a.unwrap_or(f32::NAN), b.unwrap_or(f32::NAN)])
}

impl Window {
    /// Bounded geometry, or `None` if it can't be used (re-docked instead).
    fn sanitized(mut self) -> Option<Self> {
        let finite = self.pos.iter().chain(&self.size).all(|v| v.is_finite());
        if !finite || self.tab == Tab::Canvas {
            return None;
        }
        for p in &mut self.pos {
            *p = p.clamp(-MAX_POS, MAX_POS);
        }
        for s in &mut self.size {
            *s = s.clamp(MIN_SIZE, MAX_SIZE);
        }
        if self.monitor.as_ref().is_some_and(|m| m.len() > 32) {
            self.monitor = None;
        }
        Some(self)
    }
}

#[derive(Serialize, Deserialize)]
struct Saved {
    version: u32,
    dock: DockState<Tab>,
    #[serde(default)]
    windows: Vec<Window>,
}

/// v2 as #294 wrote it; its windows were always empty.
#[derive(Deserialize)]
struct SavedV2 {
    version: u32,
    dock: DockState<v2::Tab>,
}

/// Only the version, so a newer layout can be told from a corrupt one.
#[derive(Deserialize)]
struct Version {
    version: u32,
}

/// A loaded layout: the dock and the panes in their own windows.
#[derive(Clone)]
pub struct Layout {
    pub dock: DockState<Tab>,
    pub windows: Vec<Window>,
}

impl Layout {
    fn docked(dock: DockState<Tab>) -> Self {
        Self {
            dock,
            windows: Vec::new(),
        }
    }
}

pub fn default_layout() -> DockState<Tab> {
    let mut dock = DockState::new(vec![Tab::Canvas]);
    let tree = dock.main_surface_mut();
    let [canvas, right] = tree.split_right(NodeIndex::root(), 0.78, vec![Tab::Palette]);
    tree.split_below(right, 0.35, vec![Tab::Inspector, Tab::Constraints]);
    tree.split_left(canvas, 0.22, vec![Tab::Browser, Tab::Parameters]);
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

/// The layout to open with, from the stored v3, v2 and v1 values (§7.4).
pub fn load(v3: Option<&str>, v2: Option<&str>, v1: Option<&str>) -> Layout {
    if let Some(json) = v3 {
        match serde_json::from_str::<Saved>(&sanitize(json)) {
            Ok(s) if s.version == VERSION && has_canvas(&s.dock) => {
                let mut dock = s.dock;
                let mut windows = Vec::new();
                for w in s.windows {
                    let tab = w.tab;
                    // A pane in both places, twice, or with unusable
                    // geometry stays (or goes back) in the dock.
                    let dup = dock.find_tab(&tab).is_some()
                        || windows.iter().any(|o: &Window| o.tab == tab);
                    match w.sanitized() {
                        Some(w) if !dup => windows.push(w),
                        _ if !dup => redock(&mut dock, tab),
                        _ => {}
                    }
                }
                return Layout { dock, windows };
            }
            _ => match serde_json::from_str::<Version>(json) {
                Ok(v) if v.version > VERSION => {
                    tracing::warn!("saved layout is from a newer Locus, using the default");
                    return Layout::docked(default_layout());
                }
                _ => tracing::warn!("saved layout unusable, trying the older one"),
            },
        }
    }
    if let Some(json) = v2
        && let Ok(s) = serde_json::from_str::<SavedV2>(&sanitize(json))
        && s.version == 2
        && let Some(d) = migrate(&s.dock)
    {
        return Layout::docked(d);
    }
    if let Some(json) = v1
        && let Ok(old) = serde_json::from_str::<DockState<v2::Tab>>(&sanitize(json))
        && let Some(d) = migrate(&old)
    {
        return Layout::docked(d);
    }
    Layout::docked(default_layout())
}

/// An older layout in today's tabs, or `None` if it has no canvas. A tab
/// that several old tabs map onto is kept once, where the first one was;
/// Palette is added above Inspector if missing.
fn migrate(old: &DockState<v2::Tab>) -> Option<DockState<Tab>> {
    let mut seen = Vec::new();
    let mut dock = old.filter_map_tabs(|t| {
        let t = migrate_tab(*t);
        (!seen.contains(&t)).then(|| {
            seen.push(t);
            t
        })
    });
    if !has_canvas(&dock) {
        return None;
    }
    if dock.find_tab(&Tab::Palette).is_none() {
        match dock.find_tab(&Tab::Inspector) {
            Some(at) => {
                dock[at.surface].split_above(at.node, 0.4, vec![Tab::Palette]);
            }
            None => redock(&mut dock, Tab::Palette),
        }
    }
    Some(dock)
}

/// Put `tab` back in the dock: beside its neighbour if that is docked,
/// otherwise in a new pane right of the canvas.
pub fn redock(dock: &mut DockState<Tab>, tab: Tab) {
    if dock.find_tab(&tab).is_some() {
        return;
    }
    let near = tab.neighbour().and_then(|n| dock.find_tab(&n));
    match near {
        Some(at) => {
            dock.set_focused_node_and_surface(NodePath {
                surface: at.surface,
                node: at.node,
            });
            dock.push_to_focused_leaf(tab);
        }
        None => match dock.find_main_surface_tab(&Tab::Canvas) {
            Some((node, _)) => {
                dock.main_surface_mut().split_right(node, 0.78, vec![tab]);
            }
            None => dock.push_to_first_leaf(tab),
        },
    }
}

/// The v3 value to store.
pub fn save(dock: &DockState<Tab>, windows: &[Window]) -> Option<String> {
    let saved = Saved {
        version: VERSION,
        dock: dock.clone(),
        windows: windows.to_vec(),
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

    fn sorted(mut v: Vec<Tab>) -> Vec<Tab> {
        v.sort_by_key(|t| *t as u8);
        v
    }

    /// What v1 builds wrote for their default layout.
    fn v1_default() -> String {
        use v2::Tab as O;
        let mut dock = DockState::new(vec![O::Canvas]);
        let tree = dock.main_surface_mut();
        let [_c, right] = tree.split_right(
            NodeIndex::root(),
            0.76,
            vec![
                O::Properties,
                O::Layers,
                O::Regions,
                O::Parameters,
                O::Patterns,
            ],
        );
        tree.split_below(right, 0.55, vec![O::Constraints]);
        serde_json::to_string(&dock).unwrap()
    }

    fn v2_of(json_dock: &str) -> String {
        format!("{{\"version\":2,\"dock\":{json_dock},\"windows\":[]}}")
    }

    #[test]
    fn v1_default_migrates_to_the_m5_panes() {
        let d = load(None, None, Some(&v1_default())).dock;
        assert_eq!(
            sorted(tabs(&d)),
            sorted(vec![
                Tab::Canvas,
                Tab::Browser,
                Tab::Inspector,
                Tab::Palette,
                Tab::Parameters,
                Tab::Constraints
            ])
        );
        // Palette sits above Inspector, in its own leaf.
        let p = d.find_tab(&Tab::Palette).unwrap();
        let i = d.find_tab(&Tab::Inspector).unwrap();
        assert_ne!(p.node, i.node);
    }

    #[test]
    fn v2_migrates_too_and_keeps_shortcuts() {
        use v2::Tab as O;
        let old = DockState::new(vec![O::Canvas, O::Layers, O::Shortcuts]);
        let json = v2_of(&serde_json::to_string(&old).unwrap());
        let d = load(None, Some(&json), Some(&v1_default())).dock;
        let t = tabs(&d);
        assert!(t.contains(&Tab::Shortcuts) && t.contains(&Tab::Browser));
        assert!(t.contains(&Tab::Palette), "palette added");
    }

    #[test]
    fn a_rearranged_v1_layout_loses_no_pane() {
        use v2::Tab as O;
        // Every tab moved: one stack, reversed, Canvas last.
        let all = vec![
            O::Patterns,
            O::Parameters,
            O::Regions,
            O::Constraints,
            O::Layers,
            O::Properties,
            O::Canvas,
        ];
        let json = serde_json::to_string(&DockState::new(all)).unwrap();
        let d = load(None, None, Some(&json)).dock;
        let t = tabs(&d);
        for want in [
            Tab::Browser,
            Tab::Parameters,
            Tab::Constraints,
            Tab::Inspector,
            Tab::Canvas,
            Tab::Palette,
        ] {
            assert_eq!(t.iter().filter(|x| **x == want).count(), 1, "{want:?}");
        }
    }

    #[test]
    fn v3_round_trips_with_windows_and_wins() {
        let dock = DockState::new(vec![Tab::Canvas, Tab::Browser]);
        let w = Window {
            tab: Tab::Shortcuts,
            pos: [2600.0, 100.0],
            size: [400.0, 600.0],
            monitor: Some("1920x1080".into()),
        };
        let json = save(&dock, std::slice::from_ref(&w)).unwrap();
        assert!(json.contains("\"version\":3"));
        let l = load(Some(&json), Some(&v2_of("{}")), Some(&v1_default()));
        assert_eq!(tabs(&l.dock), vec![Tab::Canvas, Tab::Browser]);
        assert_eq!(l.windows, vec![w]);
    }

    #[test]
    fn bad_windows_are_bounded_or_redocked() {
        let dock = DockState::new(vec![Tab::Canvas, Tab::Browser]);
        let json = save(
            &dock,
            &[
                // Unusable geometry: back into the dock.
                Window {
                    tab: Tab::Inspector,
                    pos: [f32::NAN, 0.0],
                    size: [300.0, 300.0],
                    monitor: None,
                },
                // Huge: clamped.
                Window {
                    tab: Tab::Palette,
                    pos: [1e9, -1e9],
                    size: [1.0, 1e9],
                    monitor: Some("x".repeat(100)),
                },
                // Also docked: the dock wins.
                Window {
                    tab: Tab::Browser,
                    pos: [0.0, 0.0],
                    size: [300.0, 300.0],
                    monitor: None,
                },
                // The canvas never undocks.
                Window {
                    tab: Tab::Canvas,
                    pos: [0.0, 0.0],
                    size: [300.0, 300.0],
                    monitor: None,
                },
            ],
        )
        .unwrap();
        // JSON has no NaN: serde writes null, which must not break loading.
        assert!(json.contains("[null,0.0]"));
        let l = load(Some(&json), None, None);
        assert!(l.dock.find_tab(&Tab::Inspector).is_some());
        assert_eq!(l.windows.len(), 1);
        let p = &l.windows[0];
        assert_eq!(p.tab, Tab::Palette);
        assert_eq!(p.pos, [MAX_POS, -MAX_POS]);
        assert_eq!(p.size, [MIN_SIZE, MAX_SIZE]);
        assert_eq!(p.monitor, None);
        assert_eq!(
            tabs(&l.dock).iter().filter(|t| **t == Tab::Browser).count(),
            1
        );
    }

    #[test]
    fn corrupt_values_fall_back() {
        for bad in [
            "",
            "{",
            "null",
            "42",
            "{\"surfaces\":[]}",
            "{\"version\":3}",
            "{\"version\":2}",
        ] {
            let d = load(Some(bad), Some(bad), Some(bad)).dock;
            assert_eq!(tabs(&d), tabs(&default_layout()), "{bad:?}");
        }
        // A corrupt v3 still lets a good v1 through.
        let d = load(Some("{"), Some("{"), Some(&v1_default())).dock;
        assert!(tabs(&d).contains(&Tab::Browser));
    }

    #[test]
    fn a_layout_without_the_canvas_falls_back() {
        use v2::Tab as O;
        let v1 = serde_json::to_string(&DockState::new(vec![O::Layers])).unwrap();
        assert!(has_canvas(&load(None, None, Some(&v1)).dock));
        let v3 = save(&DockState::new(vec![Tab::Browser]), &[]).unwrap();
        assert!(has_canvas(&load(Some(&v3), None, None).dock));
    }

    #[test]
    fn a_newer_version_falls_back_to_the_default() {
        let json = save(&DockState::new(vec![Tab::Canvas]), &[])
            .unwrap()
            .replace("\"version\":3", "\"version\":9");
        let d = load(Some(&json), None, Some(&v1_default())).dock;
        assert_eq!(tabs(&d), tabs(&default_layout()));
    }

    /// A layout saved before its panes were drawn has `null` rectangles;
    /// it still loads.
    #[test]
    fn a_layout_with_unset_rectangles_still_loads() {
        let json = save(&DockState::new(vec![Tab::Canvas, Tab::Parameters]), &[]).unwrap();
        assert!(json.contains("null"));
        assert_eq!(
            tabs(&load(Some(&json), None, None).dock),
            vec![Tab::Canvas, Tab::Parameters]
        );
    }

    #[test]
    fn redock_finds_a_neighbour_or_a_new_pane() {
        let mut d = default_layout();
        let at = d.find_tab(&Tab::Constraints).unwrap();
        redock(&mut d, Tab::Shortcuts);
        assert_eq!(d.find_tab(&Tab::Shortcuts).unwrap().node, at.node);
        let mut lone = DockState::new(vec![Tab::Canvas]);
        redock(&mut lone, Tab::Browser);
        redock(&mut lone, Tab::Browser); // no duplicate
        assert_eq!(tabs(&lone), vec![Tab::Canvas, Tab::Browser]);
    }

    #[test]
    fn nothing_stored_is_the_default() {
        assert_eq!(tabs(&load(None, None, None).dock), tabs(&default_layout()));
    }
}
