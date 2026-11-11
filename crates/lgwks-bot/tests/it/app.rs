//! The app SDK served over a real socket (#791).
//!
//! Source: #791's acceptance: the #780 reference app written as one
//! [`App`] serves its routes, an ask's answer reaches the named `on` handler,
//! `check` refuses an app whose handlers need a cap it was not granted, an
//! ungranted effect is refused at dispatch and never reaches the provider, a
//! model-written plan serves through the same check, and every tenant keeps
//! its own state.

use std::fmt;
use std::io::{Read as _, Write as _};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use lgwks_bot::Cap;
use lgwks_bot::app::{App, AppError, Effect, EffectResult, Method, Provider, ReplyTag};
use lgwks_bot::rt::edge::EdgeStop;
use lgwks_std::json::Value;

/// How an app journey broke.
#[derive(Debug)]
pub(crate) enum JourneyFailed {
    /// The socket failed.
    Io(std::io::Error),
    /// The answer was not an HTTP response.
    Malformed(String),
    /// A request answered other than the journey requires.
    Answer {
        /// The request, as `METHOD PATH`.
        request: String,
        /// What the journey requires.
        wanted: (u16, String),
        /// What the app answered.
        found: (u16, String),
    },
    /// The provider saw a different number of effects.
    Provider {
        /// Effects it saw.
        seen: u64,
        /// Effects the journey sent it.
        wanted: u64,
    },
    /// `check` or `from_plan` answered other than the journey requires.
    Verdict(String),
    /// A client thread panicked.
    Panicked,
}

impl fmt::Display for JourneyFailed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::Io(ref cause) => write!(f, "the socket failed: {cause}"),
            Self::Malformed(ref raw) => write!(f, "not an HTTP response: {raw:?}"),
            Self::Answer {
                ref request,
                ref wanted,
                ref found,
            } => write!(f, "{request} answered {found:?}, not {wanted:?}"),
            Self::Provider { seen, wanted } => {
                write!(f, "the provider saw {seen} effects, not {wanted}")
            }
            Self::Verdict(ref what) => write!(f, "the verdict was {what}"),
            Self::Panicked => f.write_str("a client thread panicked"),
        }
    }
}

impl std::error::Error for JourneyFailed {}

impl From<std::io::Error> for JourneyFailed {
    fn from(cause: std::io::Error) -> Self {
        Self::Io(cause)
    }
}

/// Log `failure` and hand it back, so every broken journey leaves a record.
fn failed<T>(failure: JourneyFailed) -> Result<T, JourneyFailed> {
    lgwks_std::trace::warn!(%failure, "app journey: failed");
    Err(failure)
}

/// One posted note.
#[derive(lgwks_std::json::Deserialize)]
#[serde(crate = "lgwks_std::json::serde", deny_unknown_fields)]
struct Note {
    /// The note's text.
    text: String,
}

/// One tenant's notes and the summaries the model answered for them.
#[derive(Debug, Clone, Default)]
struct Notes {
    /// Notes in the order they were posted.
    items: Vec<String>,
    /// Summaries in the order they arrived.
    summaries: Vec<String>,
    /// Effects refused for want of a cap.
    refused: u64,
}

/// A model stand-in that answers an ask with the prompt's length and counts
/// every effect it is handed.
struct Summariser {
    /// Effects that reached the provider.
    seen: Arc<AtomicU64>,
}

impl Provider for Summariser {
    fn dispatch(&self, tenant: &str, effect: &Effect) -> Option<Vec<u8>> {
        self.seen.fetch_add(1, Ordering::AcqRel);
        match *effect {
            Effect::Ask { ref prompt, .. } => {
                Some(format!("{tenant}:{} chars", prompt.chars().count()).into_bytes())
            }
            _ => None,
        }
    }
}

/// The #780 reference app: notes, a model summary per note, and a link
/// preview the app was never granted `net` for.
fn notes_app(seen: &Arc<AtomicU64>) -> App<Notes> {
    App::new("notes")
        .state::<Notes>()
        .grant(Cap::new("app.ask"))
        .provider(Summariser {
            seen: Arc::clone(seen),
        })
        .route(Method::POST, "/notes", |notes: &mut Notes, note: Note| {
            let posted = notes.items.len().saturating_add(1);
            let mut effects = vec![
                Effect::reply(201, posted.to_string().into_bytes()),
                Effect::Ask {
                    prompt: note.text.as_str().into(),
                    reply_to: ReplyTag::new("summarised"),
                },
            ];
            if note.text.starts_with("http") {
                effects.push(Effect::Fetch {
                    url: note.text.as_str().into(),
                    reply_to: ReplyTag::new("previewed"),
                });
            }
            notes.items.push(note.text);
            effects
        })
        .route(Method::GET, "/notes", |notes: &mut Notes, (): ()| {
            let page = format!(
                "{} notes; {}; {} refused",
                notes.items.len(),
                notes.summaries.join("|"),
                notes.refused
            );
            vec![Effect::reply(200, page.into_bytes())]
        })
        .on(ReplyTag::new("summarised"), |notes: &mut Notes, input| {
            if let EffectResult::Value(ref bytes) = *input.result() {
                let summary = String::from_utf8_lossy(bytes).into_owned();
                notes.summaries.push(summary);
            }
            Vec::new()
        })
        .on(ReplyTag::new("previewed"), |notes: &mut Notes, input| {
            if let EffectResult::Refused(_) = *input.result() {
                notes.refused = notes.refused.saturating_add(1);
            }
            Vec::new()
        })
}

