// SPDX-License-Identifier: AGPL-3.0-or-later

//! The MCP server: tool list, resources, and the §16.4 gate every call goes
//! through (scope → caps → rate limit → approval → run on the document
//! thread). Transport-agnostic: stdio and HTTP serve the same handler.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use base64::Engine as _;
use rmcp::handler::server::common::schema_for_type;
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, Implementation,
    InitializeResult, ListResourcesResult, ListToolsResult, PaginatedRequestParams,
    ReadResourceRequestParams, ReadResourceResponse, ReadResourceResult, Resource,
    ResourceContents, ServerCapabilities, Tool, ToolAnnotations,
};
use rmcp::service::RequestContext;
use rmcp::{ErrorData, RoleServer, ServerHandler};
use serde_json::Value;

use crate::dto;
use crate::error::ToolError;
use crate::host::{ApprovalRequest, Approver, Backend, Workspace};
use crate::policy::{Gate, Grants, MAX_ARGS_BYTES, Policy, RateLimiter, Scope, cap};
use crate::tools::{self, Call, Output};

/// How long an approval prompt waits before counting as "no".
pub const APPROVAL_TIMEOUT: Duration = Duration::from_secs(300);

/// Resource URIs (SPEC §16.3 Resources).
pub const DOCUMENT_URI: &str = "locus://document";
pub const LAYERS_URI: &str = "locus://layers";
pub const PLUGINS_URI: &str = "locus://plugins";

/// What the host offers beyond the common tools.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Options {
    /// List the `save` tool (headless CLI: saves to the `--doc` file).
    pub save_tool: bool,
}

/// One entry of the activity log the GUI shows (no arguments, no contents).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Activity {
    pub tool: String,
    pub summary: String,
    pub ok: bool,
}

/// State shared by every request of one MCP session.
struct Session {
    backend: Arc<dyn Backend>,
    approver: Arc<dyn Approver>,
    policy: Mutex<Policy>,
    limiter: Mutex<RateLimiter>,
    options: Options,
    activity: Mutex<Vec<Activity>>,
}

/// The MCP server handler. Cheap to clone; clones share one session.
#[derive(Clone)]
pub struct LocusServer {
    s: Arc<Session>,
}

impl std::fmt::Debug for LocusServer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LocusServer")
            .field("options", &self.s.options)
            .finish_non_exhaustive()
    }
}

/// The most recent activity entries kept.
const ACTIVITY_LEN: usize = 50;

impl LocusServer {
    #[must_use]
    pub fn new(
        backend: Arc<dyn Backend>,
        approver: Arc<dyn Approver>,
        grants: Grants,
        options: Options,
    ) -> Self {
        Self {
            s: Arc::new(Session {
                backend,
                approver,
                policy: Mutex::new(Policy::new(grants)),
                limiter: Mutex::new(RateLimiter::default()),
                options,
                activity: Mutex::new(Vec::new()),
            }),
        }
    }

    /// Replace the rate limiter (tests).
    pub fn set_rate_limiter(&self, r: RateLimiter) {
        if let Ok(mut l) = self.s.limiter.lock() {
            *l = r;
        }
    }

    #[must_use]
    pub fn grants(&self) -> Grants {
        self.s.policy.lock().map(|p| p.grants()).unwrap_or_default()
    }

    /// The user changed the grants in the app.
    pub fn set_grants(&self, grants: Grants) {
        if let Ok(mut p) = self.s.policy.lock() {
            p.set_grants(grants);
        }
    }

    /// Recent calls, oldest first.
    #[must_use]
    pub fn activity(&self) -> Vec<Activity> {
        self.s
            .activity
            .lock()
            .map(|a| a.clone())
            .unwrap_or_default()
    }

    fn log(&self, tool: &str, summary: String, ok: bool) {
        tracing::debug!(tool, ok, "mcp tool call");
        if let Ok(mut a) = self.s.activity.lock() {
            if a.len() >= ACTIVITY_LEN {
                a.remove(0);
            }
            a.push(Activity {
                tool: tool.to_owned(),
                summary,
                ok,
            });
        }
    }

