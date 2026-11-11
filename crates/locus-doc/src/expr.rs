// SPDX-License-Identifier: AGPL-3.0-or-later

//! The expression evaluator for parameters and dimensions (SPEC §5).
//!
//! Arithmetic (`+ - * / ^`, parentheses, unary minus), the functions
//! `min max sqrt sin cos tan abs floor ceil round`, the constant `pi`, and
//! references to named document parameters. Angles are **degrees**. There is no
//! scripting, no I/O and no state: input is untrusted, so length and nesting
//! are bounded and every failure is a structured [`ExprError`] — nothing here
//! panics (CLAUDE.md, untrusted input).
//!
//! ```text
//! expr   := term (('+'|'-') term)*
//! term   := unary (('*'|'/') unary)*
//! unary  := '-' unary | '+' unary | power
//! power  := atom ('^' unary)?          // right-associative; -2^2 = -4
//! atom   := number | ident | ident '(' args ')' | '(' expr ')'
//! ```

use std::collections::BTreeMap;

use thiserror::Error;

use crate::document::Parameter;

/// Longest accepted expression, in bytes.
pub const MAX_EXPR_LEN: usize = 1024;
/// Deepest nesting of parentheses / unary operators / function calls, and of
/// parameter-to-parameter references.
pub const MAX_DEPTH: usize = 32;

/// A parse or evaluation failure, with the byte offset it was found at
/// (`0` for errors that are not tied to a position, e.g. a cycle).
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("{message} (at {pos})")]
pub struct ExprError {
    pub pos: usize,
    pub message: String,
}

impl ExprError {
    fn new(pos: usize, message: impl Into<String>) -> Self {
        Self {
            pos,
            message: message.into(),
        }
    }
}

/// The parsed form of an expression.
#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    Num(f64),
    Var(String, usize),
    Neg(Box<Expr>),
    Bin(Op, Box<Expr>, Box<Expr>),
    Call(Func, Vec<Expr>, usize),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    Add,
    Sub,
    Mul,
    Div,
    Pow,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Func {
    Min,
    Max,
    Sqrt,
    Sin,
    Cos,
    Tan,
    Abs,
    Floor,
    Ceil,
    Round,
}

impl Func {
    fn from_name(name: &str) -> Option<Self> {
        Some(match name {
            "min" => Self::Min,
            "max" => Self::Max,
            "sqrt" => Self::Sqrt,
            "sin" => Self::Sin,
            "cos" => Self::Cos,
            "tan" => Self::Tan,
            "abs" => Self::Abs,
            "floor" => Self::Floor,
            "ceil" => Self::Ceil,
            "round" => Self::Round,
            _ => return None,
        })
    }

    /// Whether `name` is one of the built-in functions.
    #[must_use]
    pub fn name_is_function(name: &str) -> bool {
        Self::from_name(name).is_some()
    }

    fn variadic(self) -> bool {
        matches!(self, Self::Min | Self::Max)
    }
}

// ── Lexer ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Num(f64),
    Ident(String),
    Plus,
    Minus,
    Star,
    Slash,
    Caret,
    LParen,
    RParen,
    Comma,
    End,
}

fn lex(src: &str) -> Result<Vec<(Tok, usize)>, ExprError> {
    let bytes = src.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];
        let start = i;
        match c {
            b' ' | b'\t' | b'\n' | b'\r' => {
                i += 1;
                continue;
            }
            b'+' => out.push((Tok::Plus, start)),
            b'-' => out.push((Tok::Minus, start)),
            b'*' => out.push((Tok::Star, start)),
            b'/' => out.push((Tok::Slash, start)),
            b'^' => out.push((Tok::Caret, start)),
            b'(' => out.push((Tok::LParen, start)),
            b')' => out.push((Tok::RParen, start)),
            b',' => out.push((Tok::Comma, start)),
            b'0'..=b'9' | b'.' => {
                while i < bytes.len() && (bytes[i].is_ascii_digit() || bytes[i] == b'.') {
                    i += 1;
                }
                // Optional exponent: only if digits follow, so `2e` stays an error
                // rather than silently becoming `2` then an identifier.
                if i < bytes.len() && (bytes[i] == b'e' || bytes[i] == b'E') {
                    let mut j = i + 1;
                    if j < bytes.len() && (bytes[j] == b'+' || bytes[j] == b'-') {
                        j += 1;
                    }
                    if j < bytes.len() && bytes[j].is_ascii_digit() {
                        while j < bytes.len() && bytes[j].is_ascii_digit() {
                            j += 1;
                        }
                        i = j;
                    }
                }
                let text = &src[start..i];
                let v: f64 = text
                    .parse()
                    .map_err(|_| ExprError::new(start, format!("invalid number `{text}`")))?;
                if !v.is_finite() {
                    return Err(ExprError::new(start, "number out of range"));
                }
                out.push((Tok::Num(v), start));
                continue;
            }
            b'a'..=b'z' | b'A'..=b'Z' | b'_' => {
                while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                    i += 1;
                }
                out.push((Tok::Ident(src[start..i].to_owned()), start));
                continue;
            }
            _ => {
                return Err(ExprError::new(start, "unexpected character"));
            }
        }
        i += 1;
    }
    out.push((Tok::End, src.len()));
    Ok(out)
}

