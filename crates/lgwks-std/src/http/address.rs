//! The address rule: every address a name answers is judged before a socket
//! opens, so a URL somebody else supplied cannot reach inward (CWE-918).
//!
//! ureq resolves each request once, before it takes a pooled connection or
//! opens one, and a new connection dials only what that call returned. So the
//! judged addresses are the dialled ones: no second lookup exists for a DNS
//! rebinding to change, and a name that turns inward is refused even with a
//! connection pooled. A proxy would resolve the name itself, so a call that sets
//! a rule or a lookup uses none.
//!
//! The resolver seam is ureq's `unversioned` module, which a minor ureq release
//! may change. The lockfile holds the release this file is built against.

use std::fmt;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;

use ureq::config::Config;
use ureq::http::Uri;
use ureq::unversioned::resolver::{DefaultResolver, ResolvedSocketAddrs, Resolver};
use ureq::unversioned::transport::{DefaultConnector, NextTimeout};

use super::exchange::map_read_error;
use super::{Error, FailureStage, Options};

/// A caller's verdict on one address: `true` admits it.
type Rule = Arc<dyn Fn(IpAddr) -> bool + Send + Sync>;

/// A caller's answer to "which addresses does this name have".
type Lookup = Arc<dyn Fn(&str) -> std::io::Result<Vec<IpAddr>> + Send + Sync>;

/// Most addresses one connection keeps, ureq's own resolver bound. A lookup's
/// answers are all judged first, those past the bound included. The system
/// resolver hands over its own first 16, so the rest are neither judged nor
/// dialled.
const MAX_DIALLED: usize = 16;

/// How an exchange chooses the addresses it dials. Both unset is ureq's own
/// resolution, unjudged, with the environment's proxy.
#[derive(Clone, Default)]
pub(super) struct Addressing {
    /// The verdict every answer must pass.
    rule: Option<Rule>,
    /// Who answers a name, in place of the system resolver.
    lookup: Option<Lookup>,
}

impl Addressing {
    /// Whether the exchange resolves its own targets, which rules out a proxy.
    pub(super) fn is_set(&self) -> bool {
        self.rule.is_some() || self.lookup.is_some()
    }

    /// Whether `other` names the very same rule and lookup. Two closures built
    /// apart are two settings, even when their code is the same.
    pub(super) fn same(&self, other: &Self) -> bool {
        same_setting(self.rule.as_ref(), other.rule.as_ref())
            && same_setting(self.lookup.as_ref(), other.lookup.as_ref())
    }

    /// `self`, with each setting it does not carry taken from `base`: a call
    /// that leaves one out never lifts it.
    pub(super) fn or(&self, base: &Self) -> Self {
        Self {
            rule: self.rule.as_ref().or(base.rule.as_ref()).map(Arc::clone),
            lookup: self
                .lookup
                .as_ref()
                .or(base.lookup.as_ref())
                .map(Arc::clone),
        }
    }

    /// The agent that dials under these settings.
    pub(super) fn agent(&self, config: Config) -> ureq::Agent {
        if !self.is_set() {
            return ureq::Agent::new_with_config(config);
        }
        ureq::Agent::with_parts(config, DefaultConnector::new(), Judged(self.clone()))
    }

    /// The addresses `uri` answers: the literal itself, the caller's lookup, or
    /// the system resolver.
    fn answers(
        &self,
        uri: &Uri,
        host: &str,
        config: &Config,
        timeout: NextTimeout,
    ) -> Result<Vec<SocketAddr>, ureq::Error> {
        let bare = host
            .strip_prefix('[')
            .and_then(|bracketed| bracketed.strip_suffix(']'));
        let literal = match bare {
            Some(inside) => inside.parse::<IpAddr>(),
            None => host.parse::<IpAddr>(),
        };
        let named = match (literal, self.lookup.as_ref()) {
            (Ok(address), _) => vec![address],
            (Err(_not_a_literal), Some(lookup)) => lookup(host).map_err(|cause| {
                #[cfg(feature = "trace")]
                crate::trace::debug!(host, %cause, "answers: the caller's lookup failed");
                ureq::Error::Other(Box::new(Unresolved::Lookup(cause)))
            })?,
            (Err(_not_a_literal), None) => {
                let system = DefaultResolver::default().resolve(uri, config, timeout)?;
                return Ok(system.iter().copied().collect());
            }
        };
        let port = port_of(uri)?;
        Ok(named
            .into_iter()
            .map(|address| SocketAddr::new(address, port))
            .collect())
    }
}

impl fmt::Debug for Addressing {
    /// Says which settings are present; a closure has nothing else to show.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Addressing")
            .field("rule", &self.rule.is_some())
            .field("lookup", &self.lookup.is_some())
            .finish()
    }
}

/// Whether two optional closures are the very same one, or both absent.
fn same_setting<T: ?Sized>(own: Option<&Arc<T>>, theirs: Option<&Arc<T>>) -> bool {
    match (own, theirs) {
        (Some(own), Some(theirs)) => Arc::ptr_eq(own, theirs),
        (None, None) => true,
        (Some(_), None) | (None, Some(_)) => false,
    }
}

