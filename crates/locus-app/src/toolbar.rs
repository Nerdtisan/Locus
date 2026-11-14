// SPDX-License-Identifier: AGPL-3.0-or-later
//! The customisable toolbar's pins (#299, M5 design §2.3, §7.1).
//!
//! Each ribbon group shows an ordered list of pinned action ids; the
//! group's ▾ menu always lists every action in the group. Pins are per
//! user, stored in app settings as `locus.toolbar.v1`, never in documents:
//!
//! `{ "version": 1, "groups": { "create": ["sketch.line", …], … } }`
//!
//! A group missing from the stored value gets its default pins, so a new
//! group or a user who never touched one sees the defaults. The stored value
//! is untrusted input: it is size-bounded, unknown built-in ids and
//! duplicates are dropped, and anything that doesn't parse falls back to
//! the defaults. Plugin ids (`plugin.…`) are kept even when the plugin isn't
//! loaded yet (plugins load after settings, and a disabled plugin may come
//! back); the ribbon hides a pin whose action is missing (design §7.1).

use crate::actions::Group;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const TOOLBAR_KEY: &str = "locus.toolbar.v1";
const VERSION: u32 = 1;

/// Bounds on the stored value (untrusted input).
const MAX_JSON: usize = 64 * 1024;
const MAX_PER_GROUP: usize = 64;
const MAX_ID: usize = 128;

/// The SKETCH ribbon's groups, in Fusion's order (design §2).
pub const RIBBON: [Group; 6] = [
    Group::Create,
    Group::Modify,
    Group::Constrain,
    Group::Inspect,
    Group::Insert,
    Group::Select,
];

/// Each group's default pins: its most used tools (design §2.3, §7.1).
/// The CONSTRAIN group shows every constraint symbol, as Fusion does.
pub const DEFAULT_RIBBON: &[(Group, &[&str])] = &[
    (
        Group::Create,
        &[
            "sketch.line",
            "sketch.rect_2pt",
            "sketch.circle_centre",
            "sketch.arc_3pt",
            "sketch.polygon",
            "modify.offset",
        ],
    ),
    (
        Group::Modify,
        &[
            "modify.trim",
            "modify.fillet",
            "modify.move",
            "modify.construction",
        ],
    ),
    (
        Group::Constrain,
        &[
            "constrain.dimension",
            "constrain.coincident",
            "constrain.collinear",
            "constrain.concentric",
            "constrain.midpoint",
            "constrain.fix",
            "constrain.parallel",
            "constrain.perpendicular",
            "constrain.horizontal",
            "constrain.vertical",
            "constrain.tangent",
            "constrain.equal",
            "constrain.symmetric",
        ],
    ),
    (
        Group::Inspect,
        &["view.machine_preview", "view.stroke_width"],
    ),
    (Group::Insert, &[]),
    (Group::Select, &["select.select", "select.chain"]),
];

impl Group {
    /// The stored name; never changes once shipped.
    fn key(self) -> &'static str {
        match self {
            Group::Create => "create",
            Group::Modify => "modify",
            Group::Constrain => "constrain",
            Group::Inspect => "inspect",
            Group::Insert => "insert",
            Group::Select => "select",
            Group::Output => "output",
            Group::Edit => "edit",
            Group::View => "view",
            Group::File => "file",
        }
    }

    /// The ribbon label, in Fusion's capitals.
    pub fn title(self) -> &'static str {
        match self {
            Group::Create => "CREATE",
            Group::Modify => "MODIFY",
            Group::Constrain => "CONSTRAIN",
            Group::Inspect => "INSPECT",
            Group::Insert => "INSERT",
            Group::Select => "SELECT",
            Group::Output => "OUTPUT",
            Group::Edit => "EDIT",
            Group::View => "VIEW",
            Group::File => "FILE",
        }
    }
}

fn defaults(g: Group) -> Vec<String> {
    DEFAULT_RIBBON
        .iter()
        .find(|(dg, _)| *dg == g)
        .map(|(_, ids)| ids.iter().map(|s| (*s).to_owned()).collect())
        .unwrap_or_default()
}

/// The pinned actions per ribbon group.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Toolbar {
    groups: BTreeMap<Group, Vec<String>>,
}

impl Default for Toolbar {
    fn default() -> Self {
        Self {
            groups: RIBBON.iter().map(|&g| (g, defaults(g))).collect(),
        }
    }
}

