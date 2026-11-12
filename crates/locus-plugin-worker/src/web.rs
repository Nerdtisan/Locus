// SPDX-License-Identifier: AGPL-3.0-or-later

//! The worker's engine: the browser's `WebAssembly` API behind
//! `core_host::Engine`, and the message handler.

use js_sys::{Array, Function, Object, Reflect, Uint8Array, WebAssembly};
use locus_plugin_host::CallError;
use locus_plugin_host::core_host::wire::{Request, WireError, handle, reply_json};
use locus_plugin_host::core_host::{CoreModule, Engine, Instance, Val};
use wasm_bindgen::prelude::*;
use wasm_bindgen::{JsCast, JsValue};
use web_sys::{DedicatedWorkerGlobalScope, MessageEvent};

/// A plugin's compiled core module.
struct JsEngine {
    module: WebAssembly::Module,
}

impl JsEngine {
    fn new(m: &CoreModule) -> Result<JsEngine, CallError> {
        let bytes = Uint8Array::from(m.wasm.as_slice());
        let module = WebAssembly::Module::new(&bytes).map_err(|e| host(&e))?;
        // Belt and braces: the module was checked to import nothing.
        if WebAssembly::Module::imports(&module).length() != 0 {
            return Err(CallError::Host("the plugin imports something".into()));
        }
        Ok(JsEngine { module })
    }
}

fn message(e: &JsValue) -> String {
    let m = Reflect::get(e, &"message".into())
        .ok()
        .and_then(|m| m.as_string())
        .unwrap_or_else(|| "unknown error".into());
    locus_plugin_host::validate::sanitise_message(&m)
}

fn host(e: &JsValue) -> CallError {
    CallError::Host(message(e))
}

/// One fresh instance.
struct JsInstance {
    exports: Object,
    memory: WebAssembly::Memory,
}

impl Engine for JsEngine {
    type Instance = JsInstance;

    fn instantiate(&self) -> Result<JsInstance, CallError> {
        // The empty import object is the sandbox: there is nothing to call.
        let instance = WebAssembly::Instance::new(&self.module, &Object::new())
            .map_err(|e| CallError::Trap(message(&e)))?;
        let exports = instance.exports();
        let memory = Reflect::get(&exports, &"memory".into())
            .ok()
            .and_then(|m| m.dyn_into::<WebAssembly::Memory>().ok())
            .ok_or_else(|| CallError::Host("the plugin has no memory".into()))?;
        Ok(JsInstance { exports, memory })
    }
}

impl JsInstance {
    fn view(&self) -> Uint8Array {
        // Re-read the buffer every time: growing memory detaches the old one.
        Uint8Array::new(&self.memory.buffer())
    }
}

impl Instance for JsInstance {
    fn call(&mut self, name: &str, args: &[Val]) -> Result<Vec<Val>, CallError> {
        let f = Reflect::get(&self.exports, &name.into())
            .ok()
            .and_then(|f| f.dyn_into::<Function>().ok())
            .ok_or_else(|| CallError::Malformed(format!("the plugin doesn't export {name}")))?;
        let list = Array::new();
        for a in args {
            list.push(&match *a {
                Val::I32(x) => JsValue::from(x),
                Val::I64(x) => JsValue::from(js_sys::BigInt::from(x)),
                Val::F32(x) => JsValue::from(x),
                Val::F64(x) => JsValue::from(x),
            });
        }
        // A trap (unreachable, out of bounds, stack overflow…) throws.
        let out = f
            .apply(&JsValue::NULL, &list)
            .map_err(|e| CallError::Trap(message(&e)))?;
        if out.is_undefined() {
            return Ok(Vec::new());
        }
        // Every function the host calls returns one i32.
        let v = out
            .as_f64()
            .ok_or_else(|| CallError::Malformed("an unexpected result type".into()))?;
        Ok(vec![Val::I32(v as i32)])
    }

    fn memory_len(&self) -> usize {
        self.view().length() as usize
    }

    fn read(&self, ptr: usize, len: usize) -> Vec<u8> {
        self.view()
            .subarray(ptr as u32, (ptr + len) as u32)
            .to_vec()
    }

    fn write(&mut self, ptr: usize, bytes: &[u8]) {
        self.view()
            .subarray(ptr as u32, (ptr + bytes.len()) as u32)
            .copy_from(bytes);
    }
}

/// Answers one `{component, request}` message.
fn on_message(data: &JsValue) -> String {
    let component = Reflect::get(data, &"component".into())
        .ok()
        .and_then(|c| c.dyn_into::<Uint8Array>().ok())
        .map(|c| c.to_vec());
    let request = Reflect::get(data, &"request".into())
        .ok()
        .and_then(|r| r.as_string())
        .and_then(|r| serde_json::from_str::<Request>(&r).ok());
    let result = match (component, request) {
        (Some(c), Some(r)) => handle(JsEngine::new, &c, r),
        _ => Err(WireError::new(
            "Worker",
            "the worker got a malformed request",
        )),
    };
    reply_json(&result)
}

/// The first message the worker sends (wire::WORKER_READY).
const READY: &str = locus_plugin_host::core_host::wire::WORKER_READY;

pub fn start() {
    let scope: DedicatedWorkerGlobalScope = js_sys::global().unchecked_into();
    let reply_to = scope.clone();
    let handler = Closure::<dyn FnMut(MessageEvent)>::new(move |e: MessageEvent| {
        let reply = on_message(&e.data());
        // If posting fails the app's timeout ends the call.
        let _ = reply_to.post_message(&JsValue::from_str(&reply));
    });
    scope.set_onmessage(Some(handler.as_ref().unchecked_ref()));
    // The handler lives as long as the worker.
    handler.forget();
    // Messages that arrive before the handler is set are dropped, so the app
    // waits for this before sending the call.
    let _ = scope.post_message(&JsValue::from_str(READY));
}
