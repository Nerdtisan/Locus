// Spike #252: runs plugin components inside a Web Worker and times it.
// Messages in: {mode: "pulley" | "native" | "spin", plugin: "finger-box" | "hpgl", calls}
// Messages out: {ok, ...timings in ms} or {ok: false, error}.

const now = () => performance.now();

async function bytes(url) {
  const r = await fetch(url);
  return new Uint8Array(await r.arrayBuffer());
}

// (B) wasmtime 50 + Pulley, compiled to wasm32 (probe.wasm, no imports).
let probe = null;
async function pulley(plugin, calls) {
  const t0 = now();
  if (!probe) {
    const { instance } = await WebAssembly.instantiateStreaming(fetch("probe.wasm"), {});
    probe = instance.exports;
  }
  const t1 = now();
  const wasm = await bytes(`${plugin}.wasm`);
  const ptr = probe.alloc(wasm.length);
  new Uint8Array(probe.memory.buffer, ptr, wasm.length).set(wasm);
  const t2 = now();
  if (probe.load(ptr, wasm.length) !== 0) throw new Error("load failed");
  const t3 = now();
  const call = plugin === "hpgl" ? probe.export_empty : probe.generate;
  let result = 0;
  const times = [];
  for (let i = 0; i < calls; i++) {
    const a = now();
    result = call();
    times.push(now() - a);
    if (result < 0) throw new Error("call failed");
  }
  return { runtimeStartup: t1 - t0, compile: t3 - t2, first: times[0],
           median: median(times.slice(1)), result };
}

// (C) The component's core module, run by the browser's own engine. Only
// `describe` is called: lifting the canonical ABI by hand is what option C
// would have to implement for every interface.
function coreModule(component) {
  // Component preamble (8 bytes), then sections: id, LEB128 size, payload.
  let i = 8;
  const leb = () => { let r = 0, s = 0, b; do { b = component[i++]; r |= (b & 0x7f) << s; s += 7; } while (b & 0x80); return r >>> 0; };
  while (i < component.length) {
    const id = component[i++];
    const size = leb();
    if (id === 1) return component.subarray(i, i + size);
    i += size;
  }
  throw new Error("no core module");
}

async function native(plugin, calls) {
  const wasm = await bytes(`${plugin}.wasm`);
  const t0 = now();
  const module = await WebAssembly.compile(coreModule(wasm));
  const t1 = now();
  const imports = WebAssembly.Module.imports(module);
  const times = [];
  let name = "";
  for (let i = 0; i < calls; i++) {
    const a = now();
    const inst = await WebAssembly.instantiate(module, {});
    const ret = inst.exports["locus:plugin/info@1.0.0#describe"]();
    const v = new DataView(inst.exports.memory.buffer);
    const p = v.getUint32(ret + 8, true), n = v.getUint32(ret + 12, true);
    name = new TextDecoder().decode(new Uint8Array(inst.exports.memory.buffer, p, n));
    times.push(now() - a);
  }
  return { compile: t1 - t0, first: times[0], median: median(times.slice(1)),
           imports: imports.length, result: name };
}

function median(a) {
  if (!a.length) return NaN;
  const s = [...a].sort((x, y) => x - y);
  return s[s.length >> 1];
}

onmessage = async (e) => {
  const { mode, plugin, calls } = e.data;
  try {
    if (mode === "spin") { for (;;) {} }
    const r = mode === "pulley" ? await pulley(plugin, calls) : await native(plugin, calls);
    postMessage({ ok: true, ...r });
  } catch (err) {
    postMessage({ ok: false, error: String(err) });
  }
};