/// A client of one served app.
#[derive(Clone, Copy)]
pub(crate) struct Client(SocketAddr);

impl Client {
    /// Send one request on a fresh connection and read the answer to its end:
    /// status and body. The write side stays open: hyper drops a request
    /// whose connection half-closes before it answers, and `connection:
    /// close` ends the stream once it has.
    pub(crate) fn exchange(
        self,
        method: &str,
        path: &str,
        body: &[u8],
    ) -> Result<(u16, String), JourneyFailed> {
        let mut stream = TcpStream::connect(self.0)?;
        let mut request = format!(
            "{method} {path} HTTP/1.1\r\nhost: {}\r\nconnection: close\r\ncontent-length: {}\r\n\r\n",
            self.0,
            body.len()
        )
        .into_bytes();
        request.extend_from_slice(body);
        stream.write_all(&request)?;
        let mut raw = Vec::new();
        stream.read_to_end(&mut raw)?;
        let text = String::from_utf8_lossy(&raw).into_owned();
        let parsed = text.split_once("\r\n\r\n").and_then(|(head, rest)| {
            let code = head
                .strip_prefix("HTTP/1.1 ")?
                .get(..3)?
                .parse::<u16>()
                .ok()?;
            Some((code, rest.to_owned()))
        });
        match parsed {
            Some(answer) => Ok(answer),
            None => failed(JourneyFailed::Malformed(text)),
        }
    }

    /// Require `method` `path` with `body` to answer `status` and `wanted`.
    pub(crate) fn answers(
        self,
        method: &str,
        path: &str,
        body: &[u8],
        status: u16,
        wanted: &str,
    ) -> Result<(), JourneyFailed> {
        let found = self.exchange(method, path, body)?;
        if found.0 != status || found.1 != wanted {
            return failed(JourneyFailed::Answer {
                request: format!("{method} {path}"),
                wanted: (status, wanted.to_owned()),
                found,
            });
        }
        Ok(())
    }
}

/// Serve `app` on a loopback port until `journey` finishes; its result.
pub(crate) fn serve_app<S>(
    app: App<S>,
    journey: impl FnOnce(Client) -> Result<(), JourneyFailed> + Send,
) -> Result<(), Box<dyn std::error::Error>>
where
    S: Clone + Default + Send + 'static,
{
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let client = Client(listener.local_addr()?);
    let stop = EdgeStop::new();
    std::thread::scope(|scope| -> Result<(), Box<dyn std::error::Error>> {
        let finished = scope.spawn(|| {
            let walked = journey(client);
            stop.stop();
            walked
        });
        app.serve(listener, &stop)?;
        match finished.join() {
            Ok(walked) => Ok(walked?),
            Err(_panicked) => Ok(failed(JourneyFailed::Panicked)?),
        }
    })
}

/// Require the provider to have seen `wanted` effects.
fn provider_saw(seen: &AtomicU64, wanted: u64) -> Result<(), JourneyFailed> {
    let seen = seen.load(Ordering::Acquire);
    if seen != wanted {
        return failed(JourneyFailed::Provider { seen, wanted });
    }
    Ok(())
}

