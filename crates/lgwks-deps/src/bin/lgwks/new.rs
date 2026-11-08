//! `lgwks new <name>`: scaffold one app directory with one app file.
//!
//! The scaffold is two files: `<name>/Cargo.toml`, pinning the `lgwks_bot`
//! version and the features a serving app needs with a `[workspace]` table so
//! the app stands alone, and `<name>/app.rs`, holding one `script!` block for
//! a serving app. Nothing else is generated, and the user edits no generated
//! file other than `app.rs`. An existing non-empty directory is refused
//! rather than clobbered.

use std::io::Write;
use std::process::ExitCode;

use super::common;

/// The command's own usage.
const NEW_USAGE: &str = "\
lgwks new <name> [--json]

Scaffold <name>/Cargo.toml and <name>/app.rs: one serving app in one file.
<name> holds letters, digits, `_` and `-`. An existing non-empty directory
is refused rather than clobbered.
";

/// The `lgwks_bot` release the scaffold pins, kept beside the workspace's.
const BOT_VERSION: &str = "8.0.1";

/// The `lgwks_std` release the scaffold pins, kept beside the workspace's.
const STD_VERSION: &str = "4.1.0";

/// The marker the manifest template names the app with.
const NAME_MARK: &str = "@@NAME@@";

/// The marker the manifest template names the bot release with.
const BOT_MARK: &str = "@@BOT@@";

/// The marker the manifest template names the std release with.
const STD_MARK: &str = "@@STD@@";

/// What `<name>/Cargo.toml` holds, with markers for the app name and the
/// pinned estate releases.
const MANIFEST_TEMPLATE: &str = "\
[package]
name = \"@@NAME@@\"
version = \"0.1.0\"
edition = \"2024\"

[workspace]

[[bin]]
name = \"@@NAME@@\"
path = \"app.rs\"

[dependencies]
lgwks_bot = { version = \"=@@BOT@@\", default-features = false, features = [\"script\", \"edge\"] }
lgwks_std = { version = \"=@@STD@@\", default-features = false, features = [\"json\"] }
";

/// What `<name>/app.rs` holds, with a marker for the file's own name.
const APP_TEMPLATE: &str = "\
//! @@NAME@@: one-file serving app.
//!
//! Run it with `lgwks run` from this directory, or with `cargo run` directly.
//! The orchestration is one `script!` block; edit only this file.

use std::io::Write as _;

use lgwks_bot::script::{Scope, Tenant};

lgwks_bot::script! {
    /// Serve one echo route until the scope stops.
    pub flow drafts(listen_on: u16) -> u16:
        let served = serve on port listen_on, at most 10 runs in flight:
            route POST /drafts (body: Vec<u8>) -> Vec<u8>:
                give back body
        give back served.port()
}

/// Serve on `PORT`, or 9000 when it names nothing usable.
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let port = port_from_env();
    let scope = Scope::root(Tenant::new(\"demo\")?);
    let served = lgwks_bot::block_on(drafts(&scope, port))?;
    writeln!(std::io::stdout().lock(), \"served on {served}\")?;
    Ok(())
}

/// The port the app binds: `PORT` when it parses, 9000 otherwise.
fn port_from_env() -> u16 {
    match std::env::var(\"PORT\") {
        Ok(text) => match text.trim().parse() {
            Ok(port) => port,
            Err(_) => 9000,
        },
        Err(_) => 9000,
    }
}
";

/// Whether `name` is a usable app and directory name.
fn valid_name(name: &str) -> bool {
    if name.is_empty() {
        return false;
    }
    let mut chars = name.chars();
    match chars.next() {
        Some(first) if first.is_ascii_alphanumeric() || first == '_' => {}
        Some(_) | None => return false,
    }
    chars.all(|cell| cell.is_ascii_alphanumeric() || cell == '_' || cell == '-')
}

/// The scaffolded files with the app name written in.
fn render(name: &str) -> (String, String) {
    // The manifest markers name no real TOML or Rust, so `replace` writes
    // the values in without disturbing any code the files hold.
    let manifest = MANIFEST_TEMPLATE
        .replace(NAME_MARK, name)
        .replace(BOT_MARK, BOT_VERSION)
        .replace(STD_MARK, STD_VERSION);
    let app = APP_TEMPLATE.replace(NAME_MARK, name);
    (manifest, app)
}

/// Scaffold the app `name` names, and report what was written.
pub(crate) fn run(
    args: &[String],
    json: bool,
    out: &mut impl Write,
    err: &mut impl Write,
) -> std::io::Result<ExitCode> {
    let name = match common::one_argument("new", NEW_USAGE, "name", args, out, err)? {
        Ok(Some(name)) => name,
        Ok(None) => {
            lgwks_std::trace::debug!("lgwks new: refusing a missing name");
            writeln!(err, "lgwks new: a name is required")?;
            return write!(err, "{NEW_USAGE}").map(|()| ExitCode::from(common::EXIT_REFUSED));
        }
        Err(code) => return Ok(code),
    };
    if !valid_name(name) {
        let why = format!("{name} is not an app name: use letters, digits, `_` and `-`");
        return common::fail(why, json, out, err);
    }
    let dir = std::path::PathBuf::from(name);
    if dir.exists() {
        let occupied = match std::fs::read_dir(&dir) {
            Ok(entries) => entries.count() > 0,
            Err(_) => true,
        };
        if occupied {
            let why =
                format!("the directory {name} exists and is not empty; refusing to clobber it");
            return common::fail(why, json, out, err);
        }
    }
    if let Err(error) = std::fs::create_dir_all(&dir) {
        let why = format!("the directory {name} could not be created: {error}");
        return common::fail(why, json, out, err);
    }
    let (manifest, app) = render(name);
    let manifest_path = dir.join("Cargo.toml");
    let app_path = dir.join("app.rs");
    for (path, text) in [
        (&manifest_path, manifest.as_str()),
        (&app_path, app.as_str()),
    ] {
        if let Err(error) = std::fs::write(path, text) {
            let why = format!("{} could not be written: {error}", path.display());
            return common::fail(why, json, out, err);
        }
    }
    if json {
        let value = lgwks_std::json::json!({
            "ok": true,
            "dir": name,
            "files": ["Cargo.toml", "app.rs"],
        });
        common::json_line(&value, out).map(|()| ExitCode::SUCCESS)
    } else {
        writeln!(out, "created {name}/Cargo.toml and {name}/app.rs").map(|()| ExitCode::SUCCESS)
    }
}
