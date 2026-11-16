//! The Restate server's config file and the durability check on it.
//!
//! The driver writes the config (every bind address a port picked by binding, the
//! base directory inside the work directory), then asks the server itself what it
//! would run with: `restate-server --dump-config`. That text is read for the one
//! thing the comparison stands on. If the write-ahead log or its fsync is off for
//! the log provider in use, the row is refused: a rival that does not flush is
//! not measured against an engine that does.

use std::collections::BTreeMap;
use std::net::{Ipv4Addr, SocketAddr, TcpListener};
use std::path::Path;
use std::process::Command;

use crate::error::BenchError;
use crate::rival_error::RivalError;

/// The loopback ports one Restate stack uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Ports {
    /// Node to node, which the server binds even when it is alone.
    pub(crate) node: u16,
    /// The HTTP ingress runs are submitted through.
    pub(crate) ingress: u16,
    /// The admin API deployments are registered through.
    pub(crate) admin: u16,
    /// The worker's endpoint.
    pub(crate) worker: u16,
}

impl Ports {
    /// The loopback address of `port`.
    pub(crate) fn address(port: u16) -> SocketAddr {
        SocketAddr::from((Ipv4Addr::LOCALHOST, port))
    }
}

/// Four free loopback ports, picked by binding port 0 four times at once, so the
/// four are distinct, then releasing them.
///
/// # Errors
///
/// [`RivalError::PortBind`] when a bind or a read of the bound address fails.
pub(crate) fn pick_ports() -> Result<Ports, BenchError> {
    let bind = || TcpListener::bind((Ipv4Addr::LOCALHOST, 0));
    let held = [bind(), bind(), bind(), bind()];
    let mut ports = Vec::new();
    for listener in held {
        let port = listener
            .and_then(|bound| bound.local_addr())
            .map(|address| address.port())
            .map_err(|source| BenchError::Rival {
                source: RivalError::PortBind { source },
            })?;
        ports.push(port);
    }
    match ports[..] {
        [node, ingress, admin, worker] => Ok(Ports {
            node,
            ingress,
            admin,
            worker,
        }),
        _ => {
            lgwks_std::trace::debug!(picked = ports.len(), "not four ports were picked");
            Err(BenchError::Rival {
                source: RivalError::PortBind {
                    source: std::io::Error::other("not four ports were picked"),
                },
            })
        }
    }
}

/// The config file for a server that keeps its data under `base` and binds `ports`.
///
/// Paths go in as TOML literal strings, which have no escapes: a path holding a
/// quote or a line break is refused, so a hostile directory name cannot add a key.
///
/// # Errors
///
/// [`RivalError::ConfigPath`] when `base` is not UTF-8 or cannot be one literal.
pub(crate) fn config_text(base: &Path, ports: &Ports) -> Result<String, RivalError> {
    let refused = || RivalError::ConfigPath {
        path: base.to_path_buf(),
    };
    let Some(text) = base.to_str() else {
        lgwks_std::trace::debug!(path = ?base, "the base directory is not UTF-8");
        return Err(refused());
    };
    if text.contains('\'') || text.chars().any(char::is_control) {
        lgwks_std::trace::debug!(path = ?base, "the base directory cannot be one TOML literal");
        return Err(refused());
    }
    let (node, ingress, admin) = (
        Ports::address(ports.node),
        Ports::address(ports.ingress),
        Ports::address(ports.admin),
    );
    Ok(format!(
        "base-dir = '{text}'\n\
         bind-address = \"{node}\"\n\
         advertised-address = \"http://{node}/\"\n\
         disable-telemetry = true\n\
         log-filter = \"warn\"\n\
         [ingress]\n\
         bind-address = \"{ingress}\"\n\
         [admin]\n\
         bind-address = \"{admin}\"\n"
    ))
}

