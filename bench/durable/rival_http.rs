//! The HTTP the driver speaks to Restate: submit a run, grant its approval, wait
//! for its end, register the worker, and ask whether the server is up.
//!
//! Every call goes through `lgwks_std::http`, under a deadline. A status the rig
//! does not want is a typed refusal that keeps the start of the body, never a
//! retry in disguise: the only retries live in [`until`], which counts them.

use std::time::Duration;

use lgwks_std::http::{Client, Options, Response};
use lgwks_std::json::Value;

use crate::error::BenchError;
use crate::rival_config::Ports;
use crate::rival_error::RivalError;

/// The workflow's service name: the struct the worker serves.
const WORKFLOW: &str = "DurableFlow";

/// The most bytes of a body an error keeps.
const BODY_KEPT: usize = 200;

/// The longest any one exchange may take, redirects and body included. A run is
/// attached for as long as it takes to finish, so this is the run's bound.
const DEADLINE: Duration = Duration::from_secs(120);

/// How a run that was attached to ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Attached {
    /// It returned this value.
    Done(u64),
    /// The handler ended with a terminal failure.
    Failed {
        /// The failure's own code.
        code: u16,
        /// Its message.
        message: String,
    },
}

/// The start of `body`, for an error line.
fn kept(body: &[u8]) -> String {
    String::from_utf8_lossy(body)
        .chars()
        .take(BODY_KEPT)
        .collect()
}

/// Read an attach answer: the value of a finished run, or the terminal failure
/// the run ended with.
///
/// # Errors
///
/// [`RivalError::AttachMalformed`] for an answer that is neither, and
/// [`RivalError::Status`] for an error that is not the run's own, such as an
/// unknown workflow.
pub(crate) fn parse_attach(status: u16, body: &[u8]) -> Result<Attached, RivalError> {
    let malformed = || RivalError::AttachMalformed {
        status,
        body: kept(body),
    };
    let Ok(text) = std::str::from_utf8(body) else {
        lgwks_std::trace::debug!(status, "an attach body is not UTF-8");
        return Err(malformed());
    };
    if status == 200 {
        return text
            .trim()
            .parse::<u64>()
            .map(Attached::Done)
            .map_err(|source| {
                lgwks_std::trace::debug!(%source, "a finished run's value is not a number");
                malformed()
            });
    }
    if !(400..600).contains(&status) {
        lgwks_std::trace::debug!(status, "an attach status is neither success nor failure");
        return Err(malformed());
    }
    let value = lgwks_std::json::from_str::<Value>(text).map_err(|source| {
        lgwks_std::trace::debug!(%source, "an attach failure body is not JSON");
        malformed()
    })?;
    let (Some(code), Some(message)) = (value["code"].as_u64(), value["message"].as_str()) else {
        lgwks_std::trace::debug!(status, "an attach failure has no code and message");
        return Err(malformed());
    };
    if value["source"].as_str() != Some("invocation") || code != u64::from(status) || status == 404
    {
        lgwks_std::trace::debug!(status, "an attach failure is not the run's own");
        return Err(RivalError::Status {
            what: "attaching to a run",
            status,
            body: kept(body),
        });
    }
    Ok(Attached::Failed {
        code: status,
        message: message.to_owned(),
    })
}

/// The server's two doors, and a client to knock on them.
pub(crate) struct Wire {
    /// One client for every exchange.
    client: Client,
    /// Where runs are submitted and attached to.
    ingress: String,
    /// Where deployments are registered and health is read.
    admin: String,
    /// What the worker's endpoint is called when it is registered.
    worker: String,
}

impl Wire {
    /// A wire to the server and worker at `ports`.
    ///
    /// # Errors
    ///
    /// [`RivalError::Http`] when the client cannot be built.
    pub(crate) fn new(ports: &Ports) -> Result<Self, BenchError> {
        let client = Client::new(Options::default().deadline(DEADLINE)).map_err(|error| {
            BenchError::Rival {
                source: RivalError::Http {
                    what: "building the client",
                    detail: error.to_string(),
                },
            }
        })?;
        Ok(Self {
            client,
            ingress: format!("http://{}", Ports::address(ports.ingress)),
            admin: format!("http://{}", Ports::address(ports.admin)),
            worker: format!("http://{}", Ports::address(ports.worker)),
        })
    }

