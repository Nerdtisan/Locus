// SPDX-License-Identifier: AGPL-3.0-or-later

//! The canonical ABI of `locus:plugin@1.x`, written out for its fixed set of
//! types (ADR 0016, #253).
//!
//! On desktop wasmtime's component runtime lifts and lowers values. The web
//! host runs the bare core module, so this does it instead: parameters are
//! written into the plugin's memory through its `cabi_realloc`, results are
//! read back from the pointer it returns. Layouts follow the Component Model
//! canonical ABI (sizes, alignments, discriminants); `tests/core_parity.rs`
//! checks them against wasmtime on every fixture and example plugin.
//!
//! Plugin memory is untrusted: every read is bounds-checked, list pointers
//! must be aligned, strings must be UTF-8 and discriminants in range, exactly
//! as the canonical ABI requires. A violation is [`CallError::Malformed`];
//! nothing panics.

use super::module::export_name;
use crate::error::CallError;
use crate::types::{
    ChoiceSpec, Compensation, Constraint, Entity, ExportModel, Finding, FormatInfo, Generated,
    IntegerSpec, NumberSpec, Operation, Param, ParamKind, ParamSpec, ParamValue, Path,
    PluginFailure, PluginInfo, Segment, Severity, Shape, TextSpec, Vec2,
};

/// A core value passed to or returned from a function.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Val {
    I32(i32),
    I64(i64),
    F32(f32),
    F64(f64),
}

/// One fresh instance of a plugin's core module. Implemented over wasmtime
/// (desktop tests) and over the browser's `WebAssembly` (web worker).
pub trait Instance {
    /// Calls an exported function. Traps and limits map to [`CallError`].
    fn call(&mut self, name: &str, args: &[Val]) -> Result<Vec<Val>, CallError>;
    /// The memory's current size in bytes.
    fn memory_len(&self) -> usize;
    /// Reads `len` bytes at `ptr`; the caller has checked the range.
    fn read(&self, ptr: usize, len: usize) -> Vec<u8>;
    /// Writes at `ptr`; the caller has checked the range.
    fn write(&mut self, ptr: usize, bytes: &[u8]);
}

fn malformed(m: impl Into<String>) -> CallError {
    CallError::Malformed(m.into())
}

/// Most bytes a single list or string may span (the export cap, §15.4).
const MAX_SPAN: usize = crate::validate::MAX_EXPORT_BYTES;

// ── Reading plugin memory ───────────────────────────────────────────────────

/// A bounds-checked view of the instance memory at call return.
pub(crate) struct Mem<'a, I: Instance + ?Sized> {
    inst: &'a I,
    len: usize,
}

impl<'a, I: Instance + ?Sized> Mem<'a, I> {
    fn new(inst: &'a I) -> Self {
        Mem {
            len: inst.memory_len(),
            inst,
        }
    }

    fn bytes(&self, ptr: u32, len: usize) -> Result<Vec<u8>, CallError> {
        let start = ptr as usize;
        let end = start
            .checked_add(len)
            .filter(|e| *e <= self.len)
            .ok_or_else(|| malformed("a pointer is outside the plugin's memory"))?;
        Ok(self.inst.read(start, end - start))
    }

    fn u8(&self, p: u32) -> Result<u8, CallError> {
        Ok(self.bytes(p, 1)?[0])
    }

