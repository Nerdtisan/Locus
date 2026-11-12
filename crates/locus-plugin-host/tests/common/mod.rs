// SPDX-License-Identifier: AGPL-3.0-or-later

//! Fixture plugins for the §15.8 host suite, built in-process.
//!
//! Each fixture is a hand-written *core* WebAssembly module (WAT) that speaks
//! the canonical ABI of the `generator-plugin` world directly. Its return
//! values are laid out in data segments from Rust, so a fixture can return
//! exactly the bytes a hostile plugin would: bad pointers, invalid UTF-8,
//! NaNs, dangling indices. `wit-component` then wraps the module in a
//! component, as `wit-bindgen` + `wit-component` do for real plugins.
#![allow(dead_code, clippy::unwrap_used, clippy::expect_used)]

use std::fmt::Write as _;
use std::path::Path;

use wit_component::{ComponentEncoder, StringEncoding, embed_component_metadata};
use wit_parser::Resolve;

// Fixed addresses for the canned results.
pub const POINTS: u32 = 1024;
pub const ENTITIES: u32 = 1088;
pub const CONSTRAINTS: u32 = 1184;
pub const RESULT: u32 = 1232;
pub const INFO: u32 = 2000;
pub const SCHEMA: u32 = 2032;
pub const STRINGS: u32 = 2048;
pub const ERROR_MSG: u32 = 2200;
/// The bump allocator for `cabi_realloc` starts here.
pub const HEAP: u32 = 65536;

pub fn u32s(v: &[u32]) -> Vec<u8> {
    v.iter().flat_map(|x| x.to_le_bytes()).collect()
}

pub fn f64s(v: &[f64]) -> Vec<u8> {
    v.iter().flat_map(|x| x.to_le_bytes()).collect()
}

/// `generator.entity::line((a, b))`: discriminant 1, payload at offset 8.
pub fn line(a: u32, b: u32) -> Vec<u8> {
    let mut e = vec![1, 0, 0, 0, 0, 0, 0, 0];
    e.extend(u32s(&[a, b, 0, 0]));
    e
}

/// A one-index constraint (`horizontal` = 1, `vertical` = 2).
pub fn constraint1(disc: u8, a: u32) -> Vec<u8> {
    let mut c = vec![disc, 0, 0, 0, 0, 0, 0, 0];
    c.extend(u32s(&[a, 0, 0, 0]));
    c
}

/// `result<generated, _>::ok` with the three lists.
pub fn ok_generated(p: (u32, u32), e: (u32, u32), c: (u32, u32)) -> Vec<u8> {
    u32s(&[0, p.0, p.1, e.0, e.1, c.0, c.1])
}

/// `result<_, plugin-error>::err(case(msg))`.
pub fn err_generated(case: u8, msg: (u32, u32)) -> Vec<u8> {
    let mut r = u32s(&[1]);
    r.extend([case, 0, 0, 0]);
    r.extend(u32s(&[msg.0, msg.1]));
    r
}