/// The port `uri` is dialled on, by ureq's own default-port rule.
fn port_of(uri: &Uri) -> Result<u16, ureq::Error> {
    let port = match (uri.scheme(), uri.authority()) {
        (Some(scheme), Some(authority)) => DefaultResolver::host_and_port(scheme, authority),
        _ => None,
    };
    let parsed = port
        .as_deref()
        .and_then(|both| both.rsplit_once(':'))
        .map(|(_host, port)| port.parse::<u16>());
    match parsed {
        Some(Ok(port)) => Ok(port),
        // The URL is not carried: ureq's own `BadUri` text embeds it.
        _ => {
            #[cfg(feature = "trace")]
            crate::trace::debug!("port_of: the URL names no port to dial");
            Err(ureq::Error::HostNotFound)
        }
    }
}

/// The resolver an agent with a rule or a lookup resolves through.
#[derive(Debug)]
struct Judged(Addressing);

impl Resolver for Judged {
    fn resolve(
        &self,
        uri: &Uri,
        config: &Config,
        timeout: NextTimeout,
    ) -> Result<ResolvedSocketAddrs, ureq::Error> {
        let host = uri.host().ok_or(ureq::Error::HostNotFound)?;
        let answers = self.0.answers(uri, host, config, timeout)?;
        // One refused answer refuses the name: dialling the rest would let a
        // name that answers inward keep a way through. An IPv4-mapped IPv6
        // address is judged as the IPv4 address a socket to it reaches.
        let refused = self.0.rule.as_ref().and_then(|rule| {
            answers
                .iter()
                .map(SocketAddr::ip)
                .find(|address| !rule(address.to_canonical()))
        });
        if let Some(address) = refused {
            #[cfg(feature = "trace")]
            crate::trace::warn!(host, %address, "http: the address rule refused an answer");
            return Err(ureq::Error::Other(Box::new(Unresolved::Refused {
                host: host.to_owned(),
                address,
            })));
        }
        let mut dialled = self.empty();
        for answer in answers.into_iter().take(MAX_DIALLED) {
            dialled.push(answer);
        }
        if dialled.is_empty() {
            #[cfg(feature = "trace")]
            crate::trace::debug!(host, "resolve: the host answered no address");
            return Err(ureq::Error::HostNotFound);
        }
        Ok(dialled)
    }
}

/// Why [`Judged`] gave ureq no address, carried through `ureq::Error::Other`.
#[derive(Debug)]
pub(super) enum Unresolved {
    /// The rule refused an answer.
    Refused {
        /// The host as the URL named it.
        host: String,
        /// The refused answer, as it arrived.
        address: IpAddr,
    },
    /// The caller's lookup failed.
    Lookup(std::io::Error),
}

impl Unresolved {
    /// The refusal the caller of the exchange reads.
    pub(super) fn error(&self) -> Error {
        match *self {
            Self::Refused { ref host, address } => Error::AddressRefused {
                host: host.clone(),
                address,
            },
            Self::Lookup(ref cause) => map_read_error(cause, FailureStage::Resolve),
        }
    }
}

impl fmt::Display for Unresolved {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.error(), formatter)
    }
}

impl std::error::Error for Unresolved {}

impl Options {
    /// Judge every address before it is dialled: `rule` answers `true` for an
    /// address the exchange may reach.
    ///
    /// Use it for any URL somebody else supplied. The rule sees every address
    /// the exchange could dial, on every redirect hop, and one refused answer
    /// refuses the hop with [`Error::AddressRefused`] before a socket opens.
    /// That is every answer of a [`Options::lookup`], and the first 16 of the
    /// system resolver's, which are all ureq keeps. An IP
    /// literal in the URL is judged the same way, and an IPv4-mapped IPv6
    /// address reaches the rule as its IPv4 address. Which ranges are inward is
    /// the rule's to say; nothing here is refused without it.
    ///
    /// No proxy is used, the environment's included, because a proxy resolves
    /// the name itself. A [`Client`](super::Client) built with a rule keeps it
    /// for a call whose options carry none.
    ///
    /// ```text
    /// let options = Options::default().address_rule(|address| !address.is_loopback());
    /// get_with("http://127.0.0.1:9/", &options)  // Err(Error::AddressRefused { .. })
    /// ```
    ///
    /// `tests/it/sim_http_address.rs` drives these calls.
    #[must_use]
    pub fn address_rule(mut self, rule: impl Fn(IpAddr) -> bool + Send + Sync + 'static) -> Self {
        self.addressing.rule = Some(Arc::new(rule));
        self
    }

    /// Answer host names with `lookup` in place of the system resolver.
    ///
    /// It is called once for each request sent, every redirect hop included and
    /// pooled or not, with the host as the URL names it. An IP literal is not
    /// looked up. The answers are still judged by
    /// [`Options::address_rule`], and no proxy is used. The call's timeouts
    /// cannot interrupt it, so a lookup that can block bounds itself.
    ///
    /// ```text
    /// let options = Options::default()
    ///     .lookup(|_host| Ok(vec![IpAddr::from([169, 254, 169, 254])]))
    ///     .address_rule(is_public);
    /// get_with("http://name.example/", &options)  // Err(Error::AddressRefused { .. })
    /// ```
    ///
    /// `tests/it/sim_http_address.rs` drives these calls.
    #[must_use]
    pub fn lookup(
        mut self,
        lookup: impl Fn(&str) -> std::io::Result<Vec<IpAddr>> + Send + Sync + 'static,
    ) -> Self {
        self.addressing.lookup = Some(Arc::new(lookup));
        self
    }
}
