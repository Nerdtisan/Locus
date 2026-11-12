// SPDX-License-Identifier: AGPL-3.0-or-later
//! Machine-profile helpers for the export panel (SPEC §7, #236, #238): the
//! list of choices, and importing a single profile `.json`. Imported files
//! are untrusted: the model validates and bounds them, and a profile may not
//! reuse a built-in preset's id (so a preset can never be shadowed).

use crate::settings::Settings;
use locus_export::MachineProfile;
use locus_export::profile::{self, is_builtin};

/// Most user profiles kept in settings.
pub(super) const MAX_USER_PROFILES: usize = 64;

/// Presets first, then the user's own, in settings order.
pub(super) fn all(settings: &Settings) -> Vec<MachineProfile> {
    let mut v = profile::presets();
    v.extend(settings.profiles.iter().cloned());
    v
}

/// Add (or replace, by id) an imported profile. Returns its id.
///
/// # Errors
/// A message safe to show: invalid file, preset id clash or too many profiles.
pub(super) fn import(text: &str, settings: &mut Settings) -> Result<String, String> {
    let p = MachineProfile::from_json(text)
        .map_err(|e| format!("Could not import the profile: {e}."))?;
    if is_builtin(&p.id) {
        return Err(
            "That profile uses the id of a built-in preset; change its id first.".to_owned(),
        );
    }
    let id = p.id.clone();
    match settings.profiles.iter().position(|q| q.id == id) {
        Some(i) => settings.profiles[i] = p,
        None if settings.profiles.len() >= MAX_USER_PROFILES => {
            return Err("Too many user profiles.".to_owned());
        }
        None => settings.profiles.push(p),
    }
    Ok(id)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn import_adds_replaces_and_refuses_presets() {
        let mut s = Settings::default();
        let mut p = profile::find("generic-svg", &[]).unwrap();
        // A preset id is refused.
        assert!(import(&p.to_json().unwrap(), &mut s).is_err());
        p.id = "mine".into();
        assert_eq!(import(&p.to_json().unwrap(), &mut s).unwrap(), "mine");
        p.name = "Renamed".into();
        import(&p.to_json().unwrap(), &mut s).unwrap();
        assert_eq!(s.profiles.len(), 1);
        assert_eq!(s.profiles[0].name, "Renamed");
        assert_eq!(all(&s).last().unwrap().id, "mine");
        // Garbage never panics.
        assert!(import("{\"id\": 1", &mut s).is_err());
    }
}