    /// One exchange: `body` posted as JSON, or a GET when there is none.
    fn exchange(
        &self,
        what: &'static str,
        url: &str,
        body: Option<&str>,
    ) -> Result<Response, RivalError> {
        let options = self.client.options();
        let sent = match body {
            Some(json) => self
                .client
                .post_with(url, "application/json", json.as_bytes(), options),
            None => self.client.get_with(url, options),
        };
        sent.map_err(|error| {
            lgwks_std::trace::debug!(%error, what, "an exchange with the server failed");
            RivalError::Http {
                what,
                detail: error.to_string(),
            }
        })
    }

    /// An exchange that must answer one of `want`.
    fn demand(
        &self,
        what: &'static str,
        url: &str,
        body: Option<&str>,
        want: &[u16],
    ) -> Result<(), RivalError> {
        let answer = self.exchange(what, url, body)?;
        if want.contains(&answer.status) {
            return Ok(());
        }
        lgwks_std::trace::debug!(what, status = answer.status, "an unwanted status");
        Err(RivalError::Status {
            what,
            status: answer.status,
            body: kept(answer.body()),
        })
    }

    /// Register the worker's endpoint with the server.
    ///
    /// # Errors
    ///
    /// [`RivalError::Http`] and [`RivalError::Status`].
    pub(crate) fn register(&self) -> Result<(), RivalError> {
        let url = format!("{}/deployments", self.admin);
        let body = format!("{{\"uri\": \"{}\"}}", self.worker);
        self.demand("registering the worker", &url, Some(&body), &[200, 201])
    }

    /// Whether the admin API and the ingress both answer 200 on their health
    /// paths: `Ok(false)` while a door answers another status, and the typed
    /// cause when a door cannot be reached, so a bounded wait that runs out
    /// reports why.
    ///
    /// # Errors
    ///
    /// [`RivalError::Http`] when either door cannot be reached.
    pub(crate) fn healthy(&self) -> Result<bool, RivalError> {
        for (base, path) in [(&self.admin, "/health"), (&self.ingress, "/restate/health")] {
            let answer = self.exchange("reading health", &format!("{base}{path}"), None)?;
            if answer.status != 200 {
                lgwks_std::trace::debug!(base = base.as_str(), path, status = answer.status, "a door is not healthy yet");
                return Ok(false);
            }
        }
        Ok(true)
    }

    /// Submit run `ordinal` as workflow `key`.
    ///
    /// # Errors
    ///
    /// [`RivalError::Http`] and [`RivalError::Status`].
    pub(crate) fn submit(&self, key: &str, ordinal: u32) -> Result<(), RivalError> {
        let url = format!("{}/{WORKFLOW}/{key}/run/send", self.ingress);
        self.demand("submitting a run", &url, Some(&ordinal.to_string()), &[200, 202])
    }

    /// Grant the approval of workflow `key`, with the run's number as the payload.
    ///
    /// # Errors
    ///
    /// [`RivalError::Http`] and [`RivalError::Status`].
    pub(crate) fn approve(&self, key: &str, payload: u32) -> Result<(), RivalError> {
        let url = format!("{}/{WORKFLOW}/{key}/approve", self.ingress);
        self.demand("granting an approval", &url, Some(&payload.to_string()), &[200])
    }

    /// Wait for workflow `key` to end, and read how.
    ///
    /// # Errors
    ///
    /// [`RivalError::Http`], and everything [`parse_attach`] raises.
    pub(crate) fn attach(&self, key: &str) -> Result<Attached, RivalError> {
        let url = format!("{}/restate/workflow/{WORKFLOW}/{key}/attach", self.ingress);
        let answer = self.exchange("attaching to a run", &url, None)?;
        parse_attach(answer.status, answer.body())
    }
}

/// Run `probe` until it says yes, at most `tries` times, calling `pause` with
/// `gap` between two tries. The bound is a count, so it holds on any clock.
///
/// A try that fails keeps its cause and the wait goes on; when the bound runs out
/// the last cause is what is returned.
///
/// # Errors
///
/// The last try's own error when it failed, else [`RivalError::NotReady`] when
/// every try answered no.
pub(crate) fn until(
    what: &'static str,
    tries: u32,
    gap: Duration,
    pause: &dyn Fn(Duration),
    mut probe: impl FnMut() -> Result<bool, RivalError>,
) -> Result<(), RivalError> {
    let mut last = None;
    for attempt in 0..tries {
        match probe() {
            Ok(true) => return Ok(()),
            Ok(false) => {}
            Err(error) => last = Some(error),
        }
        if attempt + 1 < tries {
            pause(gap);
        }
    }
    lgwks_std::trace::debug!(what, tries, "a process never became ready");
    match last {
        Some(cause) => Err(cause),
        None => Err(RivalError::NotReady { what, tries }),
    }
}
