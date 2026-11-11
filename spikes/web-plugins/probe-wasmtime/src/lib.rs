//! Probe for #252, option B: wasmtime 50 with the Pulley interpreter,
//! compiled to `wasm32-unknown-unknown` and run in the browser.
//!
//! wasmtime's "custom platform" needs the embedder to provide a few C-ABI
//! functions (memory mapping, TLS). They are implemented at the bottom with
//! plain heap allocations, which is enough when memories have no guard
//! pages or reservations.
//!
//! Exports (raw C ABI, no wasm-bindgen, so the harness is plain JS):
//! - `alloc(len) -> ptr` for the harness to write a component into;
//! - `load(ptr, len) -> i32`: compile it (Cranelift → Pulley bytecode);
//! - `generate() -> i32`: fresh instance, schema, defaults, generate; the
//!   number of entities, or a negative error code;
//! - `export_empty() -> i32`: an exporter call on an empty model.

use std::alloc::{Layout, alloc_zeroed, dealloc};
use std::cell::RefCell;

mod bindings {
    pub mod generator {
        wasmtime::component::bindgen!({
            path: "../../../crates/locus-plugin-api/wit",
            world: "generator-plugin",
        });
    }
    pub mod exporter {
        wasmtime::component::bindgen!({
            path: "../../../crates/locus-plugin-api/wit",
            world: "exporter-plugin",
            with: { "locus:plugin/types": super::generator::locus::plugin::types },
        });
    }
}

use bindings::generator::locus::plugin::params as wp;
use wasmtime::component::{Component, Linker};
use wasmtime::{Config, Engine, Store};

struct Loaded {
    engine: Engine,
    component: Component,
}

thread_local! {
    static LOADED: RefCell<Option<Loaded>> = const { RefCell::new(None) };
}

fn engine() -> wasmtime::Result<Engine> {
    let mut c = Config::new();
    c.target("pulley32")?;
    c.consume_fuel(true);
    c.signals_based_traps(false);
    c.memory_reservation(0);
    c.memory_guard_size(0);
    c.memory_reservation_for_growth(0);
    c.memory_init_cow(false);
    Engine::new(&c)
}

fn store(engine: &Engine) -> wasmtime::Result<Store<()>> {
    let mut s = Store::new(engine, ());
    s.set_fuel(20_000_000_000)?;
    Ok(s)
}

thread_local! {
    static PANIC: RefCell<String> = const { RefCell::new(String::new()) };
}

/// The last panic message, for the harness (ptr; length via `panic_len`).
#[unsafe(no_mangle)]
pub extern "C" fn panic_ptr() -> *const u8 {
    PANIC.with(|p| p.borrow().as_ptr())
}

#[unsafe(no_mangle)]
pub extern "C" fn panic_len() -> usize {
    PANIC.with(|p| p.borrow().len())
}

#[unsafe(no_mangle)]
pub extern "C" fn alloc(len: usize) -> *mut u8 {
    std::panic::set_hook(Box::new(|info| {
        PANIC.with(|p| *p.borrow_mut() = info.to_string());
    }));
    let mut v = vec![0u8; len];
    let p = v.as_mut_ptr();
    std::mem::forget(v);
    p
}

/// # Safety
/// `ptr..ptr+len` must be readable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn load(ptr: *const u8, len: usize) -> i32 {
    let wasm = unsafe { std::slice::from_raw_parts(ptr, len) };
    let r = engine().and_then(|engine| {
        let component = Component::from_binary(&engine, wasm)?;
        Ok(Loaded { engine, component })
    });
    match r {
        Ok(l) => {
            LOADED.with(|c| *c.borrow_mut() = Some(l));
            0
        }
        Err(_) => -1,
    }
}