// ── Parser ───────────────────────────────────────────────────────────────

struct Parser {
    toks: Vec<(Tok, usize)>,
    at: usize,
}

impl Parser {
    fn peek(&self) -> &Tok {
        // `lex` always ends with `End` and `bump` never passes it.
        &self.toks[self.at.min(self.toks.len() - 1)].0
    }
    fn pos(&self) -> usize {
        self.toks[self.at.min(self.toks.len() - 1)].1
    }
    fn bump(&mut self) -> Tok {
        let t = self.peek().clone();
        if self.at + 1 < self.toks.len() {
            self.at += 1;
        }
        t
    }

    fn enter(depth: usize, pos: usize) -> Result<usize, ExprError> {
        if depth >= MAX_DEPTH {
            Err(ExprError::new(pos, "expression nested too deeply"))
        } else {
            Ok(depth + 1)
        }
    }

    fn expr(&mut self, depth: usize) -> Result<Expr, ExprError> {
        let mut lhs = self.term(depth)?;
        loop {
            let op = match self.peek() {
                Tok::Plus => Op::Add,
                Tok::Minus => Op::Sub,
                _ => return Ok(lhs),
            };
            self.bump();
            let rhs = self.term(depth)?;
            lhs = Expr::Bin(op, Box::new(lhs), Box::new(rhs));
        }
    }

    fn term(&mut self, depth: usize) -> Result<Expr, ExprError> {
        let mut lhs = self.unary(depth)?;
        loop {
            let op = match self.peek() {
                Tok::Star => Op::Mul,
                Tok::Slash => Op::Div,
                _ => return Ok(lhs),
            };
            self.bump();
            let rhs = self.unary(depth)?;
            lhs = Expr::Bin(op, Box::new(lhs), Box::new(rhs));
        }
    }

    fn unary(&mut self, depth: usize) -> Result<Expr, ExprError> {
        match self.peek() {
            Tok::Minus => {
                let d = Self::enter(depth, self.pos())?;
                self.bump();
                Ok(Expr::Neg(Box::new(self.unary(d)?)))
            }
            Tok::Plus => {
                let d = Self::enter(depth, self.pos())?;
                self.bump();
                self.unary(d)
            }
            _ => self.power(depth),
        }
    }

    fn power(&mut self, depth: usize) -> Result<Expr, ExprError> {
        let base = self.atom(depth)?;
        if matches!(self.peek(), Tok::Caret) {
            self.bump();
            // Exponent may itself be negative or a further power.
            let exp = self.unary(depth)?;
            return Ok(Expr::Bin(Op::Pow, Box::new(base), Box::new(exp)));
        }
        Ok(base)
    }