#[derive(Serialize, Deserialize)]
struct Saved {
    version: u32,
    groups: BTreeMap<String, Vec<String>>,
}

impl Toolbar {
    /// The pins of `g`, in order.
    pub fn pinned(&self, g: Group) -> &[String] {
        self.groups.get(&g).map_or(&[], Vec::as_slice)
    }

    pub fn is_pinned(&self, id: &str) -> bool {
        self.groups.values().any(|v| v.iter().any(|p| p == id))
    }

    /// Pin `id` into `group` before position `index` (clamped). An id that
    /// is already pinned moves, so this is also reorder (design §2.3).
    pub fn pin(&mut self, id: &str, group: Group, index: usize) {
        if !RIBBON.contains(&group) {
            return;
        }
        // Moving within a group: removing the old place shifts later ones.
        let index = match self.position(id) {
            Some((g, i)) if g == group && i < index => index - 1,
            _ => index,
        };
        self.unpin(id);
        let v = self.groups.entry(group).or_default();
        if v.len() >= MAX_PER_GROUP {
            return;
        }
        v.insert(index.min(v.len()), id.to_owned());
    }

    pub fn unpin(&mut self, id: &str) {
        for v in self.groups.values_mut() {
            v.retain(|p| p != id);
        }
    }

    /// Back to `group`'s default pins. A default pin that the user moved to
    /// another group comes back here.
    pub fn reset(&mut self, group: Group) {
        let d = defaults(group);
        for (g, v) in &mut self.groups {
            if *g != group {
                v.retain(|p| !d.contains(p));
            }
        }
        self.groups.insert(group, d);
    }

    fn position(&self, id: &str) -> Option<(Group, usize)> {
        self.groups
            .iter()
            .find_map(|(g, v)| v.iter().position(|p| p == id).map(|i| (*g, i)))
    }

    /// The pins from the stored value. `known` says whether a built-in
    /// action id exists; unknown ones are dropped (a renamed or removed
    /// action), as are duplicates. Never panics on stored data.
    pub fn load(json: Option<&str>, known: impl Fn(&str) -> bool) -> Self {
        let mut bar = Self::default();
        let Some(json) = json else { return bar };
        if json.len() > MAX_JSON {
            tracing::warn!("saved toolbar too large, using the default");
            return bar;
        }
        let saved = match serde_json::from_str::<Saved>(json) {
            Ok(s) if s.version == VERSION => s,
            Ok(_) => {
                tracing::warn!("saved toolbar is from another Locus version, using the default");
                return bar;
            }
            Err(_) => {
                tracing::warn!("saved toolbar unusable, using the default");
                return bar;
            }
        };
        let mut seen: Vec<String> = Vec::new();
        for g in RIBBON {
            let Some(ids) = saved.groups.get(g.key()) else {
                continue; // keep this group's defaults
            };
            let kept: Vec<String> = ids
                .iter()
                .filter(|id| id.len() <= MAX_ID)
                .filter(|id| id.starts_with("plugin.") || known(id))
                .filter(|id| {
                    let new = !seen.contains(id);
                    if new {
                        seen.push((*id).clone());
                    }
                    new
                })
                .take(MAX_PER_GROUP)
                .cloned()
                .collect();
            bar.groups.insert(g, kept);
        }
        // A default pin of an untouched group may also be pinned in a stored
        // group; the stored place wins.
        for g in RIBBON {
            if !saved.groups.contains_key(g.key())
                && let Some(v) = bar.groups.get_mut(&g)
            {
                v.retain(|p| !saved.groups.values().any(|s| s.contains(p)));
            }
        }
        bar
    }