    /// Run `f` on the document thread and wait for its result.
    async fn on_doc<T: Send + 'static>(
        &self,
        f: impl FnOnce(&mut dyn Workspace) -> T + Send + 'static,
    ) -> Result<T, ToolError> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        let sent = self.s.backend.submit(Box::new(move |ws| {
            let _ = tx.send(f(ws));
        }));
        if !sent {
            return Err(ToolError::Unavailable("the document is closed".into()));
        }
        rx.await
            .map_err(|_| ToolError::Unavailable("the document is closed".into()))
    }

    /// The whole §16.4 gate, then the call. Public for tests and transports.
    ///
    /// # Errors
    /// A [`ToolError`] for the client; `Ok(None)` for an unknown tool.
    pub async fn call(
        &self,
        name: &str,
        args: serde_json::Map<String, Value>,
    ) -> Option<Result<Output, ToolError>> {
        let call = match Call::parse(name, &args)? {
            Ok(c) => c,
            Err(e) => return Some(Err(e)),
        };
        if matches!(call, Call::Save) && !self.s.options.save_tool {
            return None;
        }
        let summary = call.describe();
        let result = self.gate_and_run(name, call, &args).await;
        self.log(name, summary, result.is_ok());
        Some(result)
    }

    async fn gate_and_run(
        &self,
        name: &str,
        call: Call,
        args: &serde_json::Map<String, Value>,
    ) -> Result<Output, ToolError> {
        let scope = call.scope();
        let gate = self
            .s
            .policy
            .lock()
            .map_err(|_| ToolError::Unavailable("policy lock poisoned".into()))?
            .check(name, scope)?;
        if scope != Scope::Read {
            let size = serde_json::to_vec(args)
                .map(|v| v.len())
                .unwrap_or(usize::MAX);
            cap("arguments (bytes)", size, MAX_ARGS_BYTES)?;
            call.check_caps()?;
            self.s
                .limiter
                .lock()
                .map_err(|_| ToolError::Unavailable("limiter lock poisoned".into()))?
                .hit(Instant::now())?;
        }
        if gate == Gate::Ask {
            let (tx, rx) = tokio::sync::oneshot::channel();
            self.s.approver.ask(
                ApprovalRequest {
                    tool: name.to_owned(),
                    scope,
                    summary: call.describe(),
                },
                tx,
            );
            let yes = matches!(
                tokio::time::timeout(APPROVAL_TIMEOUT, rx).await,
                Ok(Ok(true))
            );
            if !yes {
                return Err(ToolError::Declined);
            }
            if let Ok(mut p) = self.s.policy.lock() {
                p.approved(scope);
            }
        }
        self.on_doc(move |ws| call.run(ws)).await?
    }

    /// Every tool, with schemas and annotations.
    #[must_use]
    pub fn tools(&self) -> Vec<Tool> {
        let mut t = vec![
            read::<dto::NoArgs>(
                "get_document_summary",
                "Layers, parameters, entity/constraint counts, degrees of freedom and solve status. Units are mm.",
            ),
            read::<dto::ListEntitiesArgs>(
                "list_entities",
                "List entities (points, lines, circles, arcs, Béziers, text) with geometry, filtered by layer, kind and an [x0,y0,x1,y1] box. Paged (max 500).",
            ),
            read::<dto::IdArgs>(
                "get_entity",
                "One entity with its geometry and everything that references it.",
            ),
            read::<dto::ListConstraintsArgs>(
                "list_constraints",
                "List constraints, optionally only those touching one entity. Dimensions in mm, angles in degrees.",
            ),
            read::<dto::NoArgs>(
                "get_dof_report",
                "Remaining degrees of freedom and which entities are not yet fully constrained.",
            ),
            read::<dto::NoArgs>(
                "get_conflicts",
                "Conflicting and redundant constraint groups, with entity IDs.",
            ),
            read::<dto::ValidateArgs>(
                "validate_export",
                "Export validation report: open cut paths, self-intersections, duplicates, with entity IDs.",
            ),
            read::<dto::NoArgs>(
                "list_plugins",
                "Installed plugins with their parameter schemas.",
            ),
            write::<dto::AddEntitiesArgs>(
                "add_entities",
                "Add entities and, optionally, constraints between them, as one undo step. Items can be named and referred to later in the same call; a point can be given as [x, y] to create it. Example line: {\"type\":\"line\",\"start\":[0,0],\"end\":[40,0]}.",
                false,
            ),
            write::<dto::ModifyEntityArgs>(
                "modify_entity",
                "Change an entity's geometry (same kind) or construction flag. The solver then re-applies constraints.",
                false,
            ),
            write::<dto::DeleteEntitiesArgs>(
                "delete_entities",
                "Delete entities; with cascade, also everything built on them.",
                true,
            ),
            write::<dto::AddConstraintsArgs>(
                "add_constraints",
                "Add constraints as one undo step. A constraint that would over-constrain the sketch is refused with the IDs it conflicts with.",
                false,
            ),
            write::<dto::IdsArgs>("remove_constraints", "Remove constraints.", true),
            write::<dto::SetDimensionArgs>(
                "set_dimension",
                "Change a dimension's value (mm, or degrees for angles), expression, or driven flag.",
                false,
            ),
            write::<dto::SetParameterArgs>(
                "set_parameter",
                "Add or change a named document parameter; dependent dimensions update.",
                false,
            ),
            write::<dto::CreateLayerArgs>(
                "create_layer",
                "Create a layer with a machine operation.",
                false,
            ),
            write::<dto::SetLayerPropsArgs>(
                "set_layer_props",
                "Change a layer's name, colour, operation, order, visibility, export flag or lock.",
                false,
            ),
            write::<dto::MoveToLayerArgs>(
                "move_to_layer",
                "Move entities to another layer; constraints are kept.",
                false,
            ),
            write::<dto::RunGeneratorArgs>(
                "run_generator",
                "Insert a generator plugin's output (see list_plugins for parameters).",
                false,
            ),
            write::<dto::ApplyModifierArgs>(
                "apply_modifier",
                "Apply a modifier plugin to a chain of curves, non-destructively.",
                false,
            ),
            Tool::new(
                "export",
                "Export the document (svg, pdf, dxf or a plugin format) and return the file contents. Never writes a file.",
                schema_for_type::<dto::ExportArgs>(),
            )
            .annotate(
                ToolAnnotations::new()
                    .read_only(true)
                    .open_world(false),
            ),
        ];
        if self.s.options.save_tool {
            t.push(write::<dto::NoArgs>(
                "save",
                "Save the document to the file this session was started with.",
                false,
            ));
        }
        t
    }

    fn resources() -> Vec<Resource> {
        vec![
            Resource::new(DOCUMENT_URI, "document")
                .with_description("The current document as a .locus (RON) snapshot")
                .with_mime_type("text/plain"),
            Resource::new(LAYERS_URI, "layers")
                .with_description("The layer list")
                .with_mime_type("application/json"),
            Resource::new(PLUGINS_URI, "plugins")
                .with_description("The installed plugin catalogue")
                .with_mime_type("application/json"),
        ]
    }

    /// Read a resource (read scope).
    ///
    /// # Errors
    /// Scope denied or the document is gone; `Ok(None)` for an unknown URI.
    pub async fn read(&self, uri: &str) -> Result<Option<(String, String)>, ToolError> {
        self.s
            .policy
            .lock()
            .map_err(|_| ToolError::Unavailable("policy lock poisoned".into()))?
            .check("resources/read", Scope::Read)?;
        let uri = uri.to_owned();
        self.on_doc(move |ws| -> Result<Option<(String, String)>, ToolError> {
            Ok(Some(match uri.as_str() {
                DOCUMENT_URI => (
                    "text/plain".to_owned(),
                    locus_doc::file::save(ws.sketcher().document())
                        .map_err(|e| ToolError::Unavailable(e.to_string()))?,
                ),
                LAYERS_URI => (
                    "application/json".to_owned(),
                    tools::layers(ws.sketcher().document()).to_string(),
                ),
                PLUGINS_URI => (
                    "application/json".to_owned(),
                    tools::list_plugins(ws).to_string(),
                ),
                _ => return Ok(None),
            }))
        })
        .await?
    }
}

