// SPDX-License-Identifier: AGPL-3.0-or-later
//! The saved dock layout (#294, #285, M5 design §2, §7.4).
//!
//! The layout is stored per user in app settings, never in documents, as
//! `locus.layout.v4`: `{ "version": 4, "dock": DockState<Tab>, "windows": [...] }`.
//! `windows` records panes undocked into their own OS windows (#285).
//!
//! The default is the approved mockup's (#327): Browser on the left, the
//! canvas, and the Sketch palette over the Inspector on the right, each
//! visible rather than tabbed behind another pane.
//!
//! Loading, in order:
//! 1. v4, if it parses, is version 4 and still holds the canvas;
//! 2. otherwise v3 (#285), the same shape;
//! 3. otherwise v2 (`locus.layout.v2`, #294) or else v1 (`locus.layout.v1`,
//!    the bare `DockState` older builds wrote), migrated tab by tab to the
//!    M5 panes: Layers, Regions and Patterns into Browser, Properties into
//!    Inspector, then Palette added above Inspector;
//! 4. otherwise the default layout.
//!
//! An older layout that is still exactly its version's default (the same
//! stacks of panes, no pane windows) becomes today's default: the user
//! never arranged it, so the new arrangement is what they should see. A
//! layout the user did arrange is kept.
//!
//! A newer version than this build knows falls back to the default with a
//! warning. Older values are never overwritten, so an older build still
//! opens with its own layout. Stored layouts are untrusted input: nothing
//! here panics on them, and window geometry is bounded (CLAUDE.md).

use egui_dock::{DockState, NodeIndex};
use serde::{Deserialize, Serialize};

pub const LAYOUT_V1_KEY: &str = "locus.layout.v1";
pub const LAYOUT_V2_KEY: &str = "locus.layout.v2";
pub const LAYOUT_V3_KEY: &str = "locus.layout.v3";
pub const LAYOUT_V4_KEY: &str = "locus.layout.v4";
const VERSION: u32 = 4;

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

    /// Where a pane goes back into the dock, best first: the first place
    /// whose anchor is docked wins (design §2, the mockup). Panes stack
    /// rather than tab, so the pane already there stays visible.
    fn homes(self) -> &'static [Place] {
        match self {
            Tab::Canvas => &[],
            Tab::Browser => &[Place::Above(Tab::Parameters), Place::LeftOfCanvas],
            Tab::Parameters => &[Place::Below(Tab::Browser), Place::LeftOfCanvas],
            Tab::Palette => &[Place::Above(Tab::Inspector), Place::RightOfCanvas],
            Tab::Inspector => &[Place::Below(Tab::Palette), Place::RightOfCanvas],
            Tab::Constraints => &[Place::Below(Tab::Inspector), Place::RightOfCanvas],
            // Under the Inspector's column, where the mockup docks it (§5.1).
            Tab::Shortcuts => &[
                Place::Below(Tab::Constraints),
                Place::Below(Tab::Inspector),
                Place::RightOfCanvas,
            ],
        }
    }
}

