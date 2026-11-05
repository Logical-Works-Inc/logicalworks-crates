//! An MCP server and client talk over an in-memory pipe: list, call, refuse.

use lgwks_deps::rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, ListToolsResult,
    PaginatedRequestParams, ServerCapabilities, ServerConfig, Tool, object,
};
use lgwks_deps::rmcp::service::{RequestContext, RoleServer};
use lgwks_deps::rmcp::{ErrorData, ServerHandler, ServiceExt};
use lgwks_std::json::json;

/// The pipe between the two ends: a bound, so a stalled reader backpressures.
const PIPE_BYTES: usize = 64 * 1_024;

/// A server with one tool, `add`, that sums two integers.
struct Adder;

/// The server's refusal of a call, logged with the tool it named.
fn refuse_call(tool: &str, reason: &'static str) -> Result<CallToolResponse, ErrorData> {
    lgwks_std::trace::debug!(tool, reason, "mcp server: refusing a tool call");
    Err(ErrorData::invalid_params(reason, None))
}

impl ServerHandler for Adder {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        let schema = object(
            json!({
                "type": "object",
                "properties": {"left": {"type": "integer"}, "right": {"type": "integer"}},
                "required": ["left", "right"]
            })
            .into(),
        );
        Ok(ListToolsResult::with_all_items(vec![Tool::new(
            "add",
            "Sum two integers",
            schema,
        )]))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        if request.name != "add" {
            return refuse_call(&request.name, "no such tool");
        }
        let Some(arguments) = request.arguments else {
            return refuse_call(&request.name, "arguments are required");
        };
        let left = arguments.get("left").and_then(|value| value.as_i64());
        let right = arguments.get("right").and_then(|value| value.as_i64());
        match (left, right) {
            (Some(left), Some(right)) => {
                Ok(CallToolResponse::Complete(CallToolResult::success(vec![
                    ContentBlock::text(left.saturating_add(right).to_string()),
                ])))
            }
            _ => refuse_call(&request.name, "left and right are integers"),
        }
    }
}

/// Why the journey stopped: the step, and what refused it.
#[derive(Debug)]
struct Stop {
    step: &'static str,
    detail: String,
}

/// A failed step, carrying the error it failed on.
fn stopped(step: &'static str, error: impl std::fmt::Display) -> Stop {
    Stop {
        step,
        detail: error.to_string(),
    }
}

/// Ok when `holds`; otherwise a logged stop at `step`.
fn require(holds: bool, step: &'static str) -> Result<(), Stop> {
    if holds {
        Ok(())
    } else {
        lgwks_std::trace::debug!(step, "mcp journey: an answer broke the contract");
        Err(stopped(step, "the answer broke the contract"))
    }
}

fn main() {
    if let Err(stop) = run() {
        lgwks_std::trace::error!(step = stop.step, detail = %stop.detail, "the MCP journey stopped");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Stop> {
    let runtime =
        lgwks_bot::rt::runtime::Runtime::new().map_err(|error| stopped("runtime", error))?;
    runtime.block_on(journey())
}

async fn journey() -> Result<(), Stop> {
    let (server_io, client_io) = lgwks_bot::rt::io::duplex(PIPE_BYTES);
    let server = async {
        Adder
            .serve(server_io)
            .await
            .map_err(|error| stopped("server handshake", error))
    };
    let client =
        async { ().serve(client_io).await.map_err(|error| stopped("client handshake", error)) };
    let (server, client) = lgwks_bot::try_join!(server, client)?;
    let tools = client
        .list_tools(None)
        .await
        .map_err(|error| stopped("tools/list", error))?;
    let named: Vec<&str> = tools.tools.iter().map(|tool| tool.name.as_ref()).collect();
    require(named == ["add"], "tools/list names")?;
    let arguments = object(json!({"left": 40, "right": 2}).into());
    let answer = client
        .call_tool(CallToolRequestParams::new("add").with_arguments(arguments))
        .await
        .map_err(|error| stopped("tools/call add", error))?;
    let text = answer
        .content
        .first()
        .and_then(|content| content.as_text())
        .map(|text| text.text.as_str());
    require(text == Some("42"), "tools/call add result")?;
    let unknown = client
        .call_tool(CallToolRequestParams::new("subtract"))
        .await;
    require(unknown.is_err(), "an unknown tool was answered")?;
    let bare = client.call_tool(CallToolRequestParams::new("add")).await;
    require(bare.is_err(), "a call with no arguments was answered")?;
    client
        .cancel()
        .await
        .map_err(|error| stopped("client cancel", error))?;
    server
        .cancel()
        .await
        .map_err(|error| stopped("server cancel", error))?;
    Ok(())
}
