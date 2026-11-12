// SPDX-License-Identifier: AGPL-3.0-or-later
//
// Runs every call in fixtures/expected.json through the real plugin worker,
// the way the app does (locus-app/src/web_plugins/web.js: one fresh worker
// per call, ready handshake, terminated on timeout), and compares with the
// desktop host's answers. Successes must match exactly; failures by class
// (see tests/web_fixtures.rs). The verdict goes to the page title and, when
// served by serve.py, to POST /result for CI.

const READY = "locus-plugin-worker:ready";
const TIMEOUT_MS = 2000;

function call(component, request) {
  return new Promise((resolve) => {
    const worker = new Worker("locus-plugin-worker_loader.js");
    let done = false;
    const finish = (json) => {
      if (done) return;
      done = true;
      clearTimeout(timer);
      worker.terminate();
      resolve(JSON.parse(json));
    };
    const err = (kind, message) => JSON.stringify({ Err: { kind, message } });
    const timer = setTimeout(() => finish(err("Timeout", "timed out")), TIMEOUT_MS);
    let sent = false;
    worker.onmessage = (e) => {
      if (!sent && e.data === READY) {
        sent = true;
        worker.postMessage({ component, request });
        return;
      }
      finish(typeof e.data === "string" ? e.data : err("Worker", "garbage"));
    };
    worker.onerror = () => finish(err("Worker", "crashed"));
  });
}

// JSON with sorted keys: the expected file and the worker order fields differently.
function canon(v) {
  if (Array.isArray(v)) return "[" + v.map(canon).join(",") + "]";
  if (v && typeof v === "object") {
    return "{" + Object.keys(v).sort().map((k) => JSON.stringify(k) + ":" + canon(v[k])).join(",") + "}";
  }
  return JSON.stringify(v);
}

// Same mapping as tests/web_fixtures.rs.
function cls(kind) {
  switch (kind) {
    case "CpuLimit": case "Timeout": return "limit";
    case "Trap": case "MemoryLimit": return "crash";
    case "Malformed": return "malformed";
    case "Plugin": return "plugin";
    case "NotProvided": return "not-provided";
    case "Refused": return "refused";
    default: return "host";
  }
}

async function main() {
  const out = document.getElementById("out");
  const lines = [];
  const log = (s) => { lines.push(s); out.textContent = lines.join("\n"); };
  const cases = await (await fetch("fixtures/expected.json")).json();
  const wasm = {};
  let failed = 0;
  for (const c of cases) {
    if (!wasm[c.file]) {
      wasm[c.file] = new Uint8Array(await (await fetch("fixtures/" + c.file)).arrayBuffer());
    }
    const t0 = performance.now();
    const got = await call(wasm[c.file], c.request);
    const ms = Math.round(performance.now() - t0);
    const req = c.request.length > 40 ? c.request.slice(0, 40) + "…" : c.request;
    let ok;
    if ("Ok" in c.expect) {
      ok = "Ok" in got && canon(got.Ok) === canon(c.expect.Ok);
    } else {
      ok = "Err" in got && cls(got.Err.kind) === c.expect.Err;
    }
    if (!ok) failed++;
    const shown = "Ok" in got ? "ok" : `${got.Err.kind}: ${got.Err.message}`;
    log(`${ok ? "PASS" : "FAIL"} ${c.name} ${req} -> ${shown} (${ms} ms)` +
      (ok ? "" : `  expected ${JSON.stringify(c.expect).slice(0, 200)}`));
  }
  const verdict = failed === 0 ? `ALL PASS (${cases.length})` : `FAILED ${failed} of ${cases.length}`;
  log(verdict);
  document.title = verdict;
  try {
    await fetch("/result", { method: "POST", body: lines.join("\n") });
  } catch (_) {
    // Not served by serve.py: the page title has the verdict.
  }
}

main().catch((e) => {
  document.title = "FAILED: " + e;
  fetch("/result", { method: "POST", body: "FAILED: " + e }).catch(() => {});
});