    fn atom(&mut self, depth: usize) -> Result<Expr, ExprError> {
        let pos = self.pos();
        match self.bump() {
            Tok::Num(v) => Ok(Expr::Num(v)),
            Tok::LParen => {
                let d = Self::enter(depth, pos)?;
                let e = self.expr(d)?;
                self.expect_rparen()?;
                Ok(e)
            }
            Tok::Ident(name) => {
                if matches!(self.peek(), Tok::LParen) {
                    let func = Func::from_name(&name)
                        .ok_or_else(|| ExprError::new(pos, format!("unknown function `{name}`")))?;
                    self.bump();
                    let d = Self::enter(depth, pos)?;
                    let mut args = Vec::new();
                    if matches!(self.peek(), Tok::RParen) {
                        return Err(ExprError::new(
                            self.pos(),
                            format!("`{name}` needs an argument"),
                        ));
                    }
                    loop {
                        args.push(self.expr(d)?);
                        if matches!(self.peek(), Tok::Comma) {
                            self.bump();
                        } else {
                            break;
                        }
                    }
                    self.expect_rparen()?;
                    if !func.variadic() && args.len() != 1 {
                        return Err(ExprError::new(
                            pos,
                            format!("`{name}` takes exactly one argument"),
                        ));
                    }
                    Ok(Expr::Call(func, args, pos))
                } else {
                    Ok(Expr::Var(name, pos))
                }
            }
            Tok::End => Err(ExprError::new(pos, "unexpected end of expression")),
            _ => Err(ExprError::new(pos, "unexpected token")),
        }
    }

    fn expect_rparen(&mut self) -> Result<(), ExprError> {
        if matches!(self.peek(), Tok::RParen) {
            self.bump();
            Ok(())
        } else {
            Err(ExprError::new(self.pos(), "expected `)`"))
        }
    }
}

/// Parse an expression. Empty or oversized input is an error.
///
/// # Errors
/// [`ExprError`] with the offending byte offset.
pub fn parse(src: &str) -> Result<Expr, ExprError> {
    if src.len() > MAX_EXPR_LEN {
        return Err(ExprError::new(0, "expression too long"));
    }
    let toks = lex(src)?;
    let mut p = Parser { toks, at: 0 };
    let e = p.expr(0)?;
    if !matches!(p.peek(), Tok::End) {
        return Err(ExprError::new(p.pos(), "unexpected token"));
    }
    Ok(e)
}

impl Expr {
    /// Names of the parameters this expression refers to (sorted, unique).
    #[must_use]
    pub fn references(&self) -> Vec<String> {
        fn walk(e: &Expr, out: &mut Vec<String>) {
            match e {
                Expr::Num(_) => {}
                Expr::Var(n, _) => out.push(n.clone()),
                Expr::Neg(a) => walk(a, out),
                Expr::Bin(_, a, b) => {
                    walk(a, out);
                    walk(b, out);
                }
                Expr::Call(_, args, _) => args.iter().for_each(|a| walk(a, out)),
            }
        }
        let mut out = Vec::new();
        walk(self, &mut out);
        out.sort();
        out.dedup();
        out
    }

    /// Every parameter reference with its byte offset in the source text
    /// (for renaming).
    #[must_use]
    pub fn var_positions(&self) -> Vec<(String, usize)> {
        fn walk(e: &Expr, out: &mut Vec<(String, usize)>) {
            match e {
                Expr::Num(_) => {}
                Expr::Var(n, p) => out.push((n.clone(), *p)),
                Expr::Neg(a) => walk(a, out),
                Expr::Bin(_, a, b) => {
                    walk(a, out);
                    walk(b, out);
                }
                Expr::Call(_, args, _) => args.iter().for_each(|a| walk(a, out)),
            }
        }
        let mut out = Vec::new();
        walk(self, &mut out);
        out
    }

    fn eval(
        &self,
        vars: &mut dyn FnMut(&str, usize) -> Result<f64, ExprError>,
    ) -> Result<f64, ExprError> {
        let v = match self {
            Self::Num(v) => *v,
            Self::Var(name, pos) => vars(name, *pos)?,
            Self::Neg(a) => -a.eval(vars)?,
            Self::Bin(op, a, b) => {
                let (x, y) = (a.eval(vars)?, b.eval(vars)?);
                match op {
                    Op::Add => x + y,
                    Op::Sub => x - y,
                    Op::Mul => x * y,
                    Op::Div => {
                        if y == 0.0 {
                            return Err(ExprError::new(0, "division by zero"));
                        }
                        x / y
                    }
                    Op::Pow => x.powf(y),
                }
            }
            Self::Call(f, args, pos) => {
                let mut vals = Vec::with_capacity(args.len());
                for a in args {
                    vals.push(a.eval(vars)?);
                }
                let x = vals.first().copied().unwrap_or(0.0);
                match f {
                    Func::Min => vals.iter().copied().fold(x, f64::min),
                    Func::Max => vals.iter().copied().fold(x, f64::max),
                    Func::Sqrt => {
                        if x < 0.0 {
                            return Err(ExprError::new(*pos, "sqrt of a negative number"));
                        }
                        x.sqrt()
                    }
                    Func::Sin => x.to_radians().sin(),
                    Func::Cos => x.to_radians().cos(),
                    Func::Tan => x.to_radians().tan(),
                    Func::Abs => x.abs(),
                    Func::Floor => x.floor(),
                    Func::Ceil => x.ceil(),
                    Func::Round => x.round(),
                }
            }
        };
        if v.is_finite() {
            Ok(v)
        } else {
            Err(ExprError::new(0, "result is not a finite number"))
        }
    }
}

