// SPDX-License-Identifier: AGPL-3.0-or-later

//! The §16.3 tools against fixture documents, through the full §16.4 gate
//! (#226, #227): every read and write tool, one undo step per AI action,
//! caps, rate limits, scopes and approval modes.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::{Arc, Mutex};
use std::time::Duration;

use locus_doc::generated::{GenEntity, GenPath, GeneratedGeometry, PluginRef};
use locus_geom::Vec2;
use locus_mcp::policy::RateLimiter;
use locus_mcp::server::DOCUMENT_URI;
use locus_mcp::tools::Output;
use locus_mcp::{
    ApprovalMode, ApprovalRequest, Approver, Backend, Grants, Headless, LocusServer, Options,
    PluginEntry, Plugins, Scope, ToolError,
};
use serde_json::{Map, Value, json};

/// Runs jobs inline on a shared workspace, so tests can inspect it.
#[derive(Clone)]
struct Inline(Arc<Mutex<Headless>>);

impl Backend for Inline {
    fn submit(&self, job: locus_mcp::host::Job) -> bool {
        job(&mut *self.0.lock().unwrap());
        true
    }
}

/// Answers every prompt with `yes`, and records what it was asked.
#[derive(Default)]
struct Answer {
    yes: bool,
    asked: Mutex<Vec<ApprovalRequest>>,
}

impl Approver for Answer {
    fn ask(&self, r: ApprovalRequest, reply: tokio::sync::oneshot::Sender<bool>) {
        self.asked.lock().unwrap().push(r);
        let _ = reply.send(self.yes);
    }
}

#[derive(Debug)]
struct FakePlugins;

impl Plugins for FakePlugins {
    fn list(&mut self) -> Vec<PluginEntry> {
        vec![PluginEntry {
            id: "org.example.square".into(),
            name: "Square".into(),
            version: "1.0.0".into(),
            kinds: vec!["generator".into(), "modifier".into()],
            generator_params: Some(json!({"type": "object"})),
            modifier_params: Some(json!({"type": "object"})),
        }]
    }

    fn generate(
        &mut self,
        id: &str,
        _: &Map<String, Value>,
    ) -> Result<(PluginRef, GeneratedGeometry), String> {
        if id != "org.example.square" {
            return Err("unknown plugin".into());
        }
        let p = |x, y| Vec2::new(x, y);
        Ok((
            PluginRef {
                id: id.into(),
                version: "1.0.0".into(),
                params: vec![],
            },
            GeneratedGeometry {
                points: vec![p(0.0, 0.0), p(5.0, 0.0), p(5.0, 5.0)],
                entities: vec![GenEntity::Line(0, 1), GenEntity::Line(1, 2)],
                constraints: vec![],
            },
        ))
    }

    fn modify(
        &mut self,
        id: &str,
        _: &Map<String, Value>,
        input: &[GenPath],
    ) -> Result<(PluginRef, GeneratedGeometry), String> {
        assert!(!input.is_empty());
        let (r, _) = self.generate(id, &Map::new())?;
        Ok((
            r,
            GeneratedGeometry {
                points: vec![Vec2::new(1.0, 1.0), Vec2::new(2.0, 2.0)],
                entities: vec![GenEntity::Line(0, 1)],
                constraints: vec![],
            },
        ))
    }
}

fn fixture(name: &str) -> locus_doc::Document {
    let path = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
    locus_doc::file::load(&std::fs::read_to_string(path).unwrap()).unwrap()
}

struct T {
    ws: Arc<Mutex<Headless>>,
    server: LocusServer,
    approver: Arc<Answer>,
}

fn setup(doc: &str, grants: Grants, yes: bool) -> T {
    let mut h = Headless::new(fixture(doc));
    h.plugins = Some(Box::new(FakePlugins));
    let ws = Arc::new(Mutex::new(h));
    let approver = Arc::new(Answer {
        yes,
        ..Answer::default()
    });
    let server = LocusServer::new(
        Arc::new(Inline(ws.clone())),
        approver.clone(),
        grants,
        Options::default(),
    );
    T {
        ws,
        server,
        approver,
    }
}

fn all() -> Grants {
    Grants::headless(true, true, true)
}

