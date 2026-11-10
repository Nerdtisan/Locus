// SPDX-License-Identifier: AGPL-3.0-or-later

//! Named parameters and dimension expressions (SPEC §5, #171, #172).
//!
//! Parameters hold an expression and its last value; dimensions may hold an
//! expression too (`Document::dimension_expr`). After any change every value
//! is re-evaluated from the expressions, inside the same undo step.

use crate::constraint::Constraint;
use crate::document::Document;
use crate::error::DocError;
use crate::expr::{self, Func};
use crate::ids::ConstraintId;
use std::collections::BTreeMap;

/// Longest parameter name.
pub const MAX_NAME: usize = 64;
/// Most parameters a document may hold.
pub const MAX_PARAMETERS: usize = 1_000;

/// A usable parameter name: an identifier that is not a function or constant.
///
/// # Errors
/// [`DocError::Invalid`] saying what is wrong.
pub fn check_name(name: &str) -> Result<(), DocError> {
    let mut chars = name.chars();
    let ok = chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
        && name.len() <= MAX_NAME;
    if !ok {
        return Err(DocError::Invalid(format!(
            "`{name}` is not a valid parameter name (letters, digits and _, not starting with a digit, at most {MAX_NAME})"
        )));
    }
    if name == "pi" || Func::name_is_function(name) {
        return Err(DocError::Invalid(format!("`{name}` is a built-in name")));
    }
    Ok(())
}

/// `src` with every reference to `from` replaced by `to`; `src` unchanged if
/// it doesn't parse.
#[must_use]
pub fn rename_in(src: &str, from: &str, to: &str) -> String {
    let Ok(e) = expr::parse(src) else {
        return src.to_owned();
    };
    let mut out = src.to_owned();
    let mut hits: Vec<usize> = e
        .var_positions()
        .into_iter()
        .filter(|(n, _)| n == from)
        .map(|(_, p)| p)
        .collect();
    hits.sort_unstable_by(|a, b| b.cmp(a));
    for p in hits {
        out.replace_range(p..p + from.len(), to);
    }
    out
}

/// What refers to parameter `name`: other parameters and dimensions.
#[must_use]
pub fn users_of(doc: &Document, name: &str) -> Vec<String> {
    let uses = |src: &str| expr::parse(src).is_ok_and(|e| e.references().iter().any(|r| r == name));
    let mut out: Vec<String> = doc
        .parameters()
        .iter()
        .filter(|(n, p)| n.as_str() != name && uses(&p.expr))
        .map(|(n, _)| format!("parameter `{n}`"))
        .collect();
    out.extend(
        doc.dim_exprs
            .iter()
            .filter(|(_, s)| uses(s))
            .map(|(id, _)| format!("dimension {id}")),
    );
    out
}

/// A dimension's value in its stored unit: angle expressions are in degrees
/// (SPEC §5), stored as radians.
#[must_use]
pub fn dimension_value(c: &Constraint, evaluated: f64) -> f64 {
    if matches!(c, Constraint::Angle { .. }) {
        evaluated.to_radians()
    } else {
        evaluated
    }
}

/// New values for every parameter and every expression-backed dimension.
pub(crate) struct Revalued {
    pub parameters: Vec<(String, f64)>,
    pub dimensions: Vec<(ConstraintId, f64)>,
}

/// Evaluate everything in `doc`. Fails (so the command is refused) if any
/// expression no longer evaluates.
pub(crate) fn revalue(doc: &Document) -> Result<Revalued, DocError> {
    let all = expr::evaluate_all(doc.parameters());
    let mut parameters = Vec::new();
    let mut values: BTreeMap<&str, f64> = BTreeMap::new();
    for (name, r) in &all {
        let v = r
            .clone()
            .map_err(|e| DocError::Invalid(format!("parameter `{name}`: {}", e.message)))?;
        values.insert(name, v);
        parameters.push((name.clone(), v));
    }
    let mut dimensions = Vec::new();
    for (id, src) in &doc.dim_exprs {
        let c = doc
            .constraint(*id)
            .ok_or(DocError::UnknownConstraint(*id))?;
        let v = expr::evaluate(src, doc.parameters())?;
        dimensions.push((*id, dimension_value(c, v)));
    }
    Ok(Revalued {
        parameters,
        dimensions,
    })
}
