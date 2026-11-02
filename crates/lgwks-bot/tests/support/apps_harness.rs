//! Shared fixture for the model-written plan targets (`apps` and `sim_apps`,
//! #786): the demo handlers and the registry both paths admit against, so the
//! HTTP tests and the simulations run one set of operations.

use lgwks_bot::cap::Cap;
use lgwks_bot::rt::apps::OpRegistry;
use lgwks_bot::script::{FlowError, Scope};

/// Fetch and return the input uppercased.
pub async fn fetch_step(_scope: Scope, input: Vec<u8>) -> Result<Vec<u8>, FlowError> {
    Ok(input.to_ascii_uppercase())
}

/// Reverse the input.
pub async fn render_step(_scope: Scope, input: Vec<u8>) -> Result<Vec<u8>, FlowError> {
    let mut out = input;
    out.reverse();
    Ok(out)
}

/// Mark the input reviewed.
pub async fn preview_step(_scope: Scope, input: Vec<u8>) -> Result<Vec<u8>, FlowError> {
    let mut out = input;
    out.extend_from_slice(b"?");
    Ok(out)
}

/// The demo registry: `fetch` and `render` free, `preview` needing `bot.net`.
pub fn demo_registry() -> Result<OpRegistry, String> {
    let registry = OpRegistry::new()
        .operation("fetch", &[], fetch_step)
        .map_err(|error| format!("the demo registry is refused: {error}"))?;
    let registry = registry
        .operation("render", &[], render_step)
        .map_err(|error| format!("the demo registry is refused: {error}"))?;
    let registry = registry
        .operation("preview", &[Cap::net()], preview_step)
        .map_err(|error| format!("the demo registry is refused: {error}"))?;
    Ok(registry)
}