/// A generator fixture. Defaults to the well-behaved one: a 10 mm square,
/// four lines, a horizontal and a vertical constraint.
#[derive(Clone)]
pub struct Fixture {
    /// `locus:plugin` version in the embedded WIT and the export names.
    pub api_version: String,
    pub id: Vec<u8>,
    pub name: Vec<u8>,
    pub version: Vec<u8>,
    /// WAT body of `generate`, leaving an i32 (the result pointer).
    pub generate: String,
    /// Extra WAT functions (helpers for `generate`).
    pub extra: String,
    pub data: Vec<(u32, Vec<u8>)>,
    pub memory_pages: u32,
    /// Import a clock function, which the sandbox must refuse.
    pub import_clock: bool,
    /// Which world the fixture targets. `generate` is the body of its main
    /// call: `generate`, `import` or `validate` (modifiers echo their input).
    pub world: World,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum World {
    Generator,
    Modifier,
    Importer,
    Validator,
}

/// `result<list<T>, _>::ok((ptr, len))`.
pub fn ok_list(ptr: u32, len: u32) -> Vec<u8> {
    u32s(&[0, ptr, len])
}

/// A `segment` holding a circle (64 bytes: shape variant, then `reversed`).
pub fn circle_segment(x: f64, y: f64, r: f64) -> Vec<u8> {
    let mut s = vec![2, 0, 0, 0, 0, 0, 0, 0];
    s.extend(f64s(&[x, y, r]));
    s.resize(64, 0);
    s
}

/// A `path` record: segments (ptr, len), closed.
pub fn path(segs: (u32, u32), closed: bool) -> Vec<u8> {
    let mut p = u32s(&[segs.0, segs.1]);
    p.extend([u8::from(closed), 0, 0, 0]);
    p
}

/// A validator `finding` (28 bytes). Severity: 0 warning, 1 error.
pub fn finding(severity: u8, msg: (u32, u32), layer: Option<u32>, path: Option<u32>) -> Vec<u8> {
    let mut f = vec![severity, 0, 0, 0];
    f.extend(u32s(&[msg.0, msg.1]));
    for o in [layer, path] {
        f.extend(u32s(&[u32::from(o.is_some()), o.unwrap_or(0)]));
    }
    f
}

pub const FINDING_MSG: &[u8] = b"fixture finding";

impl Fixture {
    pub fn well_behaved() -> Fixture {
        let points = f64s(&[0.0, 0.0, 10.0, 0.0, 10.0, 10.0, 0.0, 10.0]);
        let entities = [line(0, 1), line(1, 2), line(2, 3), line(3, 0)].concat();
        let constraints = [constraint1(1, 0), constraint1(2, 1)].concat();
        Fixture {
            api_version: "1.0.0".into(),
            id: b"com.example.fixture".to_vec(),
            name: b"Fixture".to_vec(),
            version: b"1.0.0".to_vec(),
            generate: format!("i32.const {RESULT}"),
            extra: String::new(),
            data: vec![
                (POINTS, points),
                (ENTITIES, entities),
                (CONSTRAINTS, constraints),
                (
                    RESULT,
                    ok_generated((POINTS, 4), (ENTITIES, 4), (CONSTRAINTS, 2)),
                ),
            ],
            memory_pages: 2,
            import_clock: false,
            world: World::Generator,
        }
    }

    /// An importer claiming `.plt` that returns one closed path: a circle
    /// of radius 5 at (10, 10).
    pub fn importer() -> Fixture {
        Fixture {
            id: b"com.example.importer".to_vec(),
            name: b"Importer".to_vec(),
            world: World::Importer,
            data: vec![
                (POINTS, circle_segment(10.0, 10.0, 5.0)),
                (ENTITIES, path((POINTS, 1), true)),
                (RESULT, ok_list(ENTITIES, 1)),
                // extensions: list<string> at 1184 → one string "plt".
                (CONSTRAINTS, u32s(&[1192, 1, 1200, 3])),
                (1200, b"plt".to_vec()),
            ],
            generate: format!("i32.const {RESULT}"),
            ..Fixture::well_behaved()
        }
    }

    /// A validator returning one warning on layer 0.
    pub fn validator() -> Fixture {
        Fixture {
            id: b"com.example.validator".to_vec(),
            name: b"Validator".to_vec(),
            world: World::Validator,
            data: vec![
                (ERROR_MSG, FINDING_MSG.to_vec()),
                (
                    POINTS,
                    finding(0, (ERROR_MSG, FINDING_MSG.len() as u32), Some(0), None),
                ),
                (RESULT, ok_list(POINTS, 1)),
            ],
            generate: format!("i32.const {RESULT}"),
            ..Fixture::well_behaved()
        }
    }

    /// An identity modifier: `modify` returns the input paths as they are.
    pub fn modifier_identity() -> Fixture {
        Fixture {
            id: b"com.example.identity".to_vec(),
            name: b"Identity".to_vec(),
            world: World::Modifier,
            ..Fixture::well_behaved()
        }
    }

    /// Replaces (or adds) the data at `addr`.
    pub fn with_data(mut self, addr: u32, bytes: Vec<u8>) -> Fixture {
        self.data.retain(|(a, _)| *a != addr);
        self.data.push((addr, bytes));
        self
    }

    pub fn with_generate(mut self, body: &str) -> Fixture {
        self.generate = body.into();
        self
    }