fn with<T>(f: impl FnOnce(&Loaded) -> wasmtime::Result<T>) -> wasmtime::Result<T> {
    LOADED.with(|c| {
        let c = c.borrow();
        let l = c.as_ref().ok_or_else(|| wasmtime::Error::msg("nothing loaded"))?;
        f(l)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn generate() -> i32 {
    let r = with(|l| {
        let linker = Linker::<()>::new(&l.engine);
        let mut s = store(&l.engine)?;
        let p = bindings::generator::GeneratorPlugin::instantiate(&mut s, &l.component, &linker)?;
        let g = p.locus_plugin_generator();
        let schema = g.call_schema(&mut s)?;
        let params: Vec<wp::Param> = schema
            .iter()
            .filter_map(|spec| {
                let value = match &spec.kind {
                    wp::ParamKind::Number(n) => wp::ParamValue::Number(n.default),
                    wp::ParamKind::Integer(n) => wp::ParamValue::Integer(n.default),
                    wp::ParamKind::Boolean(b) => wp::ParamValue::Boolean(*b),
                    wp::ParamKind::Choice(c) => wp::ParamValue::Choice(c.default),
                    wp::ParamKind::Text(t) => wp::ParamValue::Text(t.default.clone()),
                    wp::ParamKind::DocParameter => return None,
                };
                Some(wp::Param {
                    key: spec.key.clone(),
                    value,
                })
            })
            .collect();
        let out = g.call_generate(&mut s, &params)?;
        Ok(out.map(|g| g.entities.len()).unwrap_or(0))
    });
    match r {
        Ok(n) => n as i32,
        Err(_) => -1,
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn export_empty() -> i32 {
    let r = with(|l| {
        let linker = Linker::<()>::new(&l.engine);
        let mut s = store(&l.engine)?;
        let p = bindings::exporter::ExporterPlugin::instantiate(&mut s, &l.component, &linker)?;
        let model = bindings::generator::locus::plugin::types::ExportModel { layers: Vec::new() };
        let out = p.locus_plugin_exporter().call_export(&mut s, &model)?;
        Ok(out.map(|b| b.len()).unwrap_or(0))
    });
    match r {
        Ok(n) => n as i32,
        Err(_) => -1,
    }
}

// ── wasmtime custom-platform shims ──────────────────────────────────────────

const PAGE: usize = 65536;

#[unsafe(no_mangle)]
pub extern "C" fn wasmtime_page_size() -> usize {
    PAGE
}

/// # Safety
/// Called by wasmtime only.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn wasmtime_mmap_new(size: usize, _prot: u32, ret: &mut *mut u8) -> i32 {
    if size == 0 {
        *ret = PAGE as *mut u8;
        return 0;
    }
    let Ok(layout) = Layout::from_size_align(size, PAGE) else {
        return 1;
    };
    let p = unsafe { alloc_zeroed(layout) };
    if p.is_null() {
        return 1;
    }
    *ret = p;
    0
}

/// # Safety
/// Called by wasmtime only.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn wasmtime_mmap_remap(addr: *mut u8, size: usize, _prot: u32) -> i32 {
    unsafe { std::ptr::write_bytes(addr, 0, size) };
    0
}

/// # Safety
/// Called by wasmtime only, with a pointer from `wasmtime_mmap_new`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn wasmtime_munmap(ptr: *mut u8, size: usize) -> i32 {
    if size == 0 {
        return 0;
    }
    if let Ok(layout) = Layout::from_size_align(size, PAGE) {
        unsafe { dealloc(ptr, layout) };
    }
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn wasmtime_mprotect(_ptr: *mut u8, _size: usize, _prot: u32) -> i32 {
    0
}

/// No copy-on-write images: wasmtime then initialises memories by copying.
#[unsafe(no_mangle)]
pub extern "C" fn wasmtime_memory_image_new(
    _ptr: *const u8,
    _len: usize,
    ret: &mut *mut u8,
) -> i32 {
    *ret = std::ptr::null_mut();
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn wasmtime_memory_image_map_at(_image: *mut u8, _addr: *mut u8, _len: usize) -> i32 {
    1
}

#[unsafe(no_mangle)]
pub extern "C" fn wasmtime_memory_image_free(_image: *mut u8) {}

thread_local! {
    static TLS: std::cell::Cell<*mut u8> = const { std::cell::Cell::new(std::ptr::null_mut()) };
}

#[unsafe(no_mangle)]
pub extern "C" fn wasmtime_tls_get(_slot: usize) -> *mut u8 {
    TLS.with(std::cell::Cell::get)
}

#[unsafe(no_mangle)]
pub extern "C" fn wasmtime_tls_set(_slot: usize, ptr: *mut u8) {
    TLS.with(|t| t.set(ptr));
}