/// The reference app serves its routes, feeds each ask's answer to its `on`
/// handler before the next request, refuses the ungranted fetch at dispatch,
/// and keeps each tenant's notes apart.
#[test]
fn the_reference_app_serves_routes_and_feeds_answers_to_on_handlers()
-> Result<(), Box<dyn std::error::Error>> {
    let seen = Arc::new(AtomicU64::new(0));
    serve_app(notes_app(&seen), |client| {
        client.answers("POST", "/t/acme/notes", br#"{"text":"buy milk"}"#, 201, "1")?;
        let link = br#"{"text":"https://x.example"}"#;
        client.answers("POST", "/t/acme/notes", link, 201, "2")?;
        let page = "2 notes; acme:8 chars|acme:17 chars; 1 refused";
        client.answers("GET", "/t/acme/notes", b"", 200, page)?;
        client.answers("GET", "/t/globex/notes", b"", 200, "0 notes; ; 0 refused")
    })?;
    Ok(provider_saw(&seen, 2)?)
}

/// What the app answers without reaching a handler: a body that does not
/// decode, an unknown route, a verb no route takes, and a path with no tenant
/// or a tenant that is not a valid name.
#[test]
fn malformed_requests_are_refused_before_any_handler() -> Result<(), Box<dyn std::error::Error>> {
    let seen = Arc::new(AtomicU64::new(0));
    serve_app(notes_app(&seen), |client| {
        let undecoded = "the body does not decode as this route's input";
        client.answers("POST", "/t/acme/notes", br#"{"text":"#, 400, undecoded)?;
        client.answers("POST", "/t/acme/notes", br#"{"txt":"x"}"#, 400, undecoded)?;
        client.answers("GET", "/t/acme/nowhere", b"", 404, "no such route")?;
        let verbs = "an app route answers GET or POST";
        client.answers("PUT", "/t/acme/notes", b"", 405, verbs)?;
        client.answers("GET", "/notes", b"", 400, "name a tenant: /t/{tenant}/...")?;
        let named = "the tenant is not a valid name";
        client.answers("GET", "/t/a.b/notes", b"", 400, named)?;
        client.answers("GET", "/t/acme/notes", b"", 200, "0 notes; ; 0 refused")
    })?;
    Ok(provider_saw(&seen, 0)?)
}

/// `check` refuses an app whose handler returns an effect it was not granted,
/// and names the route and the cap.
#[test]
fn check_refuses_a_handler_that_needs_an_ungranted_cap() -> Result<(), JourneyFailed> {
    let preview = |_: &mut u64, _: Value| {
        vec![
            Effect::reply(202, Vec::new()),
            Effect::Fetch {
                url: "https://x.example".into(),
                reply_to: ReplyTag::new("previewed"),
            },
        ]
    };
    let app = App::new("previews")
        .state::<u64>()
        .route(Method::POST, "/preview", preview);
    match app.check() {
        Err(AppError::MissingCaps { routes, caps, .. })
            if routes == [0] && caps == [Cap::net()] =>
        {
            Ok(())
        }
        other => failed(JourneyFailed::Verdict(format!("{other:?}"))),
    }
}

/// A plan a model wrote serves its routes over the socket; an oversized,
/// duplicated or malformed plan is refused with its typed error.
#[test]
fn a_model_written_plan_serves_through_the_same_check() -> Result<(), Box<dyn std::error::Error>> {
    let plan = br#"{"name":"hello","routes":[{"method":"POST","path":"/hi","status":201,"body":"hello"}]}"#;
    serve_app(App::<()>::from_plan(plan)?, |client| {
        client.answers("POST", "/t/acme/hi", b"", 201, "hello")?;
        client.answers("GET", "/t/acme/hi", b"", 404, "no such route")
    })?;
    let route = r#"{"method":"GET","path":"/a","status":200,"body":""}"#;
    let twice = format!(r#"{{"name":"x","routes":[{route},{route}]}}"#);
    let verbs = br#"{"name":"x","routes":[{"method":"PUT","path":"/a","status":200,"body":""}]}"#;
    let verdicts = (
        App::<()>::from_plan(&vec![b' '; 65 * 1_024]).err(),
        App::<()>::from_plan(twice.as_bytes()).err(),
        App::<()>::from_plan(verbs).err(),
        App::<()>::from_plan(b"{\"routes\":").err(),
    );
    match verdicts {
        (
            Some(AppError::PlanTooLarge { .. }),
            Some(AppError::DuplicateRoute {
                first: 0,
                second: 1,
            }),
            Some(AppError::MalformedPlan { at: Some(0) }),
            Some(AppError::MalformedPlan { at: None }),
        ) => Ok(()),
        other => Ok(failed(JourneyFailed::Verdict(format!("{other:?}")))?),
    }
}

/// A hundred tenants posting from ten threads each read back exactly their own
/// notes and summaries.
#[test]
fn a_hundred_concurrent_tenants_read_only_their_own_notes() -> Result<(), Box<dyn std::error::Error>>
{
    let seen = Arc::new(AtomicU64::new(0));
    serve_app(notes_app(&seen), |client| {
        std::thread::scope(|scope| -> Result<(), JourneyFailed> {
            let workers: Vec<_> = (0..10_u64)
                .map(|thread| {
                    scope.spawn(move || -> Result<(), JourneyFailed> {
                        for tenant in (thread..100).step_by(10) {
                            let path = format!("/t/tenant-{tenant}/notes");
                            client.answers("POST", &path, br#"{"text":"a"}"#, 201, "1")?;
                            client.answers("POST", &path, br#"{"text":"bb"}"#, 201, "2")?;
                            let page = format!(
                                "2 notes; tenant-{tenant}:1 chars|tenant-{tenant}:2 chars; 0 refused"
                            );
                            client.answers("GET", &path, b"", 200, &page)?;
                        }
                        Ok(())
                    })
                })
                .collect();
            for worker in workers {
                match worker.join() {
                    Ok(walked) => walked?,
                    Err(_panicked) => return failed(JourneyFailed::Panicked),
                }
            }
            Ok(())
        })
    })?;
    Ok(provider_saw(&seen, 200)?)
}