    fn u32(&self, p: u32) -> Result<u32, CallError> {
        let b = self.bytes(p, 4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    fn u64(&self, p: u32) -> Result<u64, CallError> {
        let b = self.bytes(p, 8)?;
        let mut a = [0u8; 8];
        a.copy_from_slice(&b);
        Ok(u64::from_le_bytes(a))
    }

    fn f64(&self, p: u32) -> Result<f64, CallError> {
        Ok(f64::from_bits(self.u64(p)?))
    }

    fn bool(&self, p: u32) -> Result<bool, CallError> {
        Ok(self.u8(p)? != 0)
    }

    /// A discriminant below `cases`.
    fn disc(&self, p: u32, cases: u8) -> Result<u8, CallError> {
        let d = self.u8(p)?;
        if d >= cases {
            return Err(malformed("an invalid variant case"));
        }
        Ok(d)
    }

    fn string(&self, p: u32) -> Result<String, CallError> {
        let (ptr, len) = (self.u32(p)?, self.u32(p + 4)? as usize);
        if len > MAX_SPAN {
            return Err(malformed("a string is too long"));
        }
        String::from_utf8(self.bytes(ptr, len)?).map_err(|_| malformed("a string isn't UTF-8"))
    }

    /// A `list<T>` of `size`-byte, `align`-aligned elements, each read by `f`.
    fn list<T>(
        &self,
        p: u32,
        size: u32,
        align: u32,
        max: usize,
        f: impl Fn(&Self, u32) -> Result<T, CallError>,
    ) -> Result<Vec<T>, CallError> {
        let (ptr, len) = (self.u32(p)?, self.u32(p + 4)? as usize);
        if len > max {
            return Err(malformed(format!("a list has more than {max} items")));
        }
        if ptr % align != 0 {
            return Err(malformed("a list pointer isn't aligned"));
        }
        let span = len
            .checked_mul(size as usize)
            .filter(|s| *s <= MAX_SPAN)
            .ok_or_else(|| malformed("a list is too large"))?;
        self.bytes(ptr, span)?; // bounds of the whole list
        (0..len as u32).map(|i| f(self, ptr + i * size)).collect()
    }

    /// A `list<u8>`, read in one go.
    fn byte_list(&self, p: u32) -> Result<Vec<u8>, CallError> {
        let (ptr, len) = (self.u32(p)?, self.u32(p + 4)? as usize);
        if len > crate::validate::MAX_EXPORT_BYTES {
            return Err(malformed("a byte list is too large"));
        }
        self.bytes(ptr, len)
    }

    fn vec2(&self, p: u32) -> Result<Vec2, CallError> {
        Ok(Vec2 {
            x: self.f64(p)?,
            y: self.f64(p + 8)?,
        })
    }

    fn option_f64(&self, p: u32) -> Result<Option<f64>, CallError> {
        Ok(match self.disc(p, 2)? {
            0 => None,
            _ => Some(self.f64(p + 8)?),
        })
    }

    fn option_i64(&self, p: u32) -> Result<Option<i64>, CallError> {
        Ok(match self.disc(p, 2)? {
            0 => None,
            _ => Some(self.u64(p + 8)? as i64),
        })
    }

    fn option_u32(&self, p: u32) -> Result<Option<u32>, CallError> {
        Ok(match self.disc(p, 2)? {
            0 => None,
            _ => Some(self.u32(p + 4)?),
        })
    }

    /// `plugin-error` (variant of strings, 12 bytes, align 4).
    fn plugin_error(&self, p: u32) -> Result<PluginFailure, CallError> {
        let m = crate::validate::sanitise_message(&self.string(p + 4)?);
        Ok(match self.disc(p, 3)? {
            0 => PluginFailure::BadParameter(m),
            1 => PluginFailure::Unsupported(m),
            _ => PluginFailure::Failed(m),
        })
    }

    /// `result<T, plugin-error>` where `T` sits at offset `ok_at`.
    fn result<T>(
        &self,
        p: u32,
        ok_at: u32,
        f: impl FnOnce(&Self, u32) -> Result<T, CallError>,
    ) -> Result<T, CallError> {
        match self.disc(p, 2)? {
            0 => f(self, p + ok_at),
            _ => Err(CallError::Plugin(self.plugin_error(p + 4)?)),
        }
    }

    // ── Composite types ─────────────────────────────────────────────────────

    /// `shape`: disc at 0, payload at 8; 56 bytes, align 8.
    fn shape(&self, p: u32) -> Result<Shape, CallError> {
        let q = p + 8;
        Ok(match self.disc(p, 3)? {
            0 => Shape::Line {
                a: self.vec2(q)?,
                b: self.vec2(q + 16)?,
            },
            1 => Shape::Arc {
                center: self.vec2(q)?,
                start: self.vec2(q + 16)?,
                end: self.vec2(q + 32)?,
            },
            _ => Shape::Circle {
                center: self.vec2(q)?,
                radius: self.f64(q + 16)?,
            },
        })
    }

    /// `path`: segments list at 0, closed at 8; 12 bytes, align 4. A segment
    /// is a shape (56) and `reversed` at 56; 64 bytes, align 8.
    fn path(&self, p: u32) -> Result<Path, CallError> {
        Ok(Path {
            segments: self.list(p, 64, 8, crate::validate::MAX_ITEMS, |m, s| {
                Ok(Segment {
                    shape: m.shape(s)?,
                    reversed: m.bool(s + 56)?,
                })
            })?,
            closed: self.bool(p + 8)?,
        })
    }

    fn paths(&self, p: u32) -> Result<Vec<Path>, CallError> {
        self.list(p, 12, 4, crate::validate::MAX_ITEMS, Self::path)
    }

    /// `param-spec`: key, label, help strings, then `param-kind` at 24 (disc
    /// at 24, payload at 32); 80 bytes, align 8.
    fn param_spec(&self, p: u32) -> Result<ParamSpec, CallError> {
        let q = p + 32;
        let kind = match self.disc(p + 24, 6)? {
            0 => ParamKind::Number(NumberSpec {
                unit: self.string(q)?,
                min: self.option_f64(q + 8)?,
                max: self.option_f64(q + 24)?,
                default: self.f64(q + 40)?,
            }),
            1 => ParamKind::Integer(IntegerSpec {
                min: self.option_i64(q)?,
                max: self.option_i64(q + 16)?,
                default: self.u64(q + 32)? as i64,
            }),
            2 => ParamKind::Boolean(self.bool(q)?),
            3 => ParamKind::Choice(ChoiceSpec {
                options: self.list(q, 8, 4, crate::validate::MAX_PARAMS, Self::string)?,
                default: self.u32(q + 8)?,
            }),
            4 => ParamKind::Text(TextSpec {
                default: self.string(q)?,
                max_length: self.u32(q + 8)?,
            }),
            _ => ParamKind::DocParameter,
        };
        Ok(ParamSpec {
            key: self.string(p)?,
            label: self.string(p + 8)?,
            help: self.string(p + 16)?,
            kind,
        })
    }

    fn schema(&self, p: u32) -> Result<Vec<ParamSpec>, CallError> {
        self.list(p, 80, 8, crate::validate::MAX_PARAMS, Self::param_spec)
    }

    /// `generated`: points, entities, constraints lists.
    fn generated(&self, p: u32) -> Result<Generated, CallError> {
        let max = crate::validate::MAX_ITEMS;
        Ok(Generated {
            points: self.list(p, 16, 8, max, Self::vec2)?,
            // entity: disc at 0, payload at 8; 24 bytes, align 8.
            entities: self.list(p + 8, 24, 8, max, |m, e| {
                let q = e + 8;
                Ok(match m.disc(e, 4)? {
                    0 => Entity::Point(m.u32(q)?),
                    1 => Entity::Line(m.u32(q)?, m.u32(q + 4)?),
                    2 => Entity::Circle {
                        center: m.u32(q)?,
                        radius: m.f64(q + 8)?,
                    },
                    _ => Entity::Arc {
                        center: m.u32(q)?,
                        start: m.u32(q + 4)?,
                        end: m.u32(q + 8)?,
                    },
                })
            })?,
            // constraint: disc at 0, payload at 8; 24 bytes, align 8.
            constraints: self.list(p + 16, 24, 8, max, |m, c| {
                let q = c + 8;
                let (a, b) = (m.u32(q)?, m.u32(q + 4)?);
                Ok(match m.disc(c, 9)? {
                    0 => Constraint::Coincident(a, b),
                    1 => Constraint::Horizontal(a),
                    2 => Constraint::Vertical(a),
                    3 => Constraint::Parallel(a, b),
                    4 => Constraint::Perpendicular(a, b),
                    5 => Constraint::Tangent(a, b),
                    6 => Constraint::Equal(a, b),
                    7 => Constraint::Distance(a, b, m.f64(q + 8)?),
                    _ => Constraint::Radius(a, m.f64(q + 8)?),
                })
            })?,
        })
    }

    /// `finding`: severity at 0, message at 4, layer option at 12, path
    /// option at 20; 28 bytes, align 4.
    fn finding(&self, p: u32) -> Result<Finding, CallError> {
        Ok(Finding {
            severity: match self.disc(p, 2)? {
                0 => Severity::Warning,
                _ => Severity::Error,
            },
            message: self.string(p + 4)?,
            layer: self.option_u32(p + 12)?,
            path: self.option_u32(p + 20)?,
        })
    }
}

// ── Writing parameters into plugin memory ───────────────────────────────────

/// Lowers values into the plugin. As the canonical ABI requires, every list
/// and string is its own `cabi_realloc` allocation: the guest owns them and
/// frees each one separately.
struct Lower<'a, I: Instance + ?Sized> {
    inst: &'a mut I,
}

/// An element buffer being filled for one list.
struct Buf(Vec<u8>);

impl Buf {
    fn put(&mut self, at: usize, b: &[u8]) {
        self.0[at..at + b.len()].copy_from_slice(b);
    }

