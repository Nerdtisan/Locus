// SPDX-License-Identifier: AGPL-3.0-or-later

//! The core-module host over wasmtime's **core** API, on desktop (#253, #254).
//!
//! Not used to run plugins in the app (the component runtime does that); it
//! exists so the canonical-ABI code in [`super::abi`] can be checked against
//! wasmtime's own component lift/lower on every fixture and example, and so
//! the §15.8 suite can run against the web host's logic without a browser.
//! It applies the same limits as the desktop host: fuel, a 256 MB memory cap
//! counted by a limiter, nothing linked.

use wasmtime::{Config, Linker, Memory, Module, ResourceLimiter, Store, Trap, Val as WVal};

use super::abi::{Instance, Val};
use super::{CoreModule, Engine as CoreEngine};
use crate::error::CallError;

/// A compiled core module with its per-call limits.
pub struct WasmtimeCore {
    engine: wasmtime::Engine,
    module: Module,
    fuel: u64,
    memory_bytes: usize,
}

impl std::fmt::Debug for WasmtimeCore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WasmtimeCore").finish_non_exhaustive()
    }
}

impl WasmtimeCore {
    pub fn new(module: &CoreModule, fuel: u64, memory_bytes: usize) -> Result<Self, CallError> {
        let mut config = Config::new();
        config
            .consume_fuel(true)
            .cranelift_nan_canonicalization(true)
            .relaxed_simd_deterministic(true);
        let engine = wasmtime::Engine::new(&config).map_err(|e| CallError::Host(e.to_string()))?;
        let module = Module::from_binary(&engine, &module.wasm)
            .map_err(|e| CallError::Host(e.to_string()))?;
        Ok(WasmtimeCore {
            engine,
            module,
            fuel,
            memory_bytes,
        })
    }
}

struct Limit {
    cap: usize,
    exceeded: bool,
}

impl ResourceLimiter for Limit {
    fn memory_growing(
        &mut self,
        _: usize,
        desired: usize,
        _: Option<usize>,
    ) -> wasmtime::Result<bool> {
        if desired > self.cap {
            self.exceeded = true;
            return Ok(false);
        }
        Ok(true)
    }

    fn table_growing(
        &mut self,
        _: usize,
        desired: usize,
        _: Option<usize>,
    ) -> wasmtime::Result<bool> {
        Ok(desired <= 1_000_000)
    }
}

/// One fresh instance.
pub struct WasmtimeInstance {
    store: Store<Limit>,
    instance: wasmtime::Instance,
    memory: Memory,
}

impl std::fmt::Debug for WasmtimeInstance {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WasmtimeInstance").finish_non_exhaustive()
    }
}

impl CoreEngine for WasmtimeCore {
    type Instance = WasmtimeInstance;

    fn instantiate(&self) -> Result<WasmtimeInstance, CallError> {
        let mut store = Store::new(
            &self.engine,
            Limit {
                cap: self.memory_bytes,
                exceeded: false,
            },
        );
        store.limiter(|l| l);
        store
            .set_fuel(self.fuel)
            .map_err(|e| CallError::Host(e.to_string()))?;
        // An empty linker: the module imports nothing (checked at load).
        let linker = Linker::<Limit>::new(&self.engine);
        let instance = linker
            .instantiate(&mut store, &self.module)
            .map_err(|e| classify(&store, &e))?;
        let memory = instance
            .get_memory(&mut store, "memory")
            .ok_or_else(|| CallError::Host("no memory export".into()))?;
        Ok(WasmtimeInstance {
            store,
            instance,
            memory,
        })
    }
}

fn classify(store: &Store<Limit>, e: &wasmtime::Error) -> CallError {
    if store.data().exceeded {
        return CallError::MemoryLimit(store.data().cap);
    }
    match e.downcast_ref::<Trap>() {
        Some(Trap::OutOfFuel) => CallError::CpuLimit,
        Some(t) => CallError::Trap(t.to_string()),
        None => CallError::Host(crate::validate::sanitise_message(&e.to_string())),
    }
}

impl Instance for WasmtimeInstance {
    fn call(&mut self, name: &str, args: &[Val]) -> Result<Vec<Val>, CallError> {
        let f = self
            .instance
            .get_func(&mut self.store, name)
            .ok_or_else(|| CallError::Malformed(format!("the plugin doesn't export {name}")))?;
        let params: Vec<WVal> = args
            .iter()
            .map(|v| match *v {
                Val::I32(x) => WVal::I32(x),
                Val::I64(x) => WVal::I64(x),
                Val::F32(x) => WVal::F32(x.to_bits()),
                Val::F64(x) => WVal::F64(x.to_bits()),
            })
            .collect();
        let n = f.ty(&self.store).results().len();
        let mut results = vec![WVal::I32(0); n];
        f.call(&mut self.store, &params, &mut results)
            .map_err(|e| classify(&self.store, &e))?;
        results
            .into_iter()
            .map(|v| match v {
                WVal::I32(x) => Ok(Val::I32(x)),
                WVal::I64(x) => Ok(Val::I64(x)),
                WVal::F32(x) => Ok(Val::F32(f32::from_bits(x))),
                WVal::F64(x) => Ok(Val::F64(f64::from_bits(x))),
                _ => Err(CallError::Malformed("an unexpected result type".into())),
            })
            .collect()
    }

    fn memory_len(&self) -> usize {
        self.memory.data_size(&self.store)
    }

    fn read(&self, ptr: usize, len: usize) -> Vec<u8> {
        self.memory.data(&self.store)[ptr..ptr + len].to_vec()
    }

    fn write(&mut self, ptr: usize, bytes: &[u8]) {
        self.memory.data_mut(&mut self.store)[ptr..ptr + bytes.len()].copy_from_slice(bytes);
    }
}