impl T {
    async fn call(&self, name: &str, args: Value) -> Result<Output, ToolError> {
        let Value::Object(args) = args else {
            panic!("args must be an object")
        };
        self.server
            .call(name, args)
            .await
            .unwrap_or_else(|| panic!("unknown tool {name}"))
    }

    async fn json(&self, name: &str, args: Value) -> Value {
        match self.call(name, args).await {
            Ok(Output::Json(v)) => v,
            other => panic!("{name}: {other:?}"),
        }
    }

    fn entities(&self) -> usize {
        self.ws.lock().unwrap().sketcher.document().entities().len()
    }

    fn constraints(&self) -> usize {
        self.ws
            .lock()
            .unwrap()
            .sketcher
            .document()
            .constraints()
            .len()
    }

    fn undo(&self) -> String {
        self.ws.lock().unwrap().sketcher.undo().unwrap()
    }

    fn can_undo(&self) -> bool {
        self.ws.lock().unwrap().sketcher.editor().can_undo()
    }
}

// ── Read tools (#226) ──────────────────────────────────────────────────────

#[tokio::test]
async fn summary_reports_layers_counts_and_solve_state() {
    let t = setup("plate.locus", Grants::default(), false);
    let v = t.json("get_document_summary", json!({})).await;
    assert_eq!(v["units"], "mm");
    assert_eq!(v["layers"][0]["id"], 1);
    assert_eq!(v["layers"][0]["operation"], "Cut");
    assert_eq!(v["parameters"]["width"]["value"], 60.0);
    assert_eq!(v["entities"], t.entities());
    assert_eq!(v["constraints"], t.constraints());
    assert!(v["solve"]["dof"].is_number());
    assert!(v["solve"]["converged"].as_bool().unwrap());
}

#[tokio::test]
async fn list_and_get_entities() {
    let t = setup("plate.locus", Grants::default(), false);
    let v = t.json("list_entities", json!({"kind": "line"})).await;
    assert_eq!(v["total"], 4);
    let line = &v["entities"][0];
    assert_eq!(line["kind"], "line");
    let at = &line["geom"]["start_at"];
    assert!(at[0].as_f64().unwrap().abs() < 1e-9 && at[1].as_f64().unwrap().abs() < 1e-9);

    let v = t
        .json("list_entities", json!({"within": [55.0, -1.0, 65.0, 1.0]}))
        .await;
    let ids: Vec<u64> = v["entities"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["id"].as_u64().unwrap())
        .collect();
    assert!(ids.contains(&3), "corner (60, 0) is in the box: {ids:?}");
    assert!(!ids.contains(&5), "corner (0, 40) is not");

    let v = t
        .json("list_entities", json!({"limit": 2, "offset": 1}))
        .await;
    assert_eq!(v["entities"].as_array().unwrap().len(), 2);
    assert_eq!(v["offset"], 1);

    let e = t.json("get_entity", json!({"id": 2})).await;
    assert_eq!(e["kind"], "point");
    assert!(
        e["used_by_constraints"]
            .as_array()
            .unwrap()
            .contains(&json!(13))
    );

    let err = t.call("get_entity", json!({"id": 999})).await.unwrap_err();
    assert_eq!(err.to_json()["error"]["kind"], "unknown_entity");
    assert_eq!(err.to_json()["error"]["entity"], 999);
}

#[tokio::test]
async fn constraints_dof_and_conflicts() {
    let t = setup("plate.locus", Grants::default(), false);
    let v = t.json("list_constraints", json!({"entity": 2})).await;
    let types: Vec<&str> = v["constraints"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["type"].as_str().unwrap())
        .collect();
    assert!(types.contains(&"fix") && types.contains(&"distance"));
    let dist = v["constraints"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["type"] == "distance")
        .unwrap();
    assert_eq!(dist["value"], 60.0);
    assert_eq!(dist["unit"], "mm");

    let d = t.json("get_dof_report", json!({})).await;
    assert!(d["dof"].is_number());
    assert!(d["not_fully_constrained"].is_array());

    let c = t.json("get_conflicts", json!({})).await;
    assert_eq!(c["conflicts"], json!([]));
}

