// SPDX-License-Identifier: AGPL-3.0-or-later

//! The stdio transport (SPEC §16.2): `locus-cli mcp --doc file.locus`.
//! stdout carries only protocol messages; nothing else is printed there.

use crate::server::LocusServer;

/// Serve `server` on stdin/stdout until the client closes the stream.
///
/// # Errors
/// The transport failed to start or broke.
pub async fn serve(server: LocusServer) -> Result<(), String> {
    use rmcp::ServiceExt as _;
    let running = server
        .serve(rmcp::transport::stdio())
        .await
        .map_err(|e| format!("MCP session failed to start: {e}"))?;
    running
        .waiting()
        .await
        .map(|_| ())
        .map_err(|e| format!("MCP session ended with an error: {e}"))
}