    fn u32(&mut self, at: usize, v: u32) {
        self.put(at, &v.to_le_bytes());
    }

    fn f64(&mut self, at: usize, v: f64) {
        self.put(at, &v.to_le_bytes());
    }

    fn pair(&mut self, at: usize, (ptr, len): (u32, u32)) {
        self.u32(at, ptr);
        self.u32(at + 4, len);
    }

    fn vec2(&mut self, at: usize, v: Vec2) {
        self.f64(at, v.x);
        self.f64(at + 8, v.y);
    }

    fn shape(&mut self, at: usize, s: &Shape) {
        let q = at + 8;
        match *s {
            Shape::Line { a, b } => {
                self.0[at] = 0;
                self.vec2(q, a);
                self.vec2(q + 16, b);
            }
            Shape::Arc { center, start, end } => {
                self.0[at] = 1;
                self.vec2(q, center);
                self.vec2(q + 16, start);
                self.vec2(q + 32, end);
            }
            Shape::Circle { center, radius } => {
                self.0[at] = 2;
                self.vec2(q, center);
                self.f64(q + 16, radius);
            }
        }
    }
}

impl<I: Instance + ?Sized> Lower<'_, I> {
    /// Copies `data` into a fresh allocation; `(ptr, len)` with `len` items.
    fn alloc(&mut self, data: &[u8], align: u32, len: usize) -> Result<(u32, u32), CallError> {
        let len = u32::try_from(len)
            .map_err(|_| CallError::Host("the input is too large for a plugin".into()))?;
        if data.is_empty() {
            // Nothing to free; any aligned, non-null pointer will do.
            return Ok((align, len));
        }
        let size = i32::try_from(data.len())
            .map_err(|_| CallError::Host("the input is too large for a plugin".into()))?;
        let out = self.inst.call(
            "cabi_realloc",
            &[
                Val::I32(0),
                Val::I32(0),
                Val::I32(align as i32),
                Val::I32(size),
            ],
        )?;
        let ptr = match out.as_slice() {
            [Val::I32(p)] => *p as u32,
            _ => return Err(malformed("cabi_realloc returned the wrong type")),
        };
        if ptr % align != 0
            || (ptr as usize)
                .checked_add(data.len())
                .is_none_or(|e| e > self.inst.memory_len())
        {
            return Err(malformed("cabi_realloc returned an unusable pointer"));
        }
        self.inst.write(ptr as usize, data);
        Ok((ptr, len))
    }

