//! The address rule of [`Options::address_rule`]: every address a name answers
//! is judged before a socket opens, so an inward name, a rebinding name or a
//! proxy gets no connection (CWE-918 server-side request forgery; DNS
//! rebinding; #1170).
//!
//! Names are answered through [`Options::lookup`], so no test reads DNS or
//! `/etc/hosts`. Every connection these tests expect or refuse is on loopback,
//! and a refused listener is read without blocking after the call returned: a
//! TCP handshake the client completed is already queued by then. One seed
//! drives each generated answer, so a failing assertion names a seed that
//! replays it.
#![cfg(feature = "http")]
#![forbid(unsafe_code)]

use crate::loopback;
use crate::rng;

#[cfg(feature = "http")]
mod sim {
    use std::io::{ErrorKind, Write};
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, TcpListener, TcpStream};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    use lgwks_std::http::{self, Client, Error, FailureStage, Options, PoolConfig};

    use crate::sim_http_address::loopback::read_request_head;
    use crate::sim_http_address::rng::Rng;

    /// What every test here returns.
    type Outcome = Result<(), Box<dyn std::error::Error>>;

    /// Seeds per generated family.
    const SEEDS: u64 = 2_000;

    /// The cloud metadata address a tenant URL must never reach.
    const METADATA: IpAddr = IpAddr::V4(Ipv4Addr::new(169, 254, 169, 254));
    /// IPv4 loopback.
    const V4_LOOPBACK: IpAddr = IpAddr::V4(Ipv4Addr::LOCALHOST);
    /// IPv6 loopback, which stands for a public address where a test needs a
    /// second address it can really connect to.
    const V6_LOOPBACK: IpAddr = IpAddr::V6(Ipv6Addr::LOCALHOST);

    /// A rule as a service writes one: nothing private, loopback or link-local.
    fn public(address: IpAddr) -> bool {
        match address {
            IpAddr::V4(v4) => {
                !(v4.is_private() || v4.is_loopback() || v4.is_link_local() || v4.is_unspecified())
            }
            IpAddr::V6(v6) => !(v6.is_loopback() || v6.is_unspecified()),
        }
    }

    /// A lookup that answers `script` in order and repeats its last answer,
    /// counting the calls.
    fn scripted(
        script: Vec<Vec<IpAddr>>,
        calls: &Arc<AtomicUsize>,
    ) -> impl Fn(&str) -> std::io::Result<Vec<IpAddr>> + Send + Sync + 'static {
        let calls = Arc::clone(calls);
        move |_name: &str| {
            let call = calls.fetch_add(1, Ordering::SeqCst);
            script
                .get(call)
                .or_else(|| script.last())
                .cloned()
                .ok_or_else(|| std::io::Error::other("an empty script"))
        }
    }

    /// Options whose every phase ends well inside the test's own budget.
    fn quick() -> Options {
        Options::default().timeout(Duration::from_secs(5))
    }

    /// A listener on `address` and its port.
    fn listen(address: IpAddr) -> std::io::Result<(TcpListener, u16)> {
        let listener = TcpListener::bind(SocketAddr::new(address, 0))?;
        let port = listener.local_addr()?.port();
        Ok((listener, port))
    }

    /// Whether any connection is queued on `listener`.
    fn was_reached(listener: &TcpListener) -> std::io::Result<bool> {
        listener.set_nonblocking(true)?;
        match listener.accept() {
            Ok(_) => Ok(true),
            Err(error) if error.kind() == ErrorKind::WouldBlock => Ok(false),
            Err(error) => Err(error),
        }
    }

    /// Answer `replies` requests on `listener`, one connection each, returning
    /// each request head.
    fn answer(listener: &TcpListener, replies: &[String]) -> std::io::Result<Vec<String>> {
        let mut heads = Vec::new();
        for reply in replies {
            let (mut stream, _) = listener.accept()?;
            let head = read_request_head(&mut stream)?;
            if head.is_empty() {
                break;
            }
            heads.push(String::from_utf8_lossy(&head).to_ascii_lowercase());
            stream.write_all(reply.as_bytes())?;
        }
        Ok(heads)
    }

    /// A `200` carrying `body`, closing the connection.
    fn ok(body: &str) -> String {
        format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
    }

    /// Run `call` while `listener` answers `replies`, returning what both saw.
    ///
    /// A call that never reaches the listener cannot strand the server: each
    /// expected connection is topped up by an empty one the server stops at.
    fn served<T>(
        listener: &TcpListener,
        replies: &[String],
        call: impl FnOnce() -> T,
    ) -> Result<(T, Vec<String>), Box<dyn std::error::Error>> {
        let address = listener.local_addr()?;
        std::thread::scope(|scope| {
            let server = scope.spawn(|| answer(listener, replies));
            let seen = call();
            drop(TcpStream::connect(address)?);
            Ok((seen, joined(server.join())?))
        })
    }

    /// What a joined fixture server returned, or why it did not: its own
    /// refusal, or the words it panicked with.
    fn joined<T>(
        ended: std::thread::Result<std::io::Result<T>>,
    ) -> Result<T, Box<dyn std::error::Error>> {
        match ended {
            Ok(served) => Ok(served?),
            Err(payload) => {
                let words = match payload.downcast_ref::<String>() {
                    Some(text) => text.as_str(),
                    None => match payload.downcast_ref::<&str>() {
                        Some(text) => text,
                        None => "a payload that is not text",
                    },
                };
                lgwks_std::trace::debug!(words, "the fixture server panicked");
                Err(format!("the fixture server panicked: {words}").into())
            }
        }
    }

    /// The address a refusal names, or the whole answer when it is not one.
    fn refused<T: std::fmt::Debug>(
        answer: Result<T, Error>,
    ) -> Result<(String, IpAddr), Box<dyn std::error::Error>> {
        match answer {
            Err(Error::AddressRefused { host, address }) => Ok((host, address)),
            other => Err(format!("expected an address refusal, got {other:?}").into()),
        }
    }

    #[test]
    fn a_name_answering_the_metadata_address_is_refused_before_any_connect() -> Outcome {
        let calls = Arc::new(AtomicUsize::new(0));
        let options = quick()
            .address_rule(public)
            .lookup(scripted(vec![vec![METADATA]], &calls));
        let answer = http::get_with("http://metadata.test/latest/meta-data/", &options);
        let Err(ref error) = answer else {
            return Err(format!("the metadata address was reached: {answer:?}").into());
        };
        assert_eq!(
            error.to_string(),
            "host `metadata.test` answers 169.254.169.254, which the address rule refuses; \
             none of that answer was dialled",
            "the refusal names the host and the address"
        );
        assert_eq!(
            refused(answer)?,
            ("metadata.test".to_owned(), METADATA),
            "typed refusal"
        );
        assert_eq!(calls.load(Ordering::SeqCst), 1, "one lookup per request");
        Ok(())
    }

    #[test]
    fn a_name_that_rebinds_after_the_first_answer_is_never_looked_up_twice_per_dial() -> Outcome {
        let (private, port) = listen(V4_LOOPBACK)?;
        let calls = Arc::new(AtomicUsize::new(0));
        // A client that checked one lookup and dialled another would reach
        // 127.0.0.1 here. The one lookup answers the admitted address, where
        // nothing listens on this port, and that is all that is dialled.
        let options = quick()
            .address_rule(|address| address == V6_LOOPBACK)
            .lookup(scripted(vec![vec![V6_LOOPBACK], vec![V4_LOOPBACK]], &calls));
        let answer = http::get_with(&format!("http://rebind.test:{port}/"), &options);
        assert!(
            matches!(
                answer,
                Err(Error::Failure {
                    stage: FailureStage::Connect | FailureStage::Request,
                    ..
                })
            ),
            "only the judged address is dialled: {answer:?}"
        );
        assert_eq!(calls.load(Ordering::SeqCst), 1, "one lookup per dial");
        assert!(
            !was_reached(&private)?,
            "the rebound address got a connection"
        );
        Ok(())
    }

    #[test]
    fn a_redirect_hop_that_rebinds_to_loopback_gets_no_connection() -> Outcome {
        let (front, front_port) = listen(V6_LOOPBACK)?;
        let (private, private_port) = listen(V4_LOOPBACK)?;
        let calls = Arc::new(AtomicUsize::new(0));
        let options = quick()
            .address_rule(|address| address == V6_LOOPBACK)
            .lookup(scripted(vec![vec![V6_LOOPBACK], vec![V4_LOOPBACK]], &calls));
        let redirect = format!(
            "HTTP/1.1 302 Found\r\nLocation: http://rebind.test:{private_port}/inside\r\n\
             Content-Length: 0\r\nConnection: close\r\n\r\n"
        );
        let (answer, heads) = served(&front, &[redirect], || {
            http::get_with(&format!("http://rebind.test:{front_port}/"), &options)
        })?;
        assert_eq!(
            refused(answer)?,
            ("rebind.test".to_owned(), V4_LOOPBACK),
            "the second hop's answer is judged"
        );
        assert_eq!(heads.len(), 1, "the admitted first hop was served");
        assert_eq!(calls.load(Ordering::SeqCst), 2, "one lookup per hop");
        assert!(
            !was_reached(&private)?,
            "the rebound address got a connection"
        );
        Ok(())
    }

    /// The answer family `seed` names: 1 to 24 addresses, at least one of them
    /// outside loopback, and the first such address.
    fn mixed_answer(seed: u64) -> (Vec<IpAddr>, Option<IpAddr>) {
        let mut rng = Rng::new(seed);
        let admitted = [V4_LOOPBACK, V6_LOOPBACK];
        let inward = [
            METADATA,
            IpAddr::V4(Ipv4Addr::new(10, 0, 0, 7)),
            IpAddr::V4(Ipv4Addr::new(192, 168, 1, 1)),
            IpAddr::V6(Ipv6Addr::new(0xfd00, 0, 0, 0, 0, 0, 0, 1)),
        ];
        let length = rng.below(24).saturating_add(1);
        let forced = rng.below(length);
        let answer: Vec<IpAddr> = (0..length)
            .filter_map(|position| {
                let refuse = position == forced || rng.below(8) == 0;
                let pool: &[IpAddr] = if refuse { &inward } else { &admitted };
                pool.get(rng.below(pool.len())).copied()
            })
            .collect();
        let first = answer
            .iter()
            .copied()
            .find(|address| !address.is_loopback());
        (answer, first)
    }

    /// The seed a `seed-N.test` name carries.
    fn seed_of(name: &str) -> std::io::Result<u64> {
        name.strip_prefix("seed-")
            .and_then(|rest| rest.strip_suffix(".test"))
            .and_then(|digits| digits.parse().ok())
            .ok_or_else(|| std::io::Error::other("not a seeded name"))
    }

    #[test]
    fn sim_every_mixed_answer_is_refused_whole_before_a_dial() -> Outcome {
        let (admitted, port) = listen(V4_LOOPBACK)?;
        // The rule admits loopback alone, so a client that filtered the answer
        // and dialled what was left would reach the listener.
        let client = Client::new(
            quick()
                .address_rule(|address| address.is_loopback())
                .lookup(|name| Ok(mixed_answer(seed_of(name)?).0)),
        )?;
        for seed in 0..SEEDS {
            let (answer, first) = mixed_answer(seed);
            let url = format!("http://seed-{seed}.test:{port}/");
            let (host, address) = refused(client.get_with(&url, client.options()))
                .map_err(|error| format!("seed {seed}, answer {answer:?}: {error}"))?;
            assert_eq!(host, format!("seed-{seed}.test"), "seed {seed}: the host");
            assert_eq!(
                Some(address),
                first,
                "seed {seed}: the first refused answer of {answer:?}"
            );
        }
        assert!(!was_reached(&admitted)?, "a mixed answer was dialled");
        Ok(())
    }

    /// The private IPv4 address `seed` names, in each range a service refuses.
    fn private_v4(seed: u64) -> Ipv4Addr {
        let mut rng = Rng::new(seed);
        let [third, fourth, ..] = rng.next().to_le_bytes();
        match rng.below(5) {
            0 => Ipv4Addr::new(10, 1, third, fourth),
            1 => Ipv4Addr::new(127, 0, third, fourth),
            2 => Ipv4Addr::new(169, 254, third, fourth),
            3 => Ipv4Addr::new(192, 168, third, fourth),
            _ => Ipv4Addr::new(172, 16, third, fourth),
        }
    }

    #[test]
    fn sim_an_ipv4_mapped_answer_is_judged_as_the_ipv4_it_dials() -> Outcome {
        // A rule written for IPv4 alone, which admits every IPv6 address. A
        // socket to `::ffff:a.b.c.d` reaches `a.b.c.d`, so the rule must be
        // asked about `a.b.c.d`.
        let client = Client::new(
            quick()
                .address_rule(|address| match address {
                    IpAddr::V4(_) => public(address),
                    IpAddr::V6(_) => true,
                })
                .lookup(|name| {
                    Ok(vec![IpAddr::V6(
                        private_v4(seed_of(name)?).to_ipv6_mapped(),
                    )])
                }),
        )?;
        for seed in 0..SEEDS {
            let url = format!("http://seed-{seed}.test/");
            let (_, address) = refused(client.get_with(&url, client.options()))
                .map_err(|error| format!("seed {seed}, {}: {error}", private_v4(seed)))?;
            assert_eq!(
                address,
                IpAddr::V6(private_v4(seed).to_ipv6_mapped()),
                "seed {seed}: the refusal names the answer as it arrived"
            );
        }
        Ok(())
    }

    #[test]
    fn an_ip_literal_is_judged_without_a_lookup() -> Outcome {
        for (bind, host) in [
            (V4_LOOPBACK, "127.0.0.1"),
            (V6_LOOPBACK, "[::1]"),
            (V4_LOOPBACK, "[::ffff:127.0.0.1]"),
        ] {
            let (listener, port) = listen(bind)?;
            let calls = Arc::new(AtomicUsize::new(0));
            let url = format!("http://{host}:{port}/");
            let options = quick()
                .address_rule(public)
                .lookup(scripted(vec![vec![METADATA]], &calls));
            let (named, address) = refused(http::get_with(&url, &options))?;
            assert_eq!(named, host, "the refusal names the literal as written");
            assert!(address.to_canonical().is_loopback(), "{host}: {address}");
            assert!(!was_reached(&listener)?, "{host} was dialled");
            assert_eq!(calls.load(Ordering::SeqCst), 0, "{host} needs no lookup");
        }
        let (listener, port) = listen(V6_LOOPBACK)?;
        let admitting = quick().address_rule(|address| address == V6_LOOPBACK);
        let (answer, _) = served(&listener, &[ok("six")], || {
            http::get_with(&format!("http://[::1]:{port}/"), &admitting)
        })?;
        assert_eq!(answer?.body(), b"six", "an admitted literal completes");
        Ok(())
    }

    #[test]
    fn the_system_resolver_is_judged_when_no_lookup_is_set() -> Outcome {
        let (listener, port) = listen(V4_LOOPBACK)?;
        let options = quick().address_rule(public);
        let (host, address) = refused(http::get_with(
            &format!("http://localhost:{port}/"),
            &options,
        ))?;
        assert_eq!(host, "localhost", "the refused name");
        assert!(address.is_loopback(), "localhost answered {address}");
        assert!(!was_reached(&listener)?, "localhost was dialled");
        Ok(())
    }

    #[test]
    fn an_admitted_answer_completes_under_the_name_the_caller_asked_for() -> Outcome {
        let (listener, port) = listen(V4_LOOPBACK)?;
        // Forty answers: more than one connection keeps.
        let options = quick()
            .address_rule(|address| address.is_loopback())
            .lookup(|_name| Ok(vec![V4_LOOPBACK; 40]));
        let (answer, heads) = served(&listener, &[ok("hello")], || {
            http::get_with(&format!("http://service.test:{port}/"), &options)
        })?;
        assert_eq!(answer?.body(), b"hello", "the exchange completes");
        assert!(
            heads
                .iter()
                .any(|head| head.contains(&format!("host: service.test:{port}"))),
            "the request names the host, not the address: {heads:?}"
        );
        Ok(())
    }

    #[test]
    fn a_lookup_alone_answers_the_name_and_judges_nothing() -> Outcome {
        let (listener, port) = listen(V4_LOOPBACK)?;
        let calls = Arc::new(AtomicUsize::new(0));
        let options = quick().lookup(scripted(vec![vec![V4_LOOPBACK]], &calls));
        let (answer, _) = served(&listener, &[ok("plain")], || {
            http::get_with(&format!("http://service.test:{port}/"), &options)
        })?;
        assert_eq!(answer?.body(), b"plain", "no rule, nothing refused");
        assert_eq!(calls.load(Ordering::SeqCst), 1, "the lookup answered");
        Ok(())
    }

    #[test]
    fn a_name_with_no_answer_fails_at_the_resolve_stage() -> Outcome {
        let empty = quick().address_rule(public).lookup(|_name| Ok(Vec::new()));
        let failing = quick()
            .address_rule(public)
            .lookup(|_name| Err(std::io::Error::other("the resolver is down")));
        for (options, said) in [(empty, None), (failing, Some("the resolver is down"))] {
            let answer = http::get_with("http://gone.test/", &options);
            let Err(Error::Failure {
                stage: FailureStage::Resolve,
                ref cause,
                ..
            }) = answer
            else {
                return Err(format!("expected a resolve failure, got {answer:?}").into());
            };
            assert_eq!(
                cause.as_ref().map(|cause| cause.message()),
                said,
                "the lookup's own words are kept"
            );
        }
        Ok(())
    }

    #[test]
    fn a_client_keeps_its_rule_when_per_call_options_carry_none() -> Outcome {
        let (listener, port) = listen(V4_LOOPBACK)?;
        let client = Client::new(
            quick()
                .address_rule(public)
                .lookup(|_name| Ok(vec![V4_LOOPBACK])),
        )?;
        let url = format!("http://service.test:{port}/");
        let bare = quick().header("x-tenant", "one");
        refused(client.get_with(&url, &bare))?;
        refused(client.post_with(&url, "text/plain", b"x", &bare))?;
        refused(client.get_with(&url, &Options::default()))?;
        refused(client.get_stream(&url).map(|_streamed| ()))?;
        assert!(
            !was_reached(&listener)?,
            "a per-call value dropped the rule"
        );
        Ok(())
    }

    /// Answer the request `stream` already sent and every later one, keeping it
    /// open until the client closes it. Returns how many it carried.
    fn keep_serving(mut stream: TcpStream) -> std::io::Result<usize> {
        let keep_alive = "HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok";
        let mut requests = 0_usize;
        loop {
            requests = requests.saturating_add(1);
            stream.write_all(keep_alive.as_bytes())?;
            if read_request_head(&mut stream)?.is_empty() {
                return Ok(requests);
            }
        }
    }

    /// Run `calls` on `client` while every connection to `listener` is served and
    /// held open, returning what they saw and the requests each connection
    /// carried, in the order they were opened.
    fn per_connection<T>(
        listener: &TcpListener,
        client: Client,
        calls: impl FnOnce(&Client) -> T,
    ) -> Result<(T, Vec<usize>), Box<dyn std::error::Error>> {
        let address = listener.local_addr()?;
        std::thread::scope(|scope| {
            let server = scope.spawn(|| {
                std::thread::scope(|each| -> std::io::Result<Vec<usize>> {
                    let mut serving = Vec::new();
                    loop {
                        let (mut stream, _) = listener.accept()?;
                        // The connection that sends nothing ends the server.
                        if read_request_head(&mut stream)?.is_empty() {
                            break;
                        }
                        serving.push(each.spawn(move || keep_serving(stream)));
                    }
                    serving
                        .into_iter()
                        .map(|one| {
                            joined(one.join())
                                .map_err(|cause| std::io::Error::other(cause.to_string()))
                        })
                        .collect()
                })
            });
            let seen = calls(&client);
            drop(client);
            drop(TcpStream::connect(address)?);
            Ok((seen, joined(server.join())?))
        })
    }

    #[test]
    fn a_stricter_per_call_rule_does_not_ride_the_pooled_connection() -> Outcome {
        let (listener, port) = listen(V4_LOOPBACK)?;
        let client = Client::new(quick().lookup(|_name| Ok(vec![V4_LOOPBACK])))?;
        let url = format!("http://service.test:{port}/");
        // One connection serves both of the client's own calls and stays open
        // in the pool while the ruled call is made.
        let (answers, carried) = per_connection(&listener, client, |client| {
            let first = client.get_with(&url, client.options());
            let again = client.get_with(&url, client.options());
            let stricter = client.options().clone().address_rule(public);
            [first, again, client.get_with(&url, &stricter)]
        })?;
        let [first, again, third] = answers;
        assert_eq!(first?.status(), 200, "the unruled client reaches loopback");
        assert_eq!(again?.status(), 200, "and again");
        refused(third)?;
        assert_eq!(carried, [2], "one pooled connection, none for the refusal");
        Ok(())
    }

    /// Source: CWE-918, a connection reused under a call that asked for other
    /// settings. Requirement (INV-STD-HTTP-6): a call that changes any one agent
    /// setting runs on its own agent under the client's rule, and the client's
    /// pooled connection serves only the calls that change none.
    #[test]
    fn a_per_call_agent_setting_leaves_the_pool_and_keeps_the_rule() -> Outcome {
        let (listener, port) = listen(V4_LOOPBACK)?;
        let calls = Arc::new(AtomicUsize::new(0));
        let client = Client::new(
            quick()
                .address_rule(|address| address.is_loopback())
                .lookup(scripted(vec![vec![V4_LOOPBACK]], &calls)),
        )?;
        let url = format!("http://service.test:{port}/");
        let (answers, carried) = per_connection(&listener, client, |client| {
            let first = client.get_with(&url, client.options());
            let slower = client.options().clone().timeout(Duration::from_secs(6));
            let slower = client.get_with(&url, &slower);
            let narrower = PoolConfig::default().max_idle_connections(1);
            let narrower = client.options().clone().pool(narrower);
            let narrower = client.get_with(&url, &narrower);
            let again = client.get_with(&url, client.options());
            [first, slower, narrower, again]
        })?;
        for answer in answers {
            assert_eq!(answer?.status(), 200, "every call completes");
        }
        assert_eq!(carried, [2, 1, 1], "each changed call dialled its own");
        assert_eq!(
            calls.load(Ordering::SeqCst),
            4,
            "every request is looked up"
        );
        Ok(())
    }

    /// Requirement (INV-STD-HTTP-6): a name that turns inward is refused even
    /// while a connection admitted earlier sits in the pool.
    #[test]
    fn a_pooled_connection_does_not_outlive_the_answer_that_admitted_it() -> Outcome {
        let (listener, port) = listen(V4_LOOPBACK)?;
        let calls = Arc::new(AtomicUsize::new(0));
        let client = Client::new(
            quick()
                .address_rule(|address| address.is_loopback())
                .lookup(scripted(vec![vec![V4_LOOPBACK], vec![METADATA]], &calls)),
        )?;
        let url = format!("http://service.test:{port}/");
        let (answers, carried) = per_connection(&listener, client, |client| {
            let first = client.get_with(&url, client.options());
            [first, client.get_with(&url, client.options())]
        })?;
        let [first, turned] = answers;
        assert_eq!(first?.status(), 200, "the admitted answer completes");
        assert_eq!(refused(turned)?.1, METADATA, "judged on every request");
        assert_eq!(carried, [1], "the refused request was never sent");
        Ok(())
    }

    /// Requirement (INV-STD-HTTP-6): a lookup the client does not hold is a
    /// change too, so the call dials the addresses that lookup answers.
    #[test]
    fn a_per_call_lookup_does_not_ride_the_pooled_connection() -> Outcome {
        let (listener, port) = listen(V4_LOOPBACK)?;
        let client = Client::new(quick().address_rule(|address| address.is_loopback()))?;
        let url = format!("http://localhost:{port}/");
        let (answers, carried) = per_connection(&listener, client, |client| {
            let first = client.get_with(&url, client.options());
            let named = client.options().clone().lookup(|_name| Ok(vec![METADATA]));
            [first, client.get_with(&url, &named)]
        })?;
        let [first, named] = answers;
        assert_eq!(first?.status(), 200, "the system resolver's answer");
        assert_eq!(refused(named)?.1, METADATA, "the call's lookup answered");
        assert_eq!(carried, [1], "the second call never rode the pool");
        Ok(())
    }

    #[test]
    fn a_debug_of_options_says_which_address_settings_are_present() {
        let ruled = Options::default().address_rule(public);
        let named = Options::default().lookup(|_name| Ok(Vec::new()));
        for (options, said) in [
            (
                Options::default(),
                "Addressing { rule: false, lookup: false }",
            ),
            (ruled, "Addressing { rule: true, lookup: false }"),
            (named, "Addressing { rule: false, lookup: true }"),
        ] {
            let shown = format!("{options:?}");
            assert!(shown.contains(said), "expected {said:?} in {shown}");
        }
    }

    /// The variable naming the origin port; set only on the proxy test's child.
    const CHILD_PORT: &str = "LGWKS_ADDRESS_CHILD_PORT";
    /// The variable saying whether the child sets a rule.
    const CHILD_RULED: &str = "LGWKS_ADDRESS_CHILD_RULED";

    /// The proxy test's child: one exchange in a process whose environment
    /// names a proxy. It does nothing in an ordinary run.
    #[test]
    fn the_exchange_a_proxied_child_makes() -> Outcome {
        let Ok(port) = std::env::var(CHILD_PORT) else {
            return Ok(());
        };
        let url = format!("http://service.test:{port}/");
        if std::env::var(CHILD_RULED)? == "yes" {
            let options = quick()
                .address_rule(|address| address.is_loopback())
                .lookup(|_name| Ok(vec![V4_LOOPBACK]));
            assert_eq!(
                http::get_with(&url, &options)?.body(),
                b"origin",
                "the ruled call dials the judged address itself"
            );
        } else {
            let options = Options::default().timeout(Duration::from_millis(500));
            let answer = http::get_with(&url, &options);
            assert!(
                answer.is_err(),
                "the silent proxy answers nothing: {answer:?}"
            );
        }
        Ok(())
    }

    /// Run [`the_exchange_a_proxied_child_makes`] with every proxy variable
    /// naming `proxy`.
    fn proxied_child(origin: u16, proxy: u16, ruled: &str) -> Outcome {
        let mut command = lgwks_std::process::Command::new(std::env::current_exe()?);
        command
            .args([
                "--exact",
                "sim_http_address::sim::the_exchange_a_proxied_child_makes",
                "--test-threads=1",
            ])
            .env(CHILD_PORT, origin.to_string())
            .env(CHILD_RULED, ruled);
        for variable in ["NO_PROXY", "ALL_PROXY", "HTTPS_PROXY", "HTTP_PROXY"] {
            command.env_remove(variable.to_ascii_lowercase());
            command.env_remove(variable);
        }
        command.env("HTTP_PROXY", format!("http://127.0.0.1:{proxy}"));
        let output = command.output()?;
        let said = String::from_utf8_lossy(&output.stdout);
        assert!(output.status.success(), "ruled={ruled}: {said}");
        assert!(said.contains("1 passed"), "the child test ran: {said}");
        Ok(())
    }

    #[test]
    fn the_rule_turns_the_environment_proxy_off() -> Outcome {
        let (origin, origin_port) = listen(V4_LOOPBACK)?;
        let (proxy, proxy_port) = listen(V4_LOOPBACK)?;
        let (ran, heads) = served(&origin, &[ok("origin")], || {
            proxied_child(origin_port, proxy_port, "yes")
        })?;
        ran?;
        assert_eq!(heads.len(), 1, "the origin served the ruled call");
        assert!(!was_reached(&proxy)?, "the ruled call went to the proxy");
        // The control: with no rule the proxy variable still applies.
        proxied_child(origin_port, proxy_port, "no")?;
        assert!(was_reached(&proxy)?, "without a rule the proxy is used");
        Ok(())
    }
}