/// Built-in constants: `pi`, and length units in millimetres, so `0.125 * in`
/// is 3.175 (the document's length unit is the millimetre, SPEC §4).
#[must_use]
pub fn constant(name: &str) -> Option<f64> {
    Some(match name {
        "pi" => std::f64::consts::PI,
        "mm" => 1.0,
        "cm" => 10.0,
        "m" => 1000.0,
        "in" => 25.4,
        _ => return None,
    })
}

/// Evaluate `src` with `params` in scope. Parameters are resolved recursively
/// from their `expr` (not their cached `value`), with cycle detection.
///
/// # Errors
/// Parse errors, unknown names, cycles, domain errors, non-finite results.
pub fn evaluate(src: &str, params: &BTreeMap<String, Parameter>) -> Result<f64, ExprError> {
    let mut stack = Vec::new();
    eval_src(src, params, &mut stack)
}

fn eval_src(
    src: &str,
    params: &BTreeMap<String, Parameter>,
    stack: &mut Vec<String>,
) -> Result<f64, ExprError> {
    let e = parse(src)?;
    e.eval(&mut |name, pos| {
        if let Some(p) = params.get(name) {
            if stack.iter().any(|s| s == name) {
                return Err(ExprError::new(
                    pos,
                    format!("circular reference through `{name}`"),
                ));
            }
            if stack.len() >= MAX_DEPTH {
                return Err(ExprError::new(pos, "parameters nested too deeply"));
            }
            stack.push(name.to_owned());
            let r = eval_src(&p.expr, params, stack)
                .map_err(|err| ExprError::new(pos, format!("in `{name}`: {}", err.message)));
            stack.pop();
            r
        } else if let Some(v) = constant(name) {
            Ok(v)
        } else {
            Err(ExprError::new(pos, format!("unknown name `{name}`")))
        }
    })
}

