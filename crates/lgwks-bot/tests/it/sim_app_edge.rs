//! What an app's edge answers itself: liveness with no tenant, and who a request is.
//!
//! Source: a consumer's asks in #879 and #880, and RFC 6750 section 3 (a refused
//! bearer request is `401` with `WWW-Authenticate`). Two tenants each hold a key on one
//! app at once; a refused request must queue nothing.

use std::io::{Read as _, Write as _};
use std::net::{SocketAddr, TcpListener, TcpStream};

use lgwks_bot::app::{App, AppError, Effect, Method, Principal, RequestHeaders};
use lgwks_bot::rt::edge::EdgeStop;
use lgwks_std::seeded::Seeded;

/// A test's outcome: setup failures propagate instead of panicking.
type Outcome<T = ()> = Result<T, Box<dyn std::error::Error>>;

/// A key, shown as raw header bytes.
type Shown<'head> = (&'head str, &'head [u8]);

/// One answer off the wire; status `0` when the connection closed with no answer.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Heard {
    /// The status code.
    status: u16,
    /// The response head.
    head: String,
    /// The response body.
    body: String,
}

/// One request on its own connection, with exactly the headers given.
fn ask(to: SocketAddr, request: &str, shown: &[Shown<'_>]) -> Outcome<Heard> {
    let mut wire = TcpStream::connect(to)?;
    let mut bytes =
        format!("{request} HTTP/1.1\r\nhost: edge\r\nconnection: close\r\n").into_bytes();
    for &(name, value) in shown {
        bytes.extend_from_slice(name.as_bytes());
        bytes.extend_from_slice(b": ");
        bytes.extend_from_slice(value);
        bytes.extend_from_slice(b"\r\n");
    }
    bytes.extend_from_slice(b"content-length: 0\r\n\r\n");
    wire.write_all(&bytes)?;
    let mut raw = Vec::new();
    if let Err(cause) = wire.read_to_end(&mut raw) {
        lgwks_std::trace::debug!(%cause, "sim_app_edge: the connection was reset");
    }
    let text = String::from_utf8_lossy(&raw).into_owned();
    let (head, body) = match text.split_once("\r\n\r\n") {
        Some((head, body)) => (head.to_owned(), body.to_owned()),
        None => (String::new(), String::new()),
    };
    let word = head.split(' ').nth(1);
    let status = match word.map(str::parse::<u16>) {
        Some(Ok(status)) => status,
        _ => 0,
    };
    Ok(Heard { status, head, body })
}

/// A counter per tenant, behind a key per tenant: `key-{tenant}`. A header named
/// `x-boom` makes the authenticator panic.
fn keyed() -> App<u64> {
    let counter = |count: &mut u64, who: &Principal, (): ()| {
        vec![Effect::reply(
            200,
            format!("{}:{count}", who.as_str()).into_bytes(),
        )]
    };
    App::new("keyed")
        .state::<u64>()
        .authenticate(|tenant: &str, headers: &RequestHeaders<'_>| {
            assert!(
                headers.get("x-boom").is_none(),
                "the authenticator fell over"
            );
            let token = headers.bearer()?;
            (token == format!("key-{tenant}"))
                .then(|| Principal::new(tenant))
                .flatten()
        })
        .route_as(
            Method::POST,
            "/add",
            move |count: &mut u64, who: &Principal, (): ()| {
                *count = count.saturating_add(1);
                counter(count, who, ())
            },
        )
        .route_as(Method::GET, "/count", counter)
}

/// Serve `app` on a loopback port while `journey` walks it, then stop.
fn served<S, T>(app: App<S>, journey: impl FnOnce(SocketAddr) -> Outcome<T> + Send) -> Outcome<T>
where
    S: Clone + Default + Send + 'static,
    T: Send,
{
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let to = listener.local_addr()?;
    let stop = EdgeStop::new();
    std::thread::scope(|threads| {
        let walked = threads.spawn(|| {
            let done = journey(to).map_err(|cause| cause.to_string());
            stop.stop();
            done
        });
        app.serve(listener, &stop)?;
        match walked.join() {
            Ok(done) => Ok(done?),
            Err(_panicked) => Err("the journey panicked".into()),
        }
    })
}

/// The header that shows `key` as a bearer token.
fn bearer(key: &str) -> Vec<u8> {
    format!("Bearer {key}").into_bytes()
}