/// A config dump read into (section, key) pairs. The values are text, with
/// their quotes still on.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Dump {
    /// Every pair, by section then key; top-level keys sit under the empty section.
    pairs: BTreeMap<(String, String), String>,
}

/// `value` with a trailing comment cut off. A `#` inside quotes is text.
fn without_comment(value: &str) -> &str {
    let mut quoted = false;
    for (at, character) in value.char_indices() {
        match character {
            '"' => quoted = !quoted,
            '#' if !quoted => return value.split_at(at).0.trim_end(),
            _ => {}
        }
    }
    value.trim_end()
}

/// How many brackets a value opens that it does not close.
fn open_brackets(value: &str) -> i32 {
    let mut quoted = false;
    let mut depth = 0_i32;
    for character in value.chars() {
        match character {
            '"' => quoted = !quoted,
            '[' if !quoted => depth += 1,
            ']' if !quoted => depth -= 1,
            _ => {}
        }
    }
    depth
}

impl Dump {
    /// Read the text of `--dump-config`.
    ///
    /// Blank lines and comments are skipped, `\r\n` is a line end, a multi-line
    /// array is one value, and a section opened twice, a key repeated in a
    /// section and any line that is none of these are refused.
    ///
    /// # Errors
    ///
    /// [`RivalError::DumpMalformed`] and [`RivalError::DumpDuplicate`].
    pub(crate) fn parse(text: &str) -> Result<Self, RivalError> {
        let mut pairs = BTreeMap::new();
        let mut seen: Vec<String> = Vec::new();
        let mut section = String::new();
        let mut open = 0_i32;
        for raw in text.lines() {
            let line = raw.trim();
            if open > 0 {
                open += open_brackets(without_comment(line));
                continue;
            }
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let malformed = || RivalError::DumpMalformed {
                line: line.to_owned(),
            };
            if let Some(inner) = line.strip_prefix('[') {
                let name = without_comment(inner)
                    .strip_suffix(']')
                    .filter(|name| !name.is_empty() && !name.contains(['[', ']', '"', '=']))
                    .ok_or_else(malformed)?;
                if seen.iter().any(|earlier| earlier == name) {
                    lgwks_std::trace::debug!(section = name, "the dump opens a section twice");
                    return Err(RivalError::DumpDuplicate {
                        section: name.to_owned(),
                        key: String::new(),
                    });
                }
                seen.push(name.to_owned());
                name.clone_into(&mut section);
                continue;
            }
            let (key, value) = line.split_once('=').ok_or_else(malformed)?;
            let key = key.trim();
            let value = without_comment(value.trim());
            let bare = |c: char| c.is_ascii_alphanumeric() || c == '-' || c == '_';
            if key.is_empty() || !key.chars().all(bare) || value.is_empty() {
                lgwks_std::trace::debug!(line, "a dump line is not a bare key and a value");
                return Err(malformed());
            }
            open = open_brackets(value).max(0);
            let slot = (section.clone(), key.to_owned());
            if pairs.insert(slot, value.to_owned()).is_some() {
                lgwks_std::trace::debug!(section = section.as_str(), key, "the dump repeats a key");
                return Err(RivalError::DumpDuplicate {
                    section,
                    key: key.to_owned(),
                });
            }
        }
        Ok(Self { pairs })
    }

    /// The value of `key` under `section`, as text.
    pub(crate) fn get(&self, section: &str, key: &str) -> Option<&str> {
        self.pairs
            .get(&(section.to_owned(), key.to_owned()))
            .map(String::as_str)
    }

    /// The boolean `key` under `section`: `Ok(None)` when it is absent.
    fn flag(&self, section: &'static str, key: &'static str) -> Result<Option<bool>, RivalError> {
        match self.get(section, key) {
            None => Ok(None),
            Some("true") => Ok(Some(true)),
            Some("false") => Ok(Some(false)),
            Some(other) => {
                lgwks_std::trace::debug!(key, value = other, "a durability setting is not a boolean");
                Err(RivalError::NotBoolean {
                    key,
                    value: other.to_owned(),
                })
            }
        }
    }
}