fn read<T: schemars::JsonSchema + 'static>(name: &'static str, desc: &'static str) -> Tool {
    Tool::new(name, desc, schema_for_type::<T>())
        .annotate(ToolAnnotations::new().read_only(true).open_world(false))
}

fn write<T: schemars::JsonSchema + 'static>(
    name: &'static str,
    desc: &'static str,
    destructive: bool,
) -> Tool {
    Tool::new(name, desc, schema_for_type::<T>()).annotate(
        ToolAnnotations::new()
            .read_only(false)
            .destructive(destructive)
            .idempotent(false)
            .open_world(false),
    )
}

/// Is this MIME type text we can send as-is?
fn is_text(mime: &str) -> bool {
    mime.starts_with("text/")
        || mime.ends_with("+xml")
        || mime.ends_with("/xml")
        || mime == "application/dxf"
        || mime == "image/vnd.dxf"
        || mime == "application/json"
}

/// A tool result for the client: structured JSON plus the same as text.
#[must_use]
pub fn to_result(r: Result<Output, ToolError>) -> CallToolResult {
    match r {
        Ok(Output::Json(v)) => {
            let mut res = CallToolResult::success(vec![ContentBlock::text(v.to_string())]);
            res.structured_content = Some(v);
            res
        }
        Ok(Output::File { mime, bytes, info }) => {
            let uri = format!(
                "locus://export/document.{}",
                info["extension"].as_str().unwrap_or("bin")
            );
            let contents = match String::from_utf8(bytes) {
                Ok(text) if is_text(&mime) => {
                    ResourceContents::text(text, uri).with_mime_type(mime)
                }
                Ok(text) => ResourceContents::blob(
                    base64::engine::general_purpose::STANDARD.encode(text.as_bytes()),
                    uri,
                )
                .with_mime_type(mime),
                Err(e) => ResourceContents::blob(
                    base64::engine::general_purpose::STANDARD.encode(e.as_bytes()),
                    uri,
                )
                .with_mime_type(mime),
            };
            let mut res = CallToolResult::success(vec![
                ContentBlock::text(info.to_string()),
                ContentBlock::resource(contents),
            ]);
            res.structured_content = Some(info);
            res
        }
        Ok(Output::Image { png, info }) => {
            let mut res = CallToolResult::success(vec![
                ContentBlock::image(
                    base64::engine::general_purpose::STANDARD.encode(&png),
                    "image/png",
                ),
                ContentBlock::text(info.to_string()),
            ]);
            res.structured_content = Some(info);
            res
        }
        Err(e) => {
            let v = e.to_json();
            let mut res = CallToolResult::error(vec![ContentBlock::text(v.to_string())]);
            res.structured_content = Some(v);
            res
        }
    }
}

