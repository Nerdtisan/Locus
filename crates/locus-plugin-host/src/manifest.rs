// SPDX-License-Identifier: AGPL-3.0-or-later

//! `plugin.toml`, the plugin manifest (§15.4, #182).
//!
//! ```toml
//! id = "com.example.finger-box"   # reverse-DNS
//! name = "Finger-joint box"
//! version = "1.0.0"
//! author = "Jane Doe"
//! licence = "MIT OR Apache-2.0"
//! kinds = ["generator"]
//! api = "1.0.0"                   # locus:plugin version it was built against
//! capabilities = []               # v1 grants none (§15.4)
//! sha256 = "…"                    # optional, of plugin.wasm
//! description = "…"               # optional
//! ```
//!
//! Parsing never panics; every problem is a [`ManifestError`] that names the
//! field. Unknown fields are refused so typos don't pass silently.

use serde::Deserialize;

use crate::api::SUPPORTED_API_MAJORS;
use crate::types::Kind;
use crate::validate::{MAX_TEXT, is_reverse_dns};

/// Largest `plugin.toml` read.
pub const MAX_MANIFEST_BYTES: usize = 64 * 1024;

/// A parsed, checked manifest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Manifest {
    pub id: String,
    pub name: String,
    pub version: String,
    pub author: String,
    pub licence: String,
    pub kinds: Vec<Kind>,
    /// The `locus:plugin` version, e.g. `1.0.0`.
    pub api: String,
    /// Lower-case hex SHA-256 of `plugin.wasm`, when the author gave one.
    pub sha256: Option<String>,
    pub description: String,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ManifestError {
    #[error("plugin.toml is larger than {MAX_MANIFEST_BYTES} bytes")]
    TooLarge,
    #[error("plugin.toml is not valid TOML for a manifest: {0}")]
    Syntax(String),
    #[error("plugin.toml: {field} {problem}")]
    Field {
        field: &'static str,
        problem: String,
    },
    #[error("plugin.toml: plugin API {found} isn't supported; this Locus supports {supported}")]
    UnsupportedApi { found: String, supported: String },
    /// §15.4: v1 has nothing to grant, so any request is refused at install.
    #[error("plugin.toml asks for the capability `{0}`, which Locus can't grant yet")]
    Capability(String),
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Raw {
    id: String,
    name: String,
    version: String,
    author: String,
    licence: String,
    kinds: Vec<String>,
    api: String,
    #[serde(default)]
    capabilities: Vec<String>,
    sha256: Option<String>,
    #[serde(default)]
    description: String,
}

fn field(field: &'static str, problem: impl Into<String>) -> ManifestError {
    ManifestError::Field {
        field,
        problem: problem.into(),
    }
}

fn text(name: &'static str, s: &str, max: usize) -> Result<(), ManifestError> {
    if s.trim().is_empty() {
        return Err(field(name, "is empty"));
    }
    if s.len() > max {
        return Err(field(name, format!("is longer than {max} bytes")));
    }
    if s.chars().any(char::is_control) {
        return Err(field(name, "contains control characters"));
    }
    Ok(())
}

/// A version string: dot-separated, ASCII letters, digits, `+` and `-`.
fn is_version(v: &str) -> bool {
    !v.is_empty()
        && v.len() <= 64
        && v.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"+-.".contains(&b))
        && v.split('.').all(|p| !p.is_empty())
}

/// The major of a `M.m.p` API version.
pub fn api_major(v: &str) -> Option<u64> {
    let mut parts = v.split('.');
    let major = parts.next()?.parse().ok()?;
    let rest: Vec<&str> = parts.collect();
    (rest.len() == 2 && rest.iter().all(|p| p.parse::<u64>().is_ok())).then_some(major)
}

