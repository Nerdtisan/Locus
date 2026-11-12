// SPDX-License-Identifier: AGPL-3.0-or-later
//! User settings (#65): display units, theme, auto-constraint toggle.
//!
//! Persisted through eframe's storage, which is the platform config dir on
//! desktop and `localStorage` on the web. A missing, truncated or hand-edited
//! value must never crash the app, so every load falls back to defaults.
//! Settings hold no document content (CLAUDE.md, logging rules).

use serde::{Deserialize, Serialize};

pub const SETTINGS_KEY: &str = "locus.settings.v1";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Units {
    #[default]
    Mm,
    Inch,
}

impl Units {
    pub const ALL: [Units; 2] = [Units::Mm, Units::Inch];

    pub fn label(self) -> &'static str {
        match self {
            Units::Mm => "mm",
            Units::Inch => "in",
        }
    }

    /// Format a length held in millimetres (SPEC §4) for display.
    pub fn format_len(self, mm: f64) -> String {
        match self {
            Units::Mm => format!("{mm:.3} mm"),
            Units::Inch => format!("{:.4} in", mm / 25.4),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Theme {
    #[default]
    System,
    Light,
    Dark,
}

impl Theme {
    pub const ALL: [Theme; 3] = [Theme::System, Theme::Light, Theme::Dark];

    pub fn label(self) -> &'static str {
        match self {
            Theme::System => "System",
            Theme::Light => "Light",
            Theme::Dark => "Dark",
        }
    }

    pub fn preference(self) -> egui::ThemePreference {
        match self {
            Theme::System => egui::ThemePreference::System,
            Theme::Light => egui::ThemePreference::Light,
            Theme::Dark => egui::ThemePreference::Dark,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub units: Units,
    pub theme: Theme,
    /// Infer coincident / horizontal / vertical constraints while drawing (§5).
    pub auto_constrain: bool,
    /// User machine profiles (SPEC §7, #236); presets are compiled in.
    pub profiles: Vec<locus_export::MachineProfile>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            units: Units::default(),
            theme: Theme::default(),
            auto_constrain: true,
            profiles: Vec::new(),
        }
    }
}

impl Settings {
    /// Parse stored settings; anything unreadable yields the defaults.
    pub fn from_json(s: &str) -> Self {
        serde_json::from_str(s).unwrap_or_else(|_| {
            tracing::warn!("settings unreadable, using defaults");
            Self::default()
        })
    }

    pub fn to_json(&self) -> Option<String> {
        serde_json::to_string(self).ok()
    }

    pub fn load(storage: Option<&dyn eframe::Storage>) -> Self {
        storage
            .and_then(|s| s.get_string(SETTINGS_KEY))
            .map(|s| Self::from_json(&s))
            .unwrap_or_default()
    }

    pub fn save(&self, storage: &mut dyn eframe::Storage) {
        if let Some(json) = self.to_json() {
            storage.set_string(SETTINGS_KEY, json);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips() {
        let s = Settings {
            units: Units::Inch,
            theme: Theme::Dark,
            auto_constrain: false,
            profiles: locus_export::profile::presets()
                .into_iter()
                .take(1)
                .collect(),
        };
        assert_eq!(Settings::from_json(&s.to_json().unwrap_or_default()), s);
    }

    #[test]
    fn corrupt_input_falls_back_to_defaults() {
        for bad in [
            "",
            "{",
            "null",
            "[1,2]",
            "{\"units\":\"Furlong\"}",
            "\u{0}\u{1}",
        ] {
            assert_eq!(Settings::from_json(bad), Settings::default(), "{bad:?}");
        }
    }

    #[test]
    fn missing_fields_take_defaults() {
        let s = Settings::from_json("{\"theme\":\"Light\"}");
        assert_eq!(s.theme, Theme::Light);
        assert_eq!(s.units, Units::Mm);
        assert!(s.auto_constrain);
    }

    #[test]
    fn inch_formatting() {
        assert_eq!(Units::Inch.format_len(25.4), "1.0000 in");
        assert_eq!(Units::Mm.format_len(1.5), "1.500 mm");
    }
}
