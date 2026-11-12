# Using Locus from an AI client (MCP)

Locus has a built-in [Model Context Protocol](https://modelcontextprotocol.io) server, so you can drive it from **your own** AI client: Claude Desktop, Claude Code, or a local LLM app. Locus ships no AI model and talks to no cloud service (SPEC §16.1).

Two ways to connect (SPEC §16.2):

| | **App (HTTP)** | **Headless (stdio)** |
|---|---|---|
| Runs | inside the open Locus window | `locus-cli mcp --doc file.locus` |
| You see edits live | yes | no (open the file afterwards) |
| Address | `http://127.0.0.1:47811/mcp` + bearer token | the client starts the process |
| Asks before changes | yes (your choice of mode) | no: the `--scopes` you pass are the permission |
| Saving | you save in the app | the `save` tool writes the `--doc` file |
| Platforms | Windows, macOS, Linux desktop (not the web build) | same |

Both speak MCP revision **2026-07-28**, and also accept clients still on **2025-11-25** (ADR 0005). Nothing client-specific: any client that follows the MCP spec should work.

## Turning it on in the app

1. **View ▸ MCP server (AI clients)…** ▸ **Turn on…**
2. Read the dialog and choose **Turn on**. Locus remembers this for this install; it is off until you do this.
3. Choose what the AI may do:
   - **Read the document**: on by default.
   - **Change the document**: off by default. Then choose when to ask: *every time*, *once until the server is turned off*, or *don't ask*.
   - **Export files**: off by default. Export asks every time unless you tick *Export without asking*.
   - **Allow Docker clients (host.docker.internal)**: off by default. Tick it (while the server is off) only if your AI client runs in Docker Desktop on this computer, such as Open WebUI in a container. See [Open WebUI](#open-webui).
4. Copy the token or a config snippet into your client (below).

While the server runs, the status bar shows **● MCP on (…)**. It turns to **● MCP: approval needed** when the AI is waiting for you; the prompt says what it wants to do. *Deny* changes nothing.

Every AI change is **one undo step**, labelled like `AI: add 8 entities, 7 constraints`, so Ctrl+Z (Cmd+Z) reverts exactly one AI action.

The token is made fresh **each time the server starts** (turning it on, or launching Locus with it on). Update your client's config then. The port (47811 by default) can be changed in the same window while the server is off.

## Headless: `locus-cli mcp`

```
locus-cli mcp --doc drawing.locus [--scopes read,write,export] [--plugins <dir>]
```

- `--scopes` defaults to `read`. Add `write` to let the AI edit, `export` to let it export. There is no prompt: passing a scope is your permission for that session.
- With `write`, a `save` tool writes `drawing.locus` (temp file + rename, so a failed save never damages it). If the file doesn't exist yet, the session starts with a new document.
- `export` returns the file's contents to the client; it never writes a file.
- `--plugins <dir>` makes installed generator, modifier and exporter plugins available.

## Client setup

Replace `TOKEN` with the token from the app (**Copy token**), and paths with your own. The app's **Copy JSON config** and **Copy Claude Code command** buttons fill these in for you.

### Claude Code

Attached to the app (HTTP):

```
claude mcp add --transport http locus http://127.0.0.1:47811/mcp --header "Authorization: Bearer TOKEN"
```

Headless, on a file:

```
claude mcp add locus -- locus-cli mcp --doc /path/to/drawing.locus --scopes read,write
```

Or in a project's `.mcp.json`:

```json
{
  "mcpServers": {
    "locus": {
      "type": "http",
      "url": "http://127.0.0.1:47811/mcp",
      "headers": { "Authorization": "Bearer TOKEN" }
    }
  }
}
```

### Claude Desktop

Claude Desktop's config file runs local servers over stdio, so use the headless server. Edit `claude_desktop_config.json` (Settings ▸ Developer ▸ Edit Config; on Windows `%APPDATA%\Claude\`, on macOS `~/Library/Application Support/Claude/`) and restart Claude Desktop:

```json
{
  "mcpServers": {
    "locus": {
      "command": "C:\\path\\to\\locus-cli.exe",
      "args": ["mcp", "--doc", "C:\\Users\\you\\Documents\\drawing.locus", "--scopes", "read,write,export"]
    }
  }
}
```

Open the file in Locus afterwards to see the result (or ask Claude to call `render_preview`).

### LM Studio

LM Studio reads `mcp.json` in the same `mcpServers` notation (Program tab ▸ Install ▸ Edit mcp.json). Attached to the app:

```json
{
  "mcpServers": {
    "locus": {
      "url": "http://127.0.0.1:47811/mcp",
      "headers": { "Authorization": "Bearer TOKEN" }
    }
  }
}
```

or headless:

```json
{
  "mcpServers": {
    "locus": {
      "command": "locus-cli",
      "args": ["mcp", "--doc", "/path/to/drawing.locus", "--scopes", "read,write"]
    }
  }
}
```

### llama.cpp (`llama-server` web UI)

The llama-server web UI is an MCP client **running in your browser**. Browsers send an `Origin` header, and Locus refuses every request that has one: that is what stops a web page you happen to visit from reaching your drawing (DNS rebinding, SPEC §16.4). So the web UI cannot connect directly. With llama-server's MCP proxy (`--webui-mcp-proxy`, and the proxy toggle on the connection), the request comes from llama-server instead; it works if the proxy does not pass the browser's `Origin` on. If it is refused with *browser requests are not accepted*, use LM Studio or another desktop client with the same model.

URL `http://127.0.0.1:47811/mcp`, header `Authorization: Bearer TOKEN`.

### Open WebUI

Open WebUI connects to MCP servers from its backend (not from your browser), so the `Origin` rule doesn't get in the way. Where its backend runs decides the URL:

- **Installed directly on this computer** (`pip`, desktop app): URL `http://127.0.0.1:47811/mcp`. No extra setting.
- **In Docker Desktop on this computer** (the usual `docker run … open-webui` install): inside a container, `127.0.0.1` is the container itself, so it must use `http://host.docker.internal:47811/mcp`. Locus refuses that name unless you turn on **Allow Docker clients**:
  1. In Locus, View ▸ MCP server…, turn the server off if it's on, tick **Allow Docker clients (host.docker.internal)**, then **Turn on…**. The window then shows the Docker URL.
  2. Copy the token.

Then in Open WebUI: **Admin Settings ▸ External Tools ▸ +** (add a tool server):

- Type: **MCP (Streamable HTTP)**
- URL: `http://host.docker.internal:47811/mcp` (Docker) or `http://127.0.0.1:47811/mcp` (installed directly)
- Auth: **Bearer**, and paste the token
- Save, then enable the tool in a chat.

With **Allow Docker clients** on, any program in a Docker container on this computer can try to connect. It still needs the token, and everything else below still applies. Turn it off again when you no longer need it.

### Ollama-based front-ends

Ollama itself is not an MCP client; the front-end you chat in is. Front-ends that connect to Streamable HTTP MCP servers from a backend with a custom header use the URL and header above, or the headless command where they can start stdio servers. If that backend runs in Docker Desktop, it needs **Allow Docker clients** and the `host.docker.internal` URL, as for [Open WebUI](#open-webui). Browser-only front-ends hit the same `Origin` rule as llama.cpp's web UI.

### Unsloth Studio

Unsloth Studio can add custom MCP servers. Add Locus with the URL and `Authorization` header above; if your version only takes a command, use the headless form.

### Any other client

- Streamable HTTP: `POST http://127.0.0.1:47811/mcp`, header `Authorization: Bearer TOKEN`, no `Origin` header.
- stdio: run `locus-cli mcp --doc <file> --scopes …`.

## Tools

Units are millimetres; **angles in tool arguments and results are degrees**. Points are entities, and lines, circles and arcs refer to point IDs, so a shared corner is a shared point.

| Tool | Scope | What it does |
|---|---|---|
| `get_document_summary` | read | Layers, parameters, entity and constraint counts, DOF, solve status |
| `list_entities`, `get_entity` | read | Geometry, filtered by layer, kind and an `[x0,y0,x1,y1]` box; what references an entity |
| `list_constraints`, `get_dof_report`, `get_conflicts` | read | Constraints, remaining freedom, conflicting/redundant groups with IDs |
| `render_preview` | read | A PNG of the document or a region (blue: can still move; black: fully constrained; purple dashed: construction; shaded: closed region) |
| `validate_export` | read | Open cut paths, self-intersections, duplicates, with entity IDs |
| `list_plugins` | read | Installed plugins with parameter schemas |
| `add_entities` | write | New geometry, optionally with constraints, in one step. Items can be `name`d and referred to later in the same call; a point can be given as `[x, y]` |
| `modify_entity`, `delete_entities` | write | Change geometry or the construction flag; delete (optionally with everything built on it) |
| `add_constraints`, `remove_constraints`, `set_dimension` | write | Constraints and dimensions (`value`, or an `expr` over parameters; `driven` for reference dimensions) |
| `set_parameter` | write | Document parameters (`"thickness * 2"`) |
| `create_layer`, `set_layer_props`, `move_to_layer` | write | Layers and their machine operation |
| `run_generator`, `apply_modifier` | write | Installed plugins |
| `save` | write | Headless only: save to the `--doc` file |
| `export` | export | `svg`, `pdf`, `dxf` or a plugin format; returns the file |

Resources: `locus://document` (the `.locus` text), `locus://layers`, `locus://plugins`.

Example `add_entities` call: a 40 × 20 rectangle with its constraints, in one undo step:

```json
{
  "entities": [
    {"name": "a", "type": "point", "x": 0, "y": 0},
    {"name": "b", "type": "point", "x": 40, "y": 0},
    {"name": "c", "type": "point", "x": 40, "y": 20},
    {"name": "d", "type": "point", "x": 0, "y": 20},
    {"name": "ab", "type": "line", "start": "a", "end": "b"},
    {"name": "bc", "type": "line", "start": "b", "end": "c"},
    {"name": "cd", "type": "line", "start": "c", "end": "d"},
    {"name": "da", "type": "line", "start": "d", "end": "a"}
  ],
  "constraints": [
    {"type": "fix", "point": "a"},
    {"type": "horizontal", "line": "ab"},
    {"type": "vertical", "line": "bc"},
    {"type": "horizontal", "line": "cd"},
    {"type": "vertical", "line": "da"},
    {"type": "distance", "a": "a", "b": "b", "value": 40},
    {"type": "distance", "a": "b", "b": "c", "value": 20}
  ]
}
```

Errors come back as data the AI can act on, e.g. a dimension that contradicts existing ones:

```json
{"error": {"kind": "over_constrained_conflict", "constraints": [18], "entities": [2, 3],
           "message": "conflicts with 1 existing constraint(s)", "hint": "…"}}
```

## Security model (SPEC §16.4)

- **Off by default**, turned on per install, with the status indicator always visible while it runs.
- **127.0.0.1 only.** Any other address, including `0.0.0.0` and `::1`, is refused before a socket opens.
- **Bearer token** per server session (256 random bits, compared in constant time), shown only in the app.
- **No browsers.** Any request with an `Origin` header is refused, and the `Host` must be `127.0.0.1` or `localhost` (plus `host.docker.internal` only while **Allow Docker clients** is on); this blocks DNS rebinding.
- **Scopes** read / write / export are separate; read only by default.
- **Approval modes**: ask each write, ask once per session, or allow. Export always asks unless you allow it.
- **One labelled undo step per AI action.** A refused or failed call leaves the document and the undo history untouched.
- **Limits** on writes: 30 write or export calls per 10 seconds, arguments up to 256 KiB, up to 2 000 items per call, plus the document's own limits (e.g. 10 000 pattern instances).
- **No file access** for the AI: export returns bytes; the headless `save` writes only the file you named.
- **Plugins can't reach MCP**: plugins are sandboxed and have no way to call it (SPEC §15.4).
- **No telemetry.** Locus logs only tool names and whether they succeeded: never arguments, document contents, file paths or tokens.

What your AI client does with what it reads is up to that client. If it uses a cloud model, your drawing goes to that service.
