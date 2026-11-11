// SPDX-License-Identifier: AGPL-3.0-or-later

//! Tool errors as structured data, so an AI client can self-correct (SPEC
//! §16.3): every error carries a stable `kind` and the IDs involved.
//!
//! Messages never contain document contents beyond IDs and names the client
//! itself sent, and never file paths or tokens (SPEC §16.4).

use locus_doc::DocError;
use serde_json::{Value, json};

/// Why a tool call failed. Rendered to JSON by [`ToolError::to_json`].
#[derive(Debug, Clone, PartialEq)]
pub enum ToolError {
    /// The arguments don't match the tool's schema, or reference something
    /// the call itself never defined.
    BadArguments(String),
    /// The scope this tool needs was not granted (SPEC §16.4).
    ScopeDenied { tool: String, scope: &'static str },
    /// The user declined the approval prompt.
    Declined,
    /// Too many write calls in a short time (SPEC §16.4 rate limiting).
    RateLimited { retry_after_ms: u64 },
    /// The payload or a count in it is over a cap (SPEC §16.4).
    TooLarge {
        what: &'static str,
        limit: usize,
        got: usize,
    },
    /// The document rejected the change; nothing was modified.
    Document(DocError),
    /// A plugin call failed or no plugins are available.
    Plugin(String),
    /// The export failed or was blocked by validation.
    Export(String),
    /// Something the host could not do (e.g. the app closed the session).
    Unavailable(String),
}

impl From<DocError> for ToolError {
    fn from(e: DocError) -> Self {
        ToolError::Document(e)
    }
}

impl std::fmt::Display for ToolError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ToolError::BadArguments(m) => write!(f, "bad arguments: {m}"),
            ToolError::ScopeDenied { tool, scope } => write!(
                f,
                "`{tool}` needs the {scope} scope, which the user has not granted"
            ),
            ToolError::Declined => f.write_str("the user declined this action"),
            ToolError::RateLimited { retry_after_ms } => {
                write!(f, "too many write calls; retry in {retry_after_ms} ms")
            }
            ToolError::TooLarge { what, limit, got } => {
                write!(f, "{what} is {got}; the limit is {limit}")
            }
            ToolError::Document(e) => write!(f, "{e}"),
            ToolError::Plugin(m) => write!(f, "plugin: {m}"),
            ToolError::Export(m) => write!(f, "export: {m}"),
            ToolError::Unavailable(m) => write!(f, "unavailable: {m}"),
        }
    }
}

impl ToolError {
    /// `{"error": {"kind": …, "message": …, …details}}`.
    #[must_use]
    pub fn to_json(&self) -> Value {
        let message = self.to_string();
        let mut detail = match self {
            ToolError::BadArguments(_) => json!({ "kind": "bad_arguments" }),
            ToolError::ScopeDenied { scope, .. } => {
                json!({ "kind": "scope_denied", "scope": scope })
            }
            ToolError::Declined => json!({ "kind": "declined" }),
            ToolError::RateLimited { retry_after_ms } => {
                json!({ "kind": "rate_limited", "retry_after_ms": retry_after_ms })
            }
            ToolError::TooLarge { what, limit, got } => {
                json!({ "kind": "too_large", "what": what, "limit": limit, "got": got })
            }
            ToolError::Document(e) => doc_error(e),
            ToolError::Plugin(_) => json!({ "kind": "plugin" }),
            ToolError::Export(_) => json!({ "kind": "export" }),
            ToolError::Unavailable(_) => json!({ "kind": "unavailable" }),
        };
        if let Some(o) = detail.as_object_mut() {
            o.insert("message".into(), Value::String(message));
        }
        json!({ "error": detail })
    }
}

/// A [`DocError`] as `kind` plus the IDs it names.
fn doc_error(e: &DocError) -> Value {
    use DocError as E;
    match e {
        E::UnknownEntity(id) => json!({ "kind": "unknown_entity", "entity": id }),
        E::UnknownConstraint(id) => json!({ "kind": "unknown_constraint", "constraint": id }),
        E::UnknownLayer(id) => json!({ "kind": "unknown_layer", "layer": id }),
        E::UnknownBoolean(id) => json!({ "kind": "unknown_boolean", "boolean": id }),
        E::UnknownRecipe(id) => json!({ "kind": "unknown_recipe", "recipe": id }),
        E::UnknownParameter(n) => json!({ "kind": "unknown_parameter", "parameter": n }),
        E::LayerInUse { layer, entities } => {
            json!({ "kind": "layer_in_use", "layer": layer, "entities": entities })
        }
        E::LayerLocked(id) => json!({ "kind": "layer_locked", "layer": id }),
        E::ParameterInUse { name, users } => {
            json!({ "kind": "parameter_in_use", "parameter": name, "users": users })
        }
        E::Expression(_) => json!({ "kind": "expression" }),
        E::Driven { entity, recipe } => {
            json!({ "kind": "driven", "entity": entity, "recipe": recipe })
        }
        E::RecipeInUse { entity, recipes } => {
            json!({ "kind": "recipe_in_use", "entity": entity, "recipes": recipes })
        }
        E::ConstraintOwned { constraint, recipe } => {
            json!({ "kind": "constraint_owned", "constraint": constraint, "recipe": recipe })
        }
        E::InstanceLimit { requested, limit } => {
            json!({ "kind": "instance_limit", "requested": requested, "limit": limit })
        }
        E::WrongKind {
            entity, expected, ..
        } => json!({ "kind": "wrong_kind", "entity": entity, "expected": expected }),
        E::SameEntity { entity, .. } => json!({ "kind": "same_entity", "entity": entity }),
        E::BadValue { .. } => json!({ "kind": "bad_value" }),
        E::BadText(_) => json!({ "kind": "bad_text" }),
        E::FontUnavailable(_) => json!({ "kind": "font_unavailable" }),
        E::EntityInUse {
            id,
            entities,
            constraints,
        } => json!({
            "kind": "entity_in_use",
            "entity": id,
            "used_by_entities": entities,
            "used_by_constraints": constraints,
        }),
        E::NotADimension(id) => json!({ "kind": "not_a_dimension", "constraint": id }),
        E::IdSpaceExhausted => json!({ "kind": "id_space_exhausted" }),
        E::NothingToDo(_) => json!({ "kind": "nothing_to_do" }),
        E::Invalid(_) => json!({ "kind": "invalid" }),
        E::Conflict {
            constraints,
            entities,
        } => json!({
            "kind": "over_constrained_conflict",
            "constraints": constraints,
            "entities": entities,
            "hint": "the new constraint contradicts these; remove one, or add it as driven (reference)",
        }),
        E::Redundant {
            constraints,
            entities,
        } => json!({
            "kind": "over_constrained_redundant",
            "constraints": constraints,
            "entities": entities,
            "hint": "already implied by these; add it as driven (reference) instead",
        }),
        E::Unsolvable { unsatisfied } => {
            json!({ "kind": "unsolvable", "constraints": unsatisfied })
        }
        E::Solver(_) => json!({ "kind": "solver" }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use locus_doc::{ConstraintId, EntityId};

    #[test]
    fn conflict_names_ids() {
        let e = ToolError::Document(DocError::Conflict {
            constraints: vec![ConstraintId::new(7)],
            entities: vec![EntityId::new(3), EntityId::new(4)],
        });
        let j = e.to_json();
        assert_eq!(j["error"]["kind"], "over_constrained_conflict");
        assert_eq!(j["error"]["constraints"], json!([7]));
        assert_eq!(j["error"]["entities"], json!([3, 4]));
        assert!(j["error"]["message"].is_string());
    }
}