#[tokio::test]
async fn validate_export_reports_open_paths() {
    let t = setup("open.locus", Grants::default(), false);
    let v = t.json("validate_export", json!({})).await;
    assert!(v["blocked"].as_bool().unwrap());
    let issue = &v["issues"][0];
    assert_eq!(issue["kind"], "open_path");
    assert!(!issue["entities"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn list_plugins_with_schemas() {
    let t = setup("plate.locus", Grants::default(), false);
    let v = t.json("list_plugins", json!({})).await;
    assert_eq!(v["available"], true);
    assert_eq!(v["plugins"][0]["id"], "org.example.square");
}

#[tokio::test]
async fn resources_and_bad_arguments() {
    let t = setup("plate.locus", Grants::default(), false);
    let (mime, text) = t.server.read(DOCUMENT_URI).await.unwrap().unwrap();
    assert_eq!(mime, "text/plain");
    assert!(locus_doc::file::load(&text).is_ok());
    assert!(t.server.read("locus://nope").await.unwrap().is_none());
    let err = t
        .call("list_entities", json!({"kind": "polygon"}))
        .await
        .unwrap_err();
    assert!(matches!(err, ToolError::BadArguments(_)));
    assert!(t.server.call("no_such_tool", Map::new()).await.is_none());
    // `save` exists only where the host offers it.
    assert!(t.server.call("save", Map::new()).await.is_none());
}

#[test]
fn every_tool_has_an_object_schema_and_annotations() {
    let t = setup("plate.locus", Grants::default(), false);
    let tools = t.server.tools();
    assert_eq!(tools.len(), 22);
    for tool in tools {
        assert_eq!(
            tool.input_schema.get("type"),
            Some(&json!("object")),
            "{}",
            tool.name
        );
        assert!(tool.annotations.is_some(), "{}", tool.name);
    }
}

// ── Write tools (#227) ─────────────────────────────────────────────────────

#[tokio::test]
async fn add_entities_with_named_refs_is_one_undo_step() {
    let t = setup("plate.locus", all(), false);
    let (e0, c0) = (t.entities(), t.constraints());
    let v = t
        .json(
            "add_entities",
            json!({
                "entities": [
                    {"name": "a", "type": "point", "x": 100.0, "y": 0.0},
                    {"name": "b", "type": "point", "x": 120.0, "y": 0.0},
                    {"name": "c", "type": "point", "x": 120.0, "y": 20.0},
                    {"name": "ab", "type": "line", "start": "a", "end": "$b"},
                    {"name": "bc", "type": "line", "start": "b", "end": "c"},
                    {"type": "line", "start": "c", "end": [100.0, 20.0]}
                ],
                "constraints": [
                    {"type": "horizontal", "line": "ab"},
                    {"type": "vertical", "line": "bc"},
                    {"type": "distance", "a": "a", "b": "b", "value": 25.0}
                ]
            }),
        )
        .await;
    assert_eq!(v["undo_label"], "AI: add 6 entities, 3 constraints");
    assert_eq!(t.entities(), e0 + 7, "6 entities plus the [x, y] point");
    assert_eq!(t.constraints(), c0 + 3);
    let a = v["names"]["a"].as_u64().unwrap();
    let b = v["names"]["b"].as_u64().unwrap();
    {
        let ws = t.ws.lock().unwrap();
        let doc = ws.sketcher.document();
        let pa = doc.point_pos(locus_doc::EntityId::new(a)).unwrap();
        let pb = doc.point_pos(locus_doc::EntityId::new(b)).unwrap();
        assert!(((pb.x - pa.x).hypot(pb.y - pa.y) - 25.0).abs() < 1e-6);
        assert_eq!(
            ws.sketcher.editor().undo_label(),
            Some("AI: add 6 entities, 3 constraints")
        );
    }
    assert_eq!(t.undo(), "AI: add 6 entities, 3 constraints");
    assert_eq!((t.entities(), t.constraints()), (e0, c0));
}

#[tokio::test]
async fn bad_name_and_failed_write_change_nothing() {
    let t = setup("plate.locus", all(), false);
    let (e0, c0) = (t.entities(), t.constraints());
    let err = t
        .call(
            "add_entities",
            json!({"entities": [{"type": "line", "start": "nope", "end": [1.0, 1.0]}]}),
        )
        .await
        .unwrap_err();
    assert!(matches!(err, ToolError::BadArguments(_)));
    // Contradicts the 60 mm width: refused, naming the constraints involved.
    let err = t
        .call(
            "add_constraints",
            json!({"constraints": [{"type": "distance", "a": 2, "b": 3, "value": 50.0}]}),
        )
        .await
        .unwrap_err();
    let j = err.to_json();
    let kind = j["error"]["kind"].as_str().unwrap();
    assert!(
        kind.starts_with("over_constrained") || kind == "unsolvable",
        "{j}"
    );
    assert!(!j["error"]["constraints"].as_array().unwrap().is_empty());
    assert_eq!((t.entities(), t.constraints()), (e0, c0));
    assert!(!t.can_undo());
}

#[tokio::test]
async fn modify_delete_and_constraints() {
    let t = setup("plate.locus", all(), false);
    // Move the free corner of the hole circle? Use the circle's radius.
    let v = t
        .json(
            "modify_entity",
            json!({"id": 11, "geom": {"type": "circle", "center": 10, "radius": 5.0}, "construction": true}),
        )
        .await;
    assert_eq!(v["undo_label"], "AI: modify entity 11");
    assert!(
        t.ws.lock()
            .unwrap()
            .sketcher
            .document()
            .entity(locus_doc::EntityId::new(11))
            .unwrap()
            .construction
    );
    t.undo();

    // Deleting a used point is refused with its users…
    let err = t
        .call("delete_entities", json!({"ids": [2]}))
        .await
        .unwrap_err();
    assert_eq!(err.to_json()["error"]["kind"], "entity_in_use");
    // …and cascades on request, as one step.
    let e0 = t.entities();
    let v = t
        .json("delete_entities", json!({"ids": [2], "cascade": true}))
        .await;
    assert_eq!(v["undo_label"], "AI: delete 1 entity and their dependents");
    assert_eq!(t.entities(), e0 - 3, "the point and its two lines");
    t.undo();
    assert_eq!(t.entities(), e0);

    let c0 = t.constraints();
    t.json("remove_constraints", json!({"ids": [18, 19]})).await;
    assert_eq!(t.constraints(), c0 - 2);
    t.undo();

    t.json("set_dimension", json!({"id": 18, "value": 70.0}))
        .await;
    let w = |t: &T| {
        let ws = t.ws.lock().unwrap();
        ws.sketcher
            .document()
            .point_pos(locus_doc::EntityId::new(3))
            .unwrap()
            .x
    };
    assert!((w(&t) - 70.0).abs() < 1e-6);
    t.undo();
    assert!((w(&t) - 60.0).abs() < 1e-6);
    t.json("set_dimension", json!({"id": 18, "expr": "width + 5"}))
        .await;
    assert!((w(&t) - 65.0).abs() < 1e-6);
}

#[tokio::test]
async fn angle_dimensions_are_degrees() {
    let t = setup("plate.locus", all(), false);
    let v = t
        .json(
            "add_entities",
            json!({
                "entities": [
                    {"name": "o", "type": "point", "x": 200.0, "y": 0.0},
                    {"name": "l1", "type": "line", "start": "o", "end": [210.0, 0.0]},
                    {"name": "l2", "type": "line", "start": "o", "end": [205.0, 5.0]}
                ],
                "constraints": [{"type": "angle", "a": "l1", "b": "l2", "value": 30.0}]
            }),
        )
        .await;
    let c = v["created"]["constraints"][0].as_u64().unwrap();
    let l = t.json("list_constraints", json!({})).await;
    let angle = l["constraints"]
        .as_array()
        .unwrap()
        .iter()
        .find(|x| x["id"] == c)
        .unwrap();
    assert!((angle["value"].as_f64().unwrap() - 30.0).abs() < 1e-9);
    assert_eq!(angle["unit"], "deg");
}

#[tokio::test]
async fn parameters_and_layers() {
    let t = setup("plate.locus", all(), false);
    t.json("set_parameter", json!({"name": "width", "expr": "80"}))
        .await;
    assert_eq!(
        t.ws.lock().unwrap().sketcher.document().parameters()["width"].value,
        80.0
    );
    let v = t
        .json(
            "create_layer",
            json!({"name": "Engrave", "colour": [0, 0, 255], "operation": "engrave"}),
        )
        .await;
    let layer = v["created"]["layers"][0].as_u64().unwrap();
    t.json(
        "set_layer_props",
        json!({"id": layer, "name": "Etch", "visible": false, "locked": true}),
    )
    .await;
    {
        let ws = t.ws.lock().unwrap();
        let l = ws
            .sketcher
            .document()
            .layer(locus_doc::LayerId::new(layer))
            .unwrap();
        assert_eq!(
            (l.name.as_str(), l.visible, l.locked),
            ("Etch", false, true)
        );
    }
    t.json("set_layer_props", json!({"id": layer, "locked": false}))
        .await;
    t.json("move_to_layer", json!({"entities": [11], "layer": layer}))
        .await;
    let c0 = t.constraints();
    assert_eq!(
        t.ws.lock()
            .unwrap()
            .sketcher
            .document()
            .entity(locus_doc::EntityId::new(11))
            .unwrap()
            .layer,
        locus_doc::LayerId::new(layer)
    );
    assert_eq!(t.constraints(), c0, "constraints survive a layer move (§4)");
    // Entities go to the named layer.
    let v = t
        .json(
            "add_entities",
            json!({"layer": layer, "entities": [{"type": "point", "x": 1.0, "y": 1.0}]}),
        )
        .await;
    let id = v["created"]["entities"][0].as_u64().unwrap();
    let e = t.json("get_entity", json!({"id": id})).await;
    assert_eq!(e["layer"], layer);
}

#[tokio::test]
async fn generator_and_modifier_plugins() {
    let t = setup("plate.locus", all(), false);
    let e0 = t.entities();
    let v = t
        .json(
            "run_generator",
            json!({"plugin": "org.example.square", "origin": [100.0, 100.0]}),
        )
        .await;
    assert_eq!(
        v["undo_label"],
        "AI: run generator plugin org.example.square"
    );
    assert!(t.entities() > e0);
    t.undo();
    assert_eq!(t.entities(), e0);
    let err = t
        .call("run_generator", json!({"plugin": "nope"}))
        .await
        .unwrap_err();
    assert!(matches!(err, ToolError::Plugin(_)));

    let v = t
        .json(
            "apply_modifier",
            json!({"plugin": "org.example.square", "sources": [6, 7, 8, 9]}),
        )
        .await;
    assert_eq!(v["created"]["recipes"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn export_returns_bytes_and_respects_validation() {
    let t = setup("plate.locus", all(), false);
    let Ok(Output::File { mime, bytes, info }) = t.call("export", json!({"format": "svg"})).await
    else {
        panic!("expected a file")
    };
    assert_eq!(mime, "image/svg+xml");
    assert!(String::from_utf8(bytes).unwrap().contains("<svg"));
    assert_eq!(info["format"], "svg");

    let t = setup("open.locus", all(), false);
    let err = t
        .call("export", json!({"format": "svg"}))
        .await
        .unwrap_err();
    assert!(matches!(err, ToolError::Export(_)));
    assert!(
        t.call("export", json!({"format": "svg", "force": true}))
            .await
            .is_ok()
    );
    let err = t
        .call("export", json!({"format": "png"}))
        .await
        .unwrap_err();
    assert!(matches!(err, ToolError::BadArguments(_)));
}

// ── Caps and rate limits (§16.4) ───────────────────────────────────────────

#[tokio::test]
async fn caps_reject_oversize_payloads() {
    let t = setup("plate.locus", all(), false);
    let many: Vec<Value> = (0..2001)
        .map(|i| json!({"type": "point", "x": f64::from(i), "y": 0.0}))
        .collect();
    let err = t
        .call("add_entities", json!({"entities": many}))
        .await
        .unwrap_err();
    assert!(matches!(
        err,
        ToolError::TooLarge {
            what: "entities",
            ..
        }
    ));
    let huge = "x".repeat(300 * 1024);
    let err = t
        .call("set_parameter", json!({"name": "p", "expr": huge}))
        .await
        .unwrap_err();
    assert!(matches!(err, ToolError::TooLarge { .. }));
    assert!(!t.can_undo());
}

#[tokio::test]
async fn write_calls_are_rate_limited() {
    let t = setup("plate.locus", all(), false);
    t.server
        .set_rate_limiter(RateLimiter::new(2, Duration::from_secs(60)));
    for i in 0..2 {
        t.json(
            "set_parameter",
            json!({"name": format!("p{i}"), "expr": "1"}),
        )
        .await;
    }
    let err = t
        .call("set_parameter", json!({"name": "p9", "expr": "1"}))
        .await
        .unwrap_err();
    assert!(matches!(err, ToolError::RateLimited { .. }));
    // Reads are never limited.
    t.json("get_document_summary", json!({})).await;
}

// ── Scopes and approval modes (§16.4, §16.5) ───────────────────────────────

#[tokio::test]
async fn default_grant_is_read_only() {
    let t = setup("plate.locus", Grants::default(), true);
    let err = t
        .call("set_parameter", json!({"name": "p", "expr": "1"}))
        .await
        .unwrap_err();
    assert_eq!(
        err,
        ToolError::ScopeDenied {
            tool: "set_parameter".into(),
            scope: "write"
        }
    );
    let err = t
        .call("export", json!({"format": "svg"}))
        .await
        .unwrap_err();
    assert!(matches!(
        err,
        ToolError::ScopeDenied {
            scope: "export",
            ..
        }
    ));
    assert!(t.approver.asked.lock().unwrap().is_empty(), "never asked");
    assert!(!t.can_undo());
}

#[tokio::test]
async fn withheld_read_scope_blocks_reads_and_resources() {
    let grants = Grants {
        read: false,
        ..Grants::default()
    };
    let t = setup("plate.locus", grants, true);
    assert!(t.call("get_document_summary", json!({})).await.is_err());
    assert!(t.server.read(DOCUMENT_URI).await.is_err());
}

#[tokio::test]
async fn ask_each_write_asks_every_time_and_a_no_changes_nothing() {
    let grants = Grants {
        write: true,
        ..Grants::default()
    };
    let t = setup("plate.locus", grants, false);
    let err = t
        .call("set_parameter", json!({"name": "p", "expr": "1"}))
        .await
        .unwrap_err();
    assert_eq!(err, ToolError::Declined);
    assert!(!t.can_undo());
    let asked = t.approver.asked.lock().unwrap().clone();
    assert_eq!(asked.len(), 1);
    assert_eq!(asked[0].scope, Scope::Write);
    assert_eq!(asked[0].summary, "set parameter p = 1");

    let t = setup("plate.locus", grants, true);
    for i in 0..2 {
        t.json(
            "set_parameter",
            json!({"name": format!("p{i}"), "expr": "1"}),
        )
        .await;
    }
    assert_eq!(t.approver.asked.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn ask_per_session_asks_once() {
    let grants = Grants {
        write: true,
        approval: ApprovalMode::AskPerSession,
        ..Grants::default()
    };
    let t = setup("plate.locus", grants, true);
    for i in 0..3 {
        t.json(
            "set_parameter",
            json!({"name": format!("p{i}"), "expr": "1"}),
        )
        .await;
    }
    assert_eq!(t.approver.asked.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn allow_mode_still_asks_for_export() {
    let grants = Grants {
        write: true,
        export: true,
        approval: ApprovalMode::Allow,
        ..Grants::default()
    };
    let t = setup("plate.locus", grants, true);
    t.json("set_parameter", json!({"name": "p", "expr": "1"}))
        .await;
    assert!(t.approver.asked.lock().unwrap().is_empty());
    assert!(t.call("export", json!({"format": "svg"})).await.is_ok());
    let asked = t.approver.asked.lock().unwrap().clone();
    assert_eq!(asked.len(), 1);
    assert_eq!(asked[0].scope, Scope::Export);
}

#[tokio::test]
async fn grants_can_change_mid_session() {
    let t = setup("plate.locus", Grants::default(), true);
    assert!(
        t.call("set_parameter", json!({"name": "p", "expr": "1"}))
            .await
            .is_err()
    );
    t.server.set_grants(Grants::headless(true, true, false));
    assert!(
        t.call("set_parameter", json!({"name": "p", "expr": "1"}))
            .await
            .is_ok()
    );
    let log = t.server.activity();
    assert_eq!(log.len(), 2);
    assert!(!log[0].ok && log[1].ok);
}