    /// The value to store.
    pub fn save(&self) -> Option<String> {
        let saved = Saved {
            version: VERSION,
            groups: self
                .groups
                .iter()
                .map(|(g, v)| (g.key().to_owned(), v.clone()))
                .collect(),
        };
        serde_json::to_string(&saved).ok()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::actions::Registry;

    fn known(id: &str) -> bool {
        Registry::default().get(id).is_some()
    }

    #[test]
    fn every_default_pin_is_a_registered_action() {
        let reg = Registry::default();
        for (_, ids) in DEFAULT_RIBBON {
            for id in *ids {
                assert!(reg.get(id).is_some(), "{id}");
            }
        }
        // Every ribbon group has its defaults listed.
        for g in RIBBON {
            assert!(DEFAULT_RIBBON.iter().any(|(d, _)| *d == g), "{g:?}");
        }
    }

    #[test]
    fn pin_unpin_and_reorder() {
        let mut t = Toolbar::default();
        assert!(!t.is_pinned("modify.extend"));
        t.pin("modify.extend", Group::Modify, 0);
        assert_eq!(t.pinned(Group::Modify)[0], "modify.extend");
        // Moving right within a group lands before the target index.
        t.pin("modify.extend", Group::Modify, 3);
        assert_eq!(
            t.pinned(Group::Modify),
            [
                "modify.trim",
                "modify.fillet",
                "modify.extend",
                "modify.move",
                "modify.construction"
            ]
        );
        // Into another group: it leaves the first one.
        t.pin("modify.extend", Group::Create, 99);
        assert!(
            !t.pinned(Group::Modify)
                .contains(&"modify.extend".to_owned())
        );
        assert_eq!(t.pinned(Group::Create).last().unwrap(), "modify.extend");
        t.unpin("modify.extend");
        assert!(!t.is_pinned("modify.extend"));
    }

    #[test]
    fn only_ribbon_groups_take_pins() {
        let mut t = Toolbar::default();
        t.pin("edit.undo", Group::Edit, 0);
        assert!(!t.is_pinned("edit.undo"));
    }

    #[test]
    fn reset_restores_one_group_only() {
        let mut t = Toolbar::default();
        t.unpin("modify.trim");
        t.pin("modify.fillet", Group::Create, 0);
        t.unpin("sketch.line");
        t.reset(Group::Modify);
        assert_eq!(
            t.pinned(Group::Modify),
            Toolbar::default().pinned(Group::Modify)
        );
        assert!(
            !t.pinned(Group::Create)
                .contains(&"modify.fillet".to_owned())
        );
        assert!(!t.is_pinned("sketch.line"), "Create was not reset");
    }

    #[test]
    fn round_trips() {
        let mut t = Toolbar::default();
        t.pin("modify.extend", Group::Select, 1);
        t.unpin("sketch.line");
        let json = t.save().unwrap();
        assert!(json.contains("\"version\":1"));
        assert_eq!(Toolbar::load(Some(&json), known), t);
    }

    #[test]
    fn unknown_ids_and_duplicates_are_dropped_plugins_kept() {
        let json = r#"{"version":1,"groups":{
            "create":["sketch.line","sketch.gone","sketch.line","plugin.gear.insert"],
            "modify":["sketch.line"]}}"#;
        let t = Toolbar::load(Some(json), known);
        assert_eq!(
            t.pinned(Group::Create),
            ["sketch.line", "plugin.gear.insert"]
        );
        assert!(t.pinned(Group::Modify).is_empty());
        // Untouched groups keep their defaults.
        assert_eq!(
            t.pinned(Group::Constrain),
            Toolbar::default().pinned(Group::Constrain)
        );
    }

    #[test]
    fn a_stored_pin_wins_over_an_untouched_groups_default() {
        let json = r#"{"version":1,"groups":{"create":["modify.trim"]}}"#;
        let t = Toolbar::load(Some(json), known);
        assert_eq!(t.pinned(Group::Create), ["modify.trim"]);
        assert!(!t.pinned(Group::Modify).contains(&"modify.trim".to_owned()));
    }

    #[test]
    fn corrupt_or_foreign_values_fall_back() {
        let big = format!(
            "{{\"version\":1,\"groups\":{{\"create\":[\"{}\"]}}}}",
            "x".repeat(MAX_JSON)
        );
        for bad in [
            "",
            "{",
            "null",
            "[]",
            "{\"version\":1}",
            "{\"version\":2,\"groups\":{}}",
            "{\"version\":1,\"groups\":{\"create\":[1,2]}}",
            big.as_str(),
        ] {
            assert_eq!(
                Toolbar::load(Some(bad), known),
                Toolbar::default(),
                "{bad:.40}"
            );
        }
        assert_eq!(Toolbar::load(None, known), Toolbar::default());
    }

    #[test]
    fn long_ids_and_overfull_groups_are_bounded() {
        let ids: Vec<String> = (0..200)
            .map(|i| format!("\"plugin.p{i}.insert\""))
            .collect();
        let json = format!(
            "{{\"version\":1,\"groups\":{{\"insert\":[{}, \"plugin.{}\"]}}}}",
            ids.join(","),
            "y".repeat(500)
        );
        let t = Toolbar::load(Some(&json), known);
        assert_eq!(t.pinned(Group::Insert).len(), MAX_PER_GROUP);
    }
}