    fn string(&mut self, s: &str) -> Result<(u32, u32), CallError> {
        self.alloc(s.as_bytes(), 1, s.len())
    }

    /// A list of `size`-byte, `align`-aligned elements, each filled by `f`
    /// (which may allocate nested lists and strings first).
    fn list<T>(
        &mut self,
        items: &[T],
        size: usize,
        align: u32,
        mut f: impl FnMut(&mut Self, &mut Buf, usize, &T) -> Result<(), CallError>,
    ) -> Result<(u32, u32), CallError> {
        let mut buf = Buf(vec![0; items.len() * size]);
        for (i, item) in items.iter().enumerate() {
            f(self, &mut buf, i * size, item)?;
        }
        self.alloc(&buf.0, align, items.len())
    }

    /// `path`: segments list at 0, closed at 8; 12 bytes, align 4. A segment
    /// is a shape and `reversed` at 56; 64 bytes, align 8.
    fn path(&mut self, buf: &mut Buf, at: usize, p: &Path) -> Result<(), CallError> {
        let segs = self.list(&p.segments, 64, 8, |_, b, s, seg| {
            b.shape(s, &seg.shape);
            b.0[s + 56] = u8::from(seg.reversed);
            Ok(())
        })?;
        buf.pair(at, segs);
        buf.0[at + 8] = u8::from(p.closed);
        Ok(())
    }