/// The durability the server will run with, for the log provider in use.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Durability {
    /// The provider, as the dump spells it.
    provider: String,
    /// The section whose settings decide it.
    section: &'static str,
}

impl Durability {
    /// The setting as the row prints it.
    pub(crate) fn line(&self) -> String {
        format!(
            "wal on, fsync on ({}, provider {})",
            self.section, self.provider
        )
    }
}

/// Read the durability of the active provider out of a config dump, or refuse.
///
/// The replicated provider is decided by `[log-server]`: its fsync key must be
/// there and `false`, and a WAL switch, if the build has one, must be `false`.
/// The local provider is decided by `[bifrost.local]`: both keys must be there
/// and `false`. Any other provider is refused.
///
/// # Errors
///
/// [`RivalError::DumpMissing`], [`RivalError::UnknownProvider`],
/// [`RivalError::NotBoolean`] and [`RivalError::DurabilityOff`].
pub(crate) fn durability(dump: &Dump) -> Result<Durability, RivalError> {
    let provider = dump
        .get("bifrost", "default-provider")
        .and_then(|quoted| quoted.strip_prefix('"')?.strip_suffix('"'))
        .ok_or(RivalError::DumpMissing {
            section: "bifrost",
            key: "default-provider",
        })?;
    let (section, wal_required) = match provider {
        "replicated" => ("log-server", false),
        "local" => ("bifrost.local", true),
        other => {
            lgwks_std::trace::debug!(provider = other, "the log provider is not known");
            return Err(RivalError::UnknownProvider {
                provider: other.to_owned(),
            });
        }
    };
    let off = |key: &'static str| RivalError::DurabilityOff {
        section,
        key,
        value: "true".to_owned(),
    };
    match dump.flag(section, "rocksdb-disable-wal")? {
        Some(true) => {
            lgwks_std::trace::debug!(section, "the write-ahead log is off");
            return Err(off("rocksdb-disable-wal"));
        }
        None if wal_required => {
            lgwks_std::trace::debug!(section, "the write-ahead log switch is missing");
            return Err(RivalError::DumpMissing {
                section,
                key: "rocksdb-disable-wal",
            });
        }
        Some(false) | None => {}
    }
    match dump.flag(section, "rocksdb-disable-wal-fsync")? {
        Some(false) => Ok(Durability {
            provider: provider.to_owned(),
            section,
        }),
        Some(true) => {
            lgwks_std::trace::debug!(section, "the log's fsync is off");
            Err(off("rocksdb-disable-wal-fsync"))
        }
        None => {
            lgwks_std::trace::debug!(section, "the log's fsync switch is missing");
            Err(RivalError::DumpMissing {
                section,
                key: "rocksdb-disable-wal-fsync",
            })
        }
    }
}

/// Ask `server` what it would run with under `config`, and read the durability.
///
/// # Errors
///
/// [`RivalError::Spawn`], [`RivalError::DumpFailed`] and everything
/// [`Dump::parse`] and [`durability`] raise.
pub(crate) fn check_durability(server: &str, config: &Path) -> Result<Durability, BenchError> {
    let output = Command::new(server)
        .arg("--config-file")
        .arg(config)
        .arg("--dump-config")
        .output()
        .map_err(|source| BenchError::Rival {
            source: RivalError::Spawn {
                what: "restate-server --dump-config",
                source,
            },
        })?;
    if !output.status.success() {
        lgwks_std::trace::debug!(status = %output.status, "restate-server --dump-config failed");
        return Err(BenchError::Rival {
            source: RivalError::DumpFailed {
                status: output.status.to_string(),
            },
        });
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let dump = Dump::parse(&text).map_err(|source| BenchError::Rival { source })?;
    durability(&dump).map_err(|source| BenchError::Rival { source })
}