const INSTRUCTIONS: &str = "Locus is a constrained 2D vector editor for pen plotters, vinyl cutters and lasers. \
Units are millimetres; angles in tool arguments are degrees. Points are entities; lines, circles and arcs refer to point IDs, \
so shared corners are shared points. Start with get_document_summary. Each write tool call is one undo step the user can revert. \
Constraints that would over-constrain the sketch are refused with the conflicting constraint IDs. \
Writes and exports may need the user's approval in the app; a declined call changes nothing.";

impl ServerHandler for LocusServer {
    fn get_info(&self) -> InitializeResult {
        InitializeResult::new(
            ServerCapabilities::builder()
                .enable_tools()
                .enable_resources()
                .build(),
        )
        .with_server_info(Implementation::new("locus", env!("CARGO_PKG_VERSION")))
        .with_instructions(INSTRUCTIONS)
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        Ok(ListToolsResult::with_all_items(self.tools()))
    }

    fn get_tool(&self, name: &str) -> Option<Tool> {
        self.tools().into_iter().find(|t| t.name == name)
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        let args = request.arguments.unwrap_or_default();
        match self.call(&request.name, args).await {
            Some(r) => Ok(to_result(r).into()),
            None => Err(ErrorData::invalid_params(
                format!("unknown tool `{}`", request.name),
                None,
            )),
        }
    }

    async fn list_resources(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, ErrorData> {
        Ok(ListResourcesResult::with_all_items(Self::resources()))
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResponse, ErrorData> {
        match self.read(&request.uri).await {
            Ok(Some((mime, text))) => Ok(ReadResourceResult::new(vec![
                ResourceContents::text(text, request.uri).with_mime_type(mime),
            ])
            .into()),
            Ok(None) => Err(ErrorData::resource_not_found("unknown resource", None)),
            Err(e) => Err(ErrorData::invalid_request(e.to_string(), Some(e.to_json()))),
        }
    }
}
