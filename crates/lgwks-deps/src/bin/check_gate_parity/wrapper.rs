//! Rule 10: every toolchain the hosted action installs carries the wrapper the
//! repository's cargo config runs.
//!
//! `.cargo/config.toml` sets `rustc-workspace-wrapper = "clippy-driver"`, so every
//! build of a workspace member runs through clippy's driver. A toolchain
//! installed without the `clippy` component has no driver, and cargo stops
//! before it compiles anything: on 1.100.0 under `-q` it exited 1 and printed
//! nothing. That failed every Pages run on main from 227b0738 (#1090), whose
//! stable job passed no components, while every nightly job happened to.
//!
//! So the action's `Pin Rust` step names the component first, as a literal no
//! caller's `components` input can replace or drop. The cases are in
//! `sim_pins.rs`, beside the pin rule this one completes.

use std::path::Path;

use crate::{ParityError, read};

/// The cargo config, relative to the repository root.
pub(crate) const CONFIG_REL: &str = ".cargo/config.toml";

/// The composite action every workflow installs its toolchain through.
pub(crate) const ACTION_REL: &str = ".github/actions/hosted-rust/action.yml";

/// The cargo config key that names the workspace wrapper.
const WRAPPER_KEY: &str = "rustc-workspace-wrapper";

/// The step of the action that installs the toolchain.
const PIN_STEP: &str = "- name: Pin Rust";

/// The wrapper `config` names, unquoted, when it names one.
fn configured_wrapper(config: &str) -> Option<&str> {
    config.lines().find_map(|line| {
        let (key, value) = line.split_once('=')?;
        if key.trim() != WRAPPER_KEY {
            return None;
        }
        let value = value.trim().trim_matches(['"', '\'']);
        (!value.is_empty()).then_some(value)
    })
}

/// The rustup component that ships `wrapper`. Any other wrapper is the host's
/// to install, so no component is owed for it.
fn component_for(wrapper: &str) -> Option<&'static str> {
    (wrapper == "clippy-driver").then_some("clippy")
}

/// The `components:` value of the action's `Pin Rust` step, when it has one.
/// The step ends at the next list item, so a later step's value never counts.
fn pinned_components(action: &str) -> Option<&str> {
    let (_, after) = action.split_once(PIN_STEP)?;
    after
        .lines()
        .skip(1)
        .take_while(|line| !line.trim_start().starts_with("- "))
        .find_map(|line| line.trim_start().strip_prefix("components:"))
        .map(str::trim)
}

/// Whether `value` installs `component` whatever a caller passes: the value
/// opens with the literal, ended by the value, a comma or an expression.
fn always_installs(value: &str, component: &str) -> bool {
    value
        .strip_prefix(component)
        .is_some_and(|rest| rest.is_empty() || rest.starts_with(',') || rest.starts_with("${{"))
}

/// The rule-10 violations of the config and action committed under `root`.
pub(crate) fn committed_problems(root: &Path) -> Result<Vec<String>, ParityError> {
    Ok(problems(
        &read(&root.join(CONFIG_REL))?,
        &read(&root.join(ACTION_REL))?,
    ))
}

/// The rule-10 violations of `config` and `action`. Empty means every toolchain
/// the action installs runs the configured wrapper.
pub(crate) fn problems(config: &str, action: &str) -> Vec<String> {
    let Some(wrapper) = configured_wrapper(config) else {
        return Vec::new();
    };
    let Some(component) = component_for(wrapper) else {
        return Vec::new();
    };
    match pinned_components(action) {
        Some(value) if always_installs(value, component) => Vec::new(),
        Some(value) => vec![format!(
            "{CONFIG_REL} wraps every build in {wrapper}, but {ACTION_REL} installs \
             components {value:?}, which a caller can leave without {component}; \
             open the value with the literal {component}"
        )],
        None => vec![format!(
            "{CONFIG_REL} wraps every build in {wrapper}, but the `Pin Rust` step of \
             {ACTION_REL} names no components, so a toolchain lands without {component}"
        )],
    }
}