    fn paths(&mut self, paths: &[Path]) -> Result<(u32, u32), CallError> {
        self.list(paths, 12, 4, |l, b, at, p| l.path(b, at, p))
    }

    /// `param`: key at 0, `param-value` at 8 (disc at 8, payload at 16);
    /// 24 bytes, align 8.
    fn params(&mut self, params: &[Param]) -> Result<(u32, u32), CallError> {
        self.list(params, 24, 8, |l, b, at, p| {
            let key = l.string(&p.key)?;
            b.pair(at, key);
            let q = at + 16;
            match &p.value {
                ParamValue::Number(n) => {
                    b.0[at + 8] = 0;
                    b.f64(q, *n);
                }
                ParamValue::Integer(n) => {
                    b.0[at + 8] = 1;
                    b.put(q, &n.to_le_bytes());
                }
                ParamValue::Boolean(v) => {
                    b.0[at + 8] = 2;
                    b.0[q] = u8::from(*v);
                }
                ParamValue::Choice(c) => {
                    b.0[at + 8] = 3;
                    b.u32(q, *c);
                }
                ParamValue::Text(t) => {
                    b.0[at + 8] = 4;
                    let s = l.string(t)?;
                    b.pair(q, s);
                }
            }
            Ok(())
        })
    }

    /// `layer`: name at 0, colour at 8, `operation` at 16 (disc at 16,
    /// payload at 24), paths list at 40; 48 bytes, align 8.
    fn model(&mut self, model: &ExportModel) -> Result<(u32, u32), CallError> {
        self.list(&model.layers, 48, 8, |l, b, at, layer| {
            let name = l.string(&layer.name)?;
            b.pair(at, name);
            b.u32(at + 8, layer.colour);
            let (d, q) = (at + 16, at + 24);
            match layer.operation {
                Operation::Cut {
                    kerf_width,
                    compensation,
                    passes,
                } => {
                    b.0[d] = 0;
                    b.f64(q, kerf_width);
                    b.0[q + 8] = match compensation {
                        Compensation::None => 0,
                        Compensation::Inside => 1,
                        Compensation::Outside => 2,
                    };
                    b.u32(q + 12, passes);
                }
                Operation::Score => b.0[d] = 1,
                Operation::Engrave {
                    hatch_spacing,
                    hatch_angle,
                } => {
                    b.0[d] = 2;
                    b.f64(q, hatch_spacing);
                    b.f64(q + 8, hatch_angle);
                }
                Operation::Draw {
                    pen_number,
                    pen_width,
                } => {
                    b.0[d] = 3;
                    b.u32(q, pen_number);
                    b.f64(q + 8, pen_width);
                }
                Operation::DragKnife { blade_offset } => {
                    b.0[d] = 4;
                    b.f64(q, blade_offset);
                }
                Operation::Reference => b.0[d] = 5,
            }
            let paths = l.paths(&layer.paths)?;
            b.pair(at + 40, paths);
            Ok(())
        })
    }
}

fn args((ptr, len): (u32, u32)) -> [Val; 2] {
    [Val::I32(ptr as i32), Val::I32(len as i32)]
}

fn ret_ptr(out: Vec<Val>) -> Result<u32, CallError> {
    match out.as_slice() {
        [Val::I32(p)] => Ok(*p as u32),
        _ => Err(malformed("the plugin returned the wrong type")),
    }
}

// ── The calls ───────────────────────────────────────────────────────────────