impl Manifest {
    pub fn parse(src: &str) -> Result<Manifest, ManifestError> {
        if src.len() > MAX_MANIFEST_BYTES {
            return Err(ManifestError::TooLarge);
        }
        let raw: Raw =
            toml::from_str(src).map_err(|e| ManifestError::Syntax(e.message().into()))?;

        if !is_reverse_dns(&raw.id) {
            return Err(field(
                "id",
                "must be a reverse-DNS name like com.example.my-plugin",
            ));
        }
        text("name", &raw.name, 128)?;
        if !is_version(&raw.version) {
            return Err(field("version", "must look like 1.2.3"));
        }
        text("author", &raw.author, 256)?;
        text("licence", &raw.licence, 256)?;
        if raw.description.len() > MAX_TEXT {
            return Err(field(
                "description",
                format!("is longer than {MAX_TEXT} bytes"),
            ));
        }

        let mut kinds = Vec::new();
        for k in &raw.kinds {
            let kind = Kind::from_name(k).ok_or_else(|| field("kinds", "names an unknown kind"))?;
            if kinds.contains(&kind) {
                return Err(field("kinds", "lists a kind twice"));
            }
            kinds.push(kind);
        }
        if kinds.is_empty() {
            return Err(field("kinds", "is empty"));
        }
        kinds.sort();

        let supported = supported_list();
        match api_major(&raw.api) {
            None => return Err(field("api", "must look like 1.0.0")),
            Some(m) if !SUPPORTED_API_MAJORS.contains(&m) => {
                return Err(ManifestError::UnsupportedApi {
                    found: raw.api,
                    supported,
                });
            }
            Some(_) => {}
        }

        if let Some(c) = raw.capabilities.first() {
            let c: String = c.chars().take(64).filter(|c| !c.is_control()).collect();
            return Err(ManifestError::Capability(c));
        }

        let sha256 = match raw.sha256 {
            None => None,
            Some(h) => {
                let h = h.to_ascii_lowercase();
                if h.len() != 64 || !h.bytes().all(|b| b.is_ascii_hexdigit()) {
                    return Err(field("sha256", "must be 64 hex digits"));
                }
                Some(h)
            }
        };

        Ok(Manifest {
            id: raw.id,
            name: raw.name,
            version: raw.version,
            author: raw.author,
            licence: raw.licence,
            kinds,
            api: raw.api,
            sha256,
            description: raw.description,
        })
    }
}

pub(crate) fn supported_list() -> String {
    SUPPORTED_API_MAJORS
        .iter()
        .map(|m| format!("{m}.x"))
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    const GOOD: &str = r#"
id = "com.example.finger-box"
name = "Finger-joint box"
version = "1.0.0"
author = "Jane Doe"
licence = "MIT OR Apache-2.0"
kinds = ["generator"]
api = "1.0.0"
"#;

    #[test]
    fn parses_a_good_manifest() {
        let m = Manifest::parse(GOOD).unwrap();
        assert_eq!(m.id, "com.example.finger-box");
        assert_eq!(m.kinds, vec![Kind::Generator]);
        assert_eq!(m.sha256, None);
    }

    fn with(line: &str) -> Result<Manifest, ManifestError> {
        Manifest::parse(&format!("{GOOD}{line}\n"))
    }

    fn replace(from: &str, to: &str) -> Result<Manifest, ManifestError> {
        Manifest::parse(&GOOD.replace(from, to))
    }

    #[test]
    fn rejects_bad_fields() {
        assert!(matches!(
            replace("com.example.finger-box", "finger box"),
            Err(ManifestError::Field { field: "id", .. })
        ));
        assert!(matches!(
            replace("[\"generator\"]", "[\"teleporter\"]"),
            Err(ManifestError::Field { field: "kinds", .. })
        ));
        assert!(matches!(
            replace("[\"generator\"]", "[]"),
            Err(ManifestError::Field { field: "kinds", .. })
        ));
        assert!(matches!(
            replace("api = \"1.0.0\"", "api = \"one\""),
            Err(ManifestError::Field { field: "api", .. })
        ));
        assert!(matches!(
            with("sha256 = \"abc\""),
            Err(ManifestError::Field {
                field: "sha256",
                ..
            })
        ));
        assert!(matches!(with("colour = 3"), Err(ManifestError::Syntax(_))));
        assert!(matches!(
            Manifest::parse("not = [toml"),
            Err(ManifestError::Syntax(_))
        ));
    }

    #[test]
    fn rejects_wrong_api_major() {
        assert!(matches!(
            replace("api = \"1.0.0\"", "api = \"2.0.0\""),
            Err(ManifestError::UnsupportedApi { .. })
        ));
    }

    #[test]
    fn refuses_every_capability_in_v1() {
        assert_eq!(
            with("capabilities = [\"filesystem\"]"),
            Err(ManifestError::Capability("filesystem".into()))
        );
        assert!(with("capabilities = []").is_ok());
    }

    #[test]
    fn huge_manifest_is_refused() {
        let big = format!("{GOOD}description = \"{}\"", "x".repeat(MAX_MANIFEST_BYTES));
        assert_eq!(Manifest::parse(&big), Err(ManifestError::TooLarge));
    }
}