    fn wat(&self) -> String {
        let v = &self.api_version;
        let id_at = STRINGS;
        let name_at = id_at + self.id.len() as u32;
        let ver_at = name_at + self.name.len() as u32;
        let strings = [self.id.clone(), self.name.clone(), self.version.clone()].concat();
        let info = u32s(&[
            id_at,
            self.id.len() as u32,
            name_at,
            self.name.len() as u32,
            ver_at,
            self.version.len() as u32,
        ]);
        let mut data = vec![(STRINGS, strings), (INFO, info), (SCHEMA, u32s(&[8, 0]))];
        data.extend(self.data.iter().cloned());

        let mut w = String::from("(module\n");
        if self.import_clock {
            w += "  (import \"$root\" \"now\" (func $now (result i64)))\n";
        }
        let _ = writeln!(w, "  (memory (export \"memory\") {})", self.memory_pages);
        let _ = writeln!(w, "  (global $heap (mut i32) (i32.const {HEAP}))");
        // A bump allocator: enough for the host to lower `list<param>`.
        w += r#"  (func (export "cabi_realloc") (param i32 i32 i32 i32) (result i32)
    (local $p i32)
    (local.set $p (i32.and
      (i32.add (global.get $heap) (i32.sub (local.get 2) (i32.const 1)))
      (i32.sub (i32.const 0) (local.get 2))))
    (global.set $heap (i32.add (local.get $p) (local.get 3)))
    (block $ok
      (br_if $ok (i32.le_u (global.get $heap)
        (i32.mul (memory.size) (i32.const 65536))))
      (br_if $ok (i32.ne (memory.grow (i32.add (i32.div_u
        (i32.sub (global.get $heap) (i32.mul (memory.size) (i32.const 65536)))
        (i32.const 65536)) (i32.const 1))) (i32.const -1)))
      unreachable)
    local.get $p)
"#;
        let _ = writeln!(
            w,
            "  (func (export \"locus:plugin/info@{v}#describe\") (result i32) i32.const {INFO})"
        );
        if self.world == World::Importer {
            let _ = writeln!(
                w,
                "  (func (export \"locus:plugin/importer@{v}#extensions\") (result i32) i32.const {CONSTRAINTS})"
            );
            let _ = writeln!(
                w,
                "  (func (export \"locus:plugin/importer@{v}#import\") (param i32 i32) (result i32)\n    {})",
                self.generate
            );
        } else if self.world == World::Validator {
            let _ = writeln!(
                w,
                "  (func (export \"locus:plugin/validator@{v}#schema\") (result i32) i32.const {SCHEMA})"
            );
            let _ = writeln!(
                w,
                "  (func (export \"locus:plugin/validator@{v}#validate\") (param i32 i32 i32 i32) (result i32)\n    {})",
                self.generate
            );
        } else if self.world == World::Modifier {
            let _ = writeln!(
                w,
                "  (func (export \"locus:plugin/modifier@{v}#schema\") (result i32) i32.const {SCHEMA})"
            );
            // result<list<path>, _>::ok(input): discriminant 0, then (ptr, len).
            let _ = writeln!(
                w,
                "  (func (export \"locus:plugin/modifier@{v}#modify\") (param i32 i32 i32 i32) (result i32)\n    (i32.store (i32.const {RESULT}) (i32.const 0))\n    (i32.store (i32.const {}) (local.get 0))\n    (i32.store (i32.const {}) (local.get 1))\n    i32.const {RESULT})",
                RESULT + 4,
                RESULT + 8
            );
        } else {
            let _ = writeln!(
                w,
                "  (func (export \"locus:plugin/generator@{v}#schema\") (result i32) i32.const {SCHEMA})"
            );
            let _ = writeln!(
                w,
                "  (func (export \"locus:plugin/generator@{v}#generate\") (param i32 i32) (result i32)\n    {})",
                self.generate
            );
        }
        w += &self.extra;
        for (addr, bytes) in &data {
            let _ = write!(w, "  (data (i32.const {addr}) \"");
            for b in bytes {
                let _ = write!(w, "\\{b:02x}");
            }
            w += "\")\n";
        }
        w += ")\n";
        w
    }

    /// The component bytes.
    pub fn build(&self) -> Vec<u8> {
        let wit_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../locus-plugin-api/wit");
        let mut wit = std::fs::read_to_string(wit_dir.join("plugin.wit")).unwrap();
        wit = wit.replace(
            "locus:plugin@1.0.0",
            &format!("locus:plugin@{}", self.api_version),
        );
        if self.import_clock {
            wit = wit.replace(
                "world generator-plugin {",
                "world generator-plugin {\n    import now: func() -> u64;",
            );
        }
        let mut resolve = Resolve::new();
        let pkg = resolve.push_str("plugin.wit", &wit).unwrap();
        let world_name = match self.world {
            World::Generator => "generator-plugin",
            World::Modifier => "modifier-plugin",
            World::Importer => "importer-plugin",
            World::Validator => "validator-plugin",
        };
        let world = resolve.select_world(&[pkg], Some(world_name)).unwrap();

        let mut module = wat::parse_str(self.wat()).unwrap();
        embed_component_metadata(&mut module, &resolve, world, StringEncoding::UTF8, false)
            .unwrap();
        ComponentEncoder::default()
            .module(&module)
            .unwrap()
            .validate(true)
            .encode()
            .unwrap()
    }
}