/// Evaluate every parameter, in name order. Each entry is independent: one
/// bad parameter does not hide the others.
#[must_use]
pub fn evaluate_all(
    params: &BTreeMap<String, Parameter>,
) -> BTreeMap<String, Result<f64, ExprError>> {
    params
        .iter()
        .map(|(name, p)| {
            let mut stack = vec![name.clone()];
            (name.clone(), eval_src(&p.expr, params, &mut stack))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn p(pairs: &[(&str, &str)]) -> BTreeMap<String, Parameter> {
        pairs
            .iter()
            .map(|(n, e)| {
                (
                    (*n).to_owned(),
                    Parameter {
                        expr: (*e).to_owned(),
                        value: 0.0,
                    },
                )
            })
            .collect()
    }

    fn ev(s: &str) -> Result<f64, ExprError> {
        evaluate(s, &BTreeMap::new())
    }

    #[test]
    fn arithmetic_and_precedence() {
        assert_eq!(ev("1 + 2 * 3").unwrap(), 7.0);
        assert_eq!(ev("(1 + 2) * 3").unwrap(), 9.0);
        assert_eq!(ev("-2 ^ 2").unwrap(), -4.0);
        assert_eq!(ev("2 ^ 3 ^ 2").unwrap(), 512.0);
        assert_eq!(ev("2 ^ -1").unwrap(), 0.5);
        assert_eq!(ev("--3").unwrap(), 3.0);
        assert_eq!(ev("1e2 + .5").unwrap(), 100.5);
        assert_eq!(ev("10 / 4").unwrap(), 2.5);
    }

    #[test]
    fn functions_and_degrees() {
        assert!((ev("sin(90)").unwrap() - 1.0).abs() < 1e-12);
        assert!((ev("cos(60)").unwrap() - 0.5).abs() < 1e-12);
        assert!((ev("tan(45)").unwrap() - 1.0).abs() < 1e-12);
        assert_eq!(ev("min(3, 1, 2)").unwrap(), 1.0);
        assert_eq!(ev("max(3, 1, 2)").unwrap(), 3.0);
        assert_eq!(ev("sqrt(16) + abs(-2)").unwrap(), 6.0);
        assert_eq!(ev("floor(2.7) + ceil(2.1) + round(2.5)").unwrap(), 8.0);
        assert!((ev("pi").unwrap() - std::f64::consts::PI).abs() < 1e-15);
        assert!((ev("0.125 * in").unwrap() - 3.175).abs() < 1e-12);
        assert_eq!(ev("2 * cm + 5 * mm").unwrap(), 25.0);
        assert_eq!(ev("1.5 * m").unwrap(), 1500.0);
    }

    #[test]
    fn parameters_resolve_transitively() {
        let ps = p(&[("material", "3"), ("slot", "material * 2 + 0.2")]);
        assert!((evaluate("slot + 1", &ps).unwrap() - 7.2).abs() < 1e-12);
        let all = evaluate_all(&ps);
        assert_eq!(all["material"], Ok(3.0));
    }

    #[test]
    fn cycles_are_detected() {
        let ps = p(&[("a", "b + 1"), ("b", "a + 1"), ("c", "c")]);
        assert!(evaluate("a", &ps).unwrap_err().message.contains("circular"));
        let all = evaluate_all(&ps);
        assert!(all["a"].is_err() && all["b"].is_err() && all["c"].is_err());
    }

    #[test]
    fn errors_carry_positions() {
        assert_eq!(ev("1 + $").unwrap_err().pos, 4);
        assert_eq!(ev("2 +").unwrap_err().pos, 3);
        assert_eq!(ev("(1").unwrap_err().pos, 2);
        assert_eq!(ev("foo(1)").unwrap_err().pos, 0);
        assert!(ev("sqrt(-1)").is_err());
        assert!(ev("1 / 0").is_err());
        assert!(ev("").is_err());
        assert!(ev("min()").is_err());
        assert!(ev("sin(1, 2)").is_err());
        assert!(ev("nope").is_err());
        assert!(ev("10 ^ 1000").is_err());
        assert!(ev("1e999").is_err());
        assert!(ev("1 2").is_err());
    }

    #[test]
    fn bounded_input() {
        assert!(ev(&"(".repeat(10_000)).is_err());
        assert!(ev(&"-".repeat(500)).is_err());
        assert!(ev(&format!("{}1{}", "(".repeat(100), ")".repeat(100))).is_err());
        assert!(ev(&"1+".repeat(2_000)).is_err());
        let ok = format!("{}1{}", "(".repeat(20), ")".repeat(20));
        assert_eq!(ev(&ok).unwrap(), 1.0);
        // A long left-associative chain within the length cap.
        assert_eq!(ev(&format!("1{}", "+1".repeat(400))).unwrap(), 401.0);
    }

    #[test]
    fn deep_parameter_chain_is_bounded() {
        let mut pairs: Vec<(String, String)> = vec![("p0".into(), "1".into())];
        for i in 1..200 {
            pairs.push((format!("p{i}"), format!("p{} + 1", i - 1)));
        }
        let ps: BTreeMap<String, Parameter> = pairs
            .into_iter()
            .map(|(n, e)| {
                (
                    n,
                    Parameter {
                        expr: e,
                        value: 0.0,
                    },
                )
            })
            .collect();
        assert!(evaluate("p199", &ps).is_err());
        assert_eq!(evaluate("p10", &ps).unwrap(), 11.0);
    }

    proptest! {
        #[test]
        fn never_panics_on_any_string(s in ".{0,300}") {
            let _ = evaluate(&s, &BTreeMap::new());
        }

        #[test]
        fn never_panics_on_expression_alphabet(
            s in proptest::collection::vec(
                prop::sample::select(vec![
                    "1", "2.5", "x", "pi", "+", "-", "*", "/", "^", "(", ")", ",",
                    "min", "sqrt", "sin", "floor", " ", "1e9", "e", ".",
                ]),
                0..80,
            )
        ) {
            let src: String = s.concat();
            let ps = p(&[("x", "x + 1")]);
            let _ = evaluate(&src, &ps);
            let _ = evaluate_all(&ps);
        }
    }
}