/// `info.describe`.
pub fn describe<I: Instance + ?Sized>(inst: &mut I, api: &str) -> Result<PluginInfo, CallError> {
    let p = ret_ptr(inst.call(&export_name("info", api, "describe"), &[])?)?;
    let m = Mem::new(inst);
    Ok(PluginInfo {
        id: m.string(p)?,
        name: m.string(p + 8)?,
        version: m.string(p + 16)?,
    })
}

/// `<interface>.schema` for a generator, modifier or validator.
pub fn schema<I: Instance + ?Sized>(
    inst: &mut I,
    api: &str,
    interface: &str,
) -> Result<Vec<ParamSpec>, CallError> {
    let p = ret_ptr(inst.call(&export_name(interface, api, "schema"), &[])?)?;
    Mem::new(inst).schema(p)
}

/// `generator.generate`.
pub fn generate<I: Instance + ?Sized>(
    inst: &mut I,
    api: &str,
    params: &[Param],
) -> Result<Generated, CallError> {
    let a = args(Lower { inst: &mut *inst }.params(params)?);
    let p = ret_ptr(inst.call(&export_name("generator", api, "generate"), &a)?)?;
    Mem::new(inst).result(p, 4, Mem::generated)
}

/// `modifier.modify`.
pub fn modify<I: Instance + ?Sized>(
    inst: &mut I,
    api: &str,
    input: &[Path],
    params: &[Param],
) -> Result<Vec<Path>, CallError> {
    let a = args(Lower { inst: &mut *inst }.paths(input)?);
    let b = args(Lower { inst: &mut *inst }.params(params)?);
    let p = ret_ptr(inst.call(
        &export_name("modifier", api, "modify"),
        &[a[0], a[1], b[0], b[1]],
    )?)?;
    Mem::new(inst).result(p, 4, Mem::paths)
}

/// `exporter.format`.
pub fn format<I: Instance + ?Sized>(inst: &mut I, api: &str) -> Result<FormatInfo, CallError> {
    let p = ret_ptr(inst.call(&export_name("exporter", api, "format"), &[])?)?;
    let m = Mem::new(inst);
    Ok(FormatInfo {
        id: m.string(p)?,
        name: m.string(p + 8)?,
        extension: m.string(p + 16)?,
        mime: m.string(p + 24)?,
    })
}

/// `exporter.export`.
pub fn export<I: Instance + ?Sized>(
    inst: &mut I,
    api: &str,
    model: &ExportModel,
) -> Result<Vec<u8>, CallError> {
    let a = args(Lower { inst: &mut *inst }.model(model)?);
    let p = ret_ptr(inst.call(&export_name("exporter", api, "export"), &a)?)?;
    Mem::new(inst).result(p, 4, |m, q| m.byte_list(q))
}

/// `importer.extensions`.
pub fn extensions<I: Instance + ?Sized>(inst: &mut I, api: &str) -> Result<Vec<String>, CallError> {
    let p = ret_ptr(inst.call(&export_name("importer", api, "extensions"), &[])?)?;
    Mem::new(inst).list(p, 8, 4, crate::validate::MAX_EXTENSIONS, Mem::string)
}

/// `importer.import`.
pub fn import<I: Instance + ?Sized>(
    inst: &mut I,
    api: &str,
    bytes: &[u8],
) -> Result<Vec<Path>, CallError> {
    let a = args(Lower { inst: &mut *inst }.alloc(bytes, 1, bytes.len())?);
    let p = ret_ptr(inst.call(&export_name("importer", api, "import"), &a)?)?;
    Mem::new(inst).result(p, 4, Mem::paths)
}

/// `validator.validate`.
pub fn validate<I: Instance + ?Sized>(
    inst: &mut I,
    api: &str,
    model: &ExportModel,
    params: &[Param],
) -> Result<Vec<Finding>, CallError> {
    let a = args(Lower { inst: &mut *inst }.model(model)?);
    let b = args(Lower { inst: &mut *inst }.params(params)?);
    let p = ret_ptr(inst.call(
        &export_name("validator", api, "validate"),
        &[a[0], a[1], b[0], b[1]],
    )?)?;
    Mem::new(inst).result(p, 4, |m, q| {
        m.list(q, 28, 4, crate::validate::MAX_FINDINGS, Mem::finding)
    })
}