/// A place in the dock, relative to a docked pane or to the canvas.
#[derive(Clone, Copy, Debug)]
enum Place {
    Above(Tab),
    Below(Tab),
    LeftOfCanvas,
    RightOfCanvas,
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

/// The mockup's arrangement (#327, design §2): Browser | canvas | Sketch
/// palette over Inspector. Parameters, Constraints and Shortcuts are panes
/// the user opens from View ▸ Panes (Shortcuts with ?).
pub fn default_layout() -> DockState<Tab> {
    let mut dock = DockState::new(vec![Tab::Canvas]);
    let tree = dock.main_surface_mut();
    // Each fraction is the share of the left or upper part.
    let [canvas, _browser] = tree.split_left(NodeIndex::root(), 0.16, vec![Tab::Browser]);
    let [_canvas, right] = tree.split_right(canvas, 0.80, vec![Tab::Palette]);
    tree.split_below(right, 0.36, vec![Tab::Inspector]);
    dock
}

/// The v3 default (#285), which v3 builds wrote until the user moved a pane.
const V3_DEFAULT: &[&[Tab]] = &[
    &[Tab::Browser, Tab::Parameters],
    &[Tab::Canvas],
    &[Tab::Palette],
    &[Tab::Inspector, Tab::Constraints],
];

/// The v1 and v2 default (before M5), in their own tab names.
const V1_DEFAULT: &[&[v2::Tab]] = &[
    &[v2::Tab::Canvas],
    &[
        v2::Tab::Properties,
        v2::Tab::Layers,
        v2::Tab::Regions,
        v2::Tab::Parameters,
        v2::Tab::Patterns,
    ],
    &[v2::Tab::Constraints],
];

/// Is `d` still `stock`: the same stacks of panes, whatever their sizes or
/// the tab order within a stack, and nothing floating?
fn is_stock<T: PartialEq + Clone>(d: &DockState<T>, stock: &[&[T]]) -> bool {
    let mut leaves: Vec<Vec<T>> = d
        .iter_leaves()
        .map(|(_, l)| l.tabs().to_vec())
        .filter(|t| !t.is_empty())
        .collect();
    if leaves.len() != stock.len() {
        return false;
    }
    for want in stock {
        let same = |l: &Vec<T>| l.len() == want.len() && want.iter().all(|t| l.contains(t));
        match leaves.iter().position(same) {
            Some(i) => {
                leaves.swap_remove(i);
            }
            None => return false,
        }
    }
    true
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

/// Why a stored value in the current shape (v3 and v4 share it) can't be used.
enum Unparsed {
    /// Written by a newer Locus than this one.
    Newer,
    Unusable,
}

fn parse(json: &str, version: u32) -> Result<Layout, Unparsed> {
    match serde_json::from_str::<Saved>(&sanitize(json)) {
        Ok(s) if s.version == version && has_canvas(&s.dock) => {
            let mut dock = s.dock;
            let mut windows = Vec::new();
            for w in s.windows {
                let tab = w.tab;
                // A pane in both places, twice, or with unusable
                // geometry stays (or goes back) in the dock.
                let dup =
                    dock.find_tab(&tab).is_some() || windows.iter().any(|o: &Window| o.tab == tab);
                match w.sanitized() {
                    Some(w) if !dup => windows.push(w),
                    _ if !dup => redock(&mut dock, tab),
                    _ => {}
                }
            }
            Ok(Layout { dock, windows })
        }
        _ => match serde_json::from_str::<Version>(json) {
            Ok(v) if v.version > VERSION => Err(Unparsed::Newer),
            _ => Err(Unparsed::Unusable),
        },
    }
}

/// The layout to open with, from the stored v4, v3, v2 and v1 values (§7.4).
pub fn load(v4: Option<&str>, v3: Option<&str>, v2: Option<&str>, v1: Option<&str>) -> Layout {
    if let Some(json) = v4 {
        match parse(json, VERSION) {
            Ok(l) => return l,
            Err(Unparsed::Newer) => {
                tracing::warn!("saved layout is from a newer Locus, using the default");
                return Layout::docked(default_layout());
            }
            Err(Unparsed::Unusable) => {
                tracing::warn!("saved layout unusable, trying the older one")
            }
        }
    }
    if let Some(json) = v3
        && let Ok(l) = parse(json, 3)
    {
        return if l.windows.is_empty() && is_stock(&l.dock, V3_DEFAULT) {
            Layout::docked(default_layout())
        } else {
            l
        };
    }
    let v2 = v2
        .and_then(|json| serde_json::from_str::<SavedV2>(&sanitize(json)).ok())
        .filter(|s| s.version == 2)
        .map(|s| s.dock);
    let v1 = v1.and_then(|json| serde_json::from_str::<DockState<v2::Tab>>(&sanitize(json)).ok());
    for old in v2.into_iter().chain(v1) {
        if is_stock(&old, V1_DEFAULT) {
            return Layout::docked(default_layout());
        }
        if let Some(d) = migrate(&old) {
            return Layout::docked(d);
        }
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
    redock(&mut dock, Tab::Palette);
    Some(dock)
}

/// Put `tab` back in the dock at the first of its homes that exists:
/// above or below a docked pane, or beside the canvas. No-op if docked.
pub fn redock(dock: &mut DockState<Tab>, tab: Tab) {
    if tab == Tab::Canvas || dock.find_tab(&tab).is_some() {
        return;
    }
    for place in tab.homes() {
        let anchor = match *place {
            Place::Above(t) | Place::Below(t) => t,
            Place::LeftOfCanvas | Place::RightOfCanvas => Tab::Canvas,
        };
        let Some(at) = dock.find_tab(&anchor) else {
            continue;
        };
        let tree = &mut dock[at.surface];
        match *place {
            Place::Above(_) => tree.split_above(at.node, 0.36, vec![tab]),
            Place::Below(_) => tree.split_below(at.node, 0.6, vec![tab]),
            Place::LeftOfCanvas => tree.split_left(at.node, 0.18, vec![tab]),
            Place::RightOfCanvas => tree.split_right(at.node, 0.8, vec![tab]),
        };
        return;
    }
    dock.push_to_first_leaf(tab);
}

/// The v4 value to store.
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

    /// The mockup's stacks (#327).
    const MOCKUP: &[&[Tab]] = &[
        &[Tab::Browser],
        &[Tab::Canvas],
        &[Tab::Palette],
        &[Tab::Inspector],
    ];

    /// What v1 (and v2) builds wrote for their default layout.
    fn v1_default_dock() -> DockState<v2::Tab> {
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
        dock
    }

    fn v1_default() -> String {
        serde_json::to_string(&v1_default_dock()).unwrap()
    }

    fn v2_of(json_dock: &str) -> String {
        format!("{{\"version\":2,\"dock\":{json_dock},\"windows\":[]}}")
    }

    /// What v3 builds wrote for their default layout (#285).
    fn v3_default_dock() -> DockState<Tab> {
        let mut dock = DockState::new(vec![Tab::Canvas]);
        let tree = dock.main_surface_mut();
        let [canvas, right] = tree.split_right(NodeIndex::root(), 0.78, vec![Tab::Palette]);
        tree.split_below(right, 0.35, vec![Tab::Inspector, Tab::Constraints]);
        tree.split_left(canvas, 0.22, vec![Tab::Browser, Tab::Parameters]);
        dock
    }

    fn v3_of(dock: &DockState<Tab>, windows: &[Window]) -> String {
        save(dock, windows)
            .unwrap()
            .replace("\"version\":4", "\"version\":3")
    }

    fn node(d: &DockState<Tab>, t: Tab) -> NodeIndex {
        d.find_tab(&t).unwrap().node
    }

    #[test]
    fn the_default_is_the_mockup() {
        let d = default_layout();
        assert!(is_stock(&d, MOCKUP));
        // Browser left of everything; Palette over Inspector right of the canvas.
        let root = NodeIndex::root();
        assert_eq!(node(&d, Tab::Browser), root.left());
        assert_eq!(node(&d, Tab::Canvas), root.right().left());
        assert_eq!(node(&d, Tab::Palette), root.right().right().left());
        assert_eq!(node(&d, Tab::Inspector), root.right().right().right());
    }

    #[test]
    fn an_untouched_older_default_becomes_the_mockup() {
        // v3's default, as v3 builds stored it.
        let v3 = v3_of(&v3_default_dock(), &[]);
        assert!(is_stock(&load(None, Some(&v3), None, None).dock, MOCKUP));
        // Tab order inside a stack doesn't count as arranging it.
        let mut shuffled = v3_default_dock();
        let at = shuffled.find_tab(&Tab::Parameters).unwrap();
        shuffled.remove_tab(at);
        shuffled.set_focused_node_and_surface(egui_dock::NodePath {
            surface: at.surface,
            node: at.node,
        });
        shuffled.push_to_focused_leaf(Tab::Parameters);
        let l = load(None, Some(&v3_of(&shuffled, &[])), None, None);
        assert!(is_stock(&l.dock, MOCKUP));
        // v1's default, and v2's, which is the same.
        let v1 = load(None, None, None, Some(&v1_default()));
        assert!(is_stock(&v1.dock, MOCKUP));
        let v2 = load(None, None, Some(&v2_of(&v1_default())), None);
        assert!(is_stock(&v2.dock, MOCKUP));
    }

    #[test]
    fn an_arranged_v3_layout_is_kept() {
        // Constraints moved out under the canvas.
        let mut d = v3_default_dock();
        d.remove_tab(d.find_tab(&Tab::Constraints).unwrap());
        let (canvas, _) = d.find_main_surface_tab(&Tab::Canvas).unwrap();
        d.main_surface_mut()
            .split_below(canvas, 0.7, vec![Tab::Constraints]);
        let l = load(None, Some(&v3_of(&d, &[])), None, None);
        assert_eq!(sorted(tabs(&l.dock)), sorted(tabs(&d)));
        assert_eq!(node(&l.dock, Tab::Constraints), node(&d, Tab::Constraints));
        // The default with a pane in its own window is arranged too.
        let mut d = v3_default_dock();
        d.remove_tab(d.find_tab(&Tab::Parameters).unwrap());
        let w = Window {
            tab: Tab::Parameters,
            pos: [100.0, 100.0],
            size: [300.0, 400.0],
            monitor: None,
        };
        let l = load(None, Some(&v3_of(&d, std::slice::from_ref(&w))), None, None);
        assert_eq!(l.windows, vec![w]);
        assert!(l.dock.find_tab(&Tab::Constraints).is_some());
    }

    #[test]
    fn v2_migrates_too_and_keeps_shortcuts() {
        use v2::Tab as O;
        let old = DockState::new(vec![O::Canvas, O::Layers, O::Shortcuts]);
        let json = v2_of(&serde_json::to_string(&old).unwrap());
        let d = load(None, None, Some(&json), Some(&v1_default())).dock;
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
        let d = load(None, None, None, Some(&json)).dock;
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
        // Palette sits above Inspector, in its own stack.
        assert_ne!(node(&d, Tab::Palette), node(&d, Tab::Inspector));
    }

    #[test]
    fn v4_round_trips_with_windows_and_wins() {
        let dock = DockState::new(vec![Tab::Canvas, Tab::Browser]);
        let w = Window {
            tab: Tab::Shortcuts,
            pos: [2600.0, 100.0],
            size: [400.0, 600.0],
            monitor: Some("1920x1080".into()),
        };
        let json = save(&dock, std::slice::from_ref(&w)).unwrap();
        assert!(json.contains("\"version\":4"));
        let v3 = v3_of(&v3_default_dock(), &[]);
        let l = load(
            Some(&json),
            Some(&v3),
            Some(&v2_of("{}")),
            Some(&v1_default()),
        );
        assert_eq!(tabs(&l.dock), vec![Tab::Canvas, Tab::Browser]);
        assert_eq!(l.windows, vec![w]);
        // A v3 value in the v4 slot is not v4: it falls through.
        assert!(is_stock(&load(Some(&v3), None, None, None).dock, MOCKUP));
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
        let l = load(Some(&json), None, None, None);
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
            "{\"version\":4}",
            "{\"version\":3}",
            "{\"version\":2}",
        ] {
            let d = load(Some(bad), Some(bad), Some(bad), Some(bad)).dock;
            assert_eq!(tabs(&d), tabs(&default_layout()), "{bad:?}");
        }
        // A corrupt v4 still lets a good older one through.
        let mut arranged = v3_default_dock();
        arranged.remove_tab(arranged.find_tab(&Tab::Constraints).unwrap());
        let d = load(Some("{"), Some(&v3_of(&arranged, &[])), None, None).dock;
        assert_eq!(sorted(tabs(&d)), sorted(tabs(&arranged)));
        let d = load(Some("{"), Some("{"), Some("{"), Some(&v1_default())).dock;
        assert!(tabs(&d).contains(&Tab::Browser));
    }

    #[test]
    fn a_layout_without_the_canvas_falls_back() {
        use v2::Tab as O;
        let v1 = serde_json::to_string(&DockState::new(vec![O::Layers])).unwrap();
        assert!(has_canvas(&load(None, None, None, Some(&v1)).dock));
        let v4 = save(&DockState::new(vec![Tab::Browser]), &[]).unwrap();
        assert!(has_canvas(&load(Some(&v4), None, None, None).dock));
        let v3 = v3_of(&DockState::new(vec![Tab::Browser]), &[]);
        assert!(has_canvas(&load(None, Some(&v3), None, None).dock));
    }

    #[test]
    fn a_newer_version_falls_back_to_the_default() {
        let json = save(&DockState::new(vec![Tab::Canvas]), &[])
            .unwrap()
            .replace("\"version\":4", "\"version\":9");
        let d = load(Some(&json), None, None, Some(&v1_default())).dock;
        assert_eq!(tabs(&d), tabs(&default_layout()));
    }

    /// A layout saved before its panes were drawn has `null` rectangles;
    /// it still loads.
    #[test]
    fn a_layout_with_unset_rectangles_still_loads() {
        let json = save(&DockState::new(vec![Tab::Canvas, Tab::Parameters]), &[]).unwrap();
        assert!(json.contains("null"));
        assert_eq!(
            tabs(&load(Some(&json), None, None, None).dock),
            vec![Tab::Canvas, Tab::Parameters]
        );
    }

    /// Re-docked panes stack at their home instead of hiding a pane behind
    /// a tab (#327).
    #[test]
    fn redock_stacks_panes_at_their_home() {
        let mut d = default_layout();
        let inspector = node(&d, Tab::Inspector);
        redock(&mut d, Tab::Shortcuts);
        // Below the Inspector, in its own stack.
        assert_eq!(node(&d, Tab::Inspector), inspector.left());
        assert_eq!(node(&d, Tab::Shortcuts), inspector.right());
        let browser = node(&d, Tab::Browser);
        redock(&mut d, Tab::Parameters);
        assert_eq!(node(&d, Tab::Parameters), browser.right());
        // Palette closed and reopened: above the Inspector again.
        let mut d = default_layout();
        d.remove_tab(d.find_tab(&Tab::Palette).unwrap());
        redock(&mut d, Tab::Palette);
        let i = node(&d, Tab::Inspector);
        assert_eq!(node(&d, Tab::Palette), i.parent().unwrap().left());
        // With nothing to anchor to, beside the canvas; never twice.
        let mut lone = DockState::new(vec![Tab::Canvas]);
        redock(&mut lone, Tab::Browser);
        redock(&mut lone, Tab::Browser);
        redock(&mut lone, Tab::Canvas);
        assert_eq!(sorted(tabs(&lone)), vec![Tab::Canvas, Tab::Browser]);
        assert_eq!(node(&lone, Tab::Browser), NodeIndex::root().left());
    }

    #[test]
    fn nothing_stored_is_the_default() {
        assert_eq!(
            tabs(&load(None, None, None, None).dock),
            tabs(&default_layout())
        );
    }
}
