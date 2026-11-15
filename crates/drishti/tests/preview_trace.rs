//! Local preview verification through the shipped binaries.
//!
//! Counterpart of the executable half of `tools/verify_local.py` (kernel#850
//! row 15): headless normal and reduced traces check out, invalid UTF-8 is
//! refused with exit 1 and a `canvas-preview:` line, and FIFOs are refused
//! before any blocking open. Dependency-tree and formatting gates live in CI,
//! not here.

use std::path::{Path, PathBuf};
use std::process::Command;

/// Preview binary under test.
fn preview_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_canvas_preview"))
}

/// Run a closure inside a fresh process-unique temp dir, removed after.
fn with_temp_dir(name: &str, run: impl FnOnce(&Path) -> Result<(), String>) -> Result<(), String> {
    let dir =
        std::env::temp_dir().join(format!("drishti-{name}-{}", lgwks_std::id::Uuid::now_v7()));
    std::fs::create_dir_all(&dir).map_err(|error| format!("temp setup: {error}"))?;
    let outcome = run(&dir);
    let cleaned = std::fs::remove_dir_all(&dir).map_err(|error| format!("temp cleanup: {error}"));
    outcome.and(cleaned)
}

/// Write bytes to a temp file and return its path.
fn write_input(dir: &Path, name: &str, bytes: &[u8]) -> Result<PathBuf, String> {
    let path = dir.join(name);
    std::fs::write(&path, bytes).map_err(|error| format!("input unwritable: {error}"))?;
    Ok(path)
}

/// Run the preview headless and return its output.
fn run_headless(source: &Path, reduced: bool) -> Result<std::process::Output, String> {
    let mut command = Command::new(preview_bin());
    command.arg("--headless").arg("--source").arg(source);
    if reduced {
        command.arg("--reduced-motion");
    }
    command
        .output()
        .map_err(|error| format!("preview failed to run: {error}"))
}

/// Headless traces check out in both motion modes.
#[test]
fn headless_traces_check_out() -> Result<(), String> {
    with_temp_dir("traces", |dir| {
        let source = write_input(
            dir,
            "source.txt",
            b"hello canvas\nsecond line\nthird line\n",
        )?;
        let source_len = source.metadata().map_err(|error| format!("{error}"))?.len();
        for reduced in [false, true] {
            let output = run_headless(&source, reduced)?;
            if !output.status.success() {
                return Err(format!(
                    "headless run failed: {}",
                    String::from_utf8_lossy(&output.stderr)
                ));
            }
            let text =
                String::from_utf8(output.stdout).map_err(|_| String::from("trace is not UTF-8"))?;
            let mut records = Vec::new();
            for line in text.lines() {
                let record: lgwks_std::json::Value = lgwks_std::json::from_str(line)
                    .map_err(|error| format!("bad JSONL: {error}"))?;
                records.push(record);
            }
            let frames = drishti::preview::check_records(&records, source_len, reduced)
                .map_err(|error| format!("trace rejected: {error}"))?;
            if frames < 4 {
                return Err(String::from("trace must hold frames"));
            }
        }
        Ok(())
    })
}

/// Invalid UTF-8 is refused with exit 1 and an application error line.
#[test]
fn invalid_utf8_is_refused() -> Result<(), String> {
    with_temp_dir("utf8", |dir| {
        let bad = write_input(dir, "bad.txt", &[0xff])?;
        let output = run_headless(&bad, false)?;
        if output.status.code() != Some(1) {
            return Err(String::from("invalid UTF-8 must exit 1"));
        }
        let stderr = String::from_utf8_lossy(&output.stderr);
        let refused = stderr.lines().any(|line| {
            line.starts_with("canvas-preview:") && line.to_lowercase().contains("utf-8")
        });
        if !refused {
            return Err(format!("must print an application refusal, got: {stderr}"));
        }
        Ok(())
    })
}

/// FIFOs are refused before any blocking open, with exit 1.
#[test]
fn fifo_is_refused_without_blocking() -> Result<(), String> {
    with_temp_dir("fifo", |dir| {
        let fifo = dir.join("input-fifo");
        if fifo.exists() {
            std::fs::remove_file(&fifo).map_err(|error| format!("{error}"))?;
        }
        let created = Command::new("mkfifo")
            .arg(&fifo)
            .output()
            .map_err(|error| format!("mkfifo missing: {error}"))?;
        if !created.status.success() {
            return Err(String::from("mkfifo must succeed on this host"));
        }
        let output = run_headless(&fifo, false)?;
        if output.status.code() != Some(1) {
            return Err(String::from("FIFO must exit 1"));
        }
        let stderr = String::from_utf8_lossy(&output.stderr);
        if !stderr.contains("canvas-preview: input must be a regular file") {
            return Err(format!("FIFO must be refused promptly, got: {stderr}"));
        }
        Ok(())
    })
}