/// Catches one tenant's key opening another's state.
#[test]
fn sim_app_edge_neighbour_a_key_of_one_tenant_is_refused_on_anothers_path() -> Outcome {
    served(keyed(), |to| {
        let (acme, globex) = (bearer("key-acme"), bearer("key-globex"));
        let mine: &[Shown<'_>] = &[("authorization", &acme)];
        let theirs: &[Shown<'_>] = &[("authorization", &globex)];
        assert_eq!(ask(to, "POST /t/acme/add", mine)?.body, "acme:1");
        let refused = ask(to, "POST /t/acme/add", theirs)?;
        assert_eq!(refused.status, 401);
        assert!(
            refused
                .head
                .to_lowercase()
                .contains("www-authenticate: bearer")
        );
        assert_eq!(ask(to, "POST /t/globex/add", theirs)?.body, "globex:1");
        assert_eq!(
            ask(to, "GET /t/acme/count", mine)?.body,
            "acme:1",
            "nothing queued"
        );
        Ok(())
    })
}

/// Catches a malformed, repeated or unreadable key being read as a key.
#[test]
fn sim_app_edge_weird_bytes_only_one_well_formed_bearer_token_is_a_key() -> Outcome {
    served(keyed(), |to| {
        let good = bearer("key-acme");
        let cases: [(&[Shown<'_>], u16); 8] = [
            (&[], 401),
            (&[("authorization", b"Bearer")], 401),
            (&[("authorization", b"Bearer ")], 401),
            (&[("authorization", b"Basic key-acme")], 401),
            (&[("authorization", b"Bearer key-acme extra")], 401),
            (&[("authorization", b"Bearer key-\xff\xfe")], 401),
            (&[("authorization", &good), ("authorization", &good)], 401),
            (&[("authorization", b"bEaReR key-acme")], 200),
        ];
        for (shown, wanted) in cases {
            assert_eq!(
                ask(to, "GET /t/acme/count", shown)?.status,
                wanted,
                "{shown:?}"
            );
        }
        Ok(())
    })
}

/// Catches a head past the bound reaching the authenticator, or the bound being wrong.
#[test]
fn sim_app_edge_giant_and_empty_a_head_is_bounded_exactly() -> Outcome {
    served(keyed(), |to| {
        let good = bearer("key-acme");
        let names: Vec<String> = (0..61).map(|number| format!("x-{number}")).collect();
        let mut shown: Vec<Shown<'_>> = vec![("authorization", &good)];
        // Host, connection and content-length are three more: sixty-four in all.
        shown.extend(
            names
                .iter()
                .take(60)
                .map(|name| (name.as_str(), b"1".as_slice())),
        );
        assert_eq!(ask(to, "GET /t/acme/count", &shown)?.status, 200);
        if let Some(name) = names.last() {
            shown.push((name.as_str(), b"1"));
        }
        assert_eq!(ask(to, "GET /t/acme/count", &shown)?.status, 431);
        let vast = vec![b'v'; 17 * 1_024];
        let wide: &[Shown<'_>] = &[("authorization", &good), ("x-vast", &vast)];
        assert_eq!(ask(to, "GET /t/acme/count", wide)?.status, 431);
        Ok(())
    })
}

/// Catches an unknown caller learning anything from a body, and a fallen authenticator
/// admitting a request or taking the edge down.
#[test]
fn sim_app_edge_saboteur_an_unknown_caller_learns_nothing_and_a_panic_admits_nothing() -> Outcome {
    served(keyed(), |to| {
        let good = bearer("key-acme");
        let mine: &[Shown<'_>] = &[("authorization", &good)];
        assert_eq!(
            ask(to, "DELETE /t/acme/add", &[])?.status,
            401,
            "before the verb"
        );
        assert_eq!(ask(to, "GET /t/not%20a%20tenant!/count", &[])?.status, 400);
        let fallen = ask(
            to,
            "POST /t/acme/add",
            &[("authorization", &good), ("x-boom", b"1")],
        )?;
        assert_ne!(fallen.status, 200);
        assert_eq!(ask(to, "GET /t/acme/count", mine)?.body, "acme:0");
        Ok(())
    })
}

/// Catches a route that needs a principal serving with nobody to name one.
#[test]
fn sim_app_edge_wrong_order_a_named_route_needs_an_authenticator() -> Outcome {
    let bare = App::new("bare").state::<u64>().route_as(
        Method::GET,
        "/me",
        |_: &mut u64, who: &Principal, (): ()| {
            vec![Effect::reply(200, who.as_str().as_bytes().to_vec())]
        },
    );
    assert!(matches!(
        bare.check(),
        Err(AppError::NoAuthenticator { route: 0 })
    ));
    assert!(Principal::new("").is_none());
    assert!(Principal::new(&"p".repeat(257)).is_none());
    assert!(Principal::new(&"p".repeat(256)).is_some());
    Ok(())
}

/// Catches liveness needing a key or a tenant, and a verb or path that is not a probe.
#[test]
fn sim_app_edge_fat_finger_only_the_probe_is_answered_without_a_tenant() -> Outcome {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let to = listener.local_addr()?;
    drop(listener);
    served(keyed(), |to| {
        let probe = ask(to, "GET /healthz", &[])?;
        assert_eq!((probe.status, probe.body.as_str()), (200, "ok"));
        assert_eq!(ask(to, "HEAD /healthz", &[])?.status, 200);
        assert_eq!(ask(to, "POST /healthz", &[])?.status, 400);
        assert_eq!(ask(to, "GET /healthz/", &[])?.status, 400);
        assert_eq!(
            ask(to, "GET /t/healthz", &[])?.status,
            401,
            "that is a tenant"
        );
        Ok(())
    })?;
    assert!(
        TcpStream::connect(to).is_err(),
        "nothing answers on a closed port"
    );
    Ok(())
}

/// Catches a lost or doubled request among callers racing with and without keys.
#[test]
fn sim_app_edge_double_clicker_a_hundred_callers_count_exactly() -> Outcome {
    served(keyed(), |to| {
        let (acme, globex) = (bearer("key-acme"), bearer("key-globex"));
        std::thread::scope(|threads| {
            for number in 0_u32..100 {
                let (acme, globex) = (&acme, &globex);
                threads.spawn(move || match number % 4 {
                    0 => ask(to, "POST /t/acme/add", &[("authorization", acme)]).is_ok(),
                    1 => ask(to, "POST /t/globex/add", &[("authorization", globex)]).is_ok(),
                    2 => ask(to, "POST /t/acme/add", &[("authorization", globex)]).is_ok(),
                    _ => ask(to, "POST /t/globex/add", &[]).is_ok(),
                });
            }
        });
        let acme_count = ask(to, "GET /t/acme/count", &[("authorization", &acme)])?;
        let globex_count = ask(to, "GET /t/globex/count", &[("authorization", &globex)])?;
        assert_eq!(acme_count.body, "acme:25");
        assert_eq!(globex_count.body, "globex:25");
        Ok(())
    })
}

/// One seed's requests against a model of each tenant's count, as a trace.
fn monkey(seed: u64) -> Outcome<Vec<(u16, String)>> {
    let mut dice = Seeded::from_seed(seed);
    let mut moves = Vec::with_capacity(30);
    for _ in 0..30 {
        moves.push((dice.below(2)?, dice.below(4)?, dice.below(2)?));
    }
    served(keyed(), move |to| {
        let tenants = ["acme", "globex"];
        let mut model = [0_u64; 2];
        let mut trace = Vec::new();
        for (which, key, verb) in moves {
            let at = usize::try_from(which)?;
            let (tenant, other) = (tenants[at], tenants[at ^ 1]);
            let shown = match key {
                0 | 1 => bearer(&format!("key-{tenant}")),
                2 => bearer(&format!("key-{other}")),
                _ => b"Bearer nonsense".to_vec(),
            };
            let request = if verb == 0 {
                format!("POST /t/{tenant}/add")
            } else {
                format!("GET /t/{tenant}/count")
            };
            let heard = ask(to, &request, &[("authorization", &shown)])?;
            if key <= 1 {
                if verb == 0 {
                    model[at] = model[at].saturating_add(1);
                }
                assert_eq!(heard.status, 200, "seed {seed}");
                assert_eq!(heard.body, format!("{tenant}:{}", model[at]), "seed {seed}");
            } else {
                assert_eq!(heard.status, 401, "seed {seed}");
            }
            trace.push((heard.status, heard.body));
        }
        Ok(trace)
    })
}

/// Catches any drift from the model, and any answer a scheduler could change.
#[test]
fn sim_app_edge_monkey_random_requests_match_the_model_and_replay_exactly() -> Outcome {
    for seed in 0..12 {
        assert_eq!(monkey(seed)?, monkey(seed)?, "seed {seed} did not replay");
    }
    Ok(())
}
