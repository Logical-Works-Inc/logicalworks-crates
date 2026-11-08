//! Who a request is, decided at the edge before it spends a mailbox slot (#880).
//!
//! An app that calls [`App::authenticate`](super::App::authenticate) has every
//! tenant request judged by that function on the driver, after the tenant name is
//! read and before the body is. A refusal is `401` with nothing read and nothing
//! queued. An accepted request carries only its [`Principal`] into the cell, so a key
//! never reaches a tenant's log.

use std::sync::Arc;

use lgwks_deps::axum::http::HeaderMap;

/// A request head carries at most this many headers past the edge's judgement.
const MAX_HEADERS: usize = 64;

/// A request head carries at most this many name and value bytes.
const MAX_HEADER_BYTES: usize = 16 * 1_024;

/// A principal's name is 1..=256 bytes.
const MAX_PRINCIPAL_BYTES: usize = 256;

/// The function that decides who a request is.
pub(super) type Authenticate =
    Arc<dyn Fn(&str, &RequestHeaders<'_>) -> Option<Principal> + Send + Sync>;

/// Whom an authenticator says a request came from.
///
/// # Examples
///
/// ```
/// use lgwks_bot::app::Principal;
///
/// let name = Principal::new("billing-service").map(|who| who.as_str().len());
/// assert_eq!(name, Some(15));
/// assert!(Principal::new("").is_none());
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Principal(Box<str>);

impl Principal {
    /// A principal named `name`, or `None` for an empty name or one past 256 bytes.
    ///
    /// A long name is refused rather than clipped: two names clipped to one would be
    /// one identity.
    ///
    /// # Examples
    ///
    /// ```
    /// use lgwks_bot::app::Principal;
    ///
    /// assert!(Principal::new("ok").is_some());
    /// assert!(Principal::new(&"x".repeat(257)).is_none());
    /// ```
    #[must_use]
    pub fn new(name: &str) -> Option<Self> {
        if name.is_empty() || name.len() > MAX_PRINCIPAL_BYTES {
            lgwks_std::trace::debug!(
                bytes = name.len(),
                "app: a principal's name is out of bounds"
            );
            return None;
        }
        Some(Self(name.into()))
    }

    /// The name.
    ///
    /// # Examples
    ///
    /// ```
    /// use lgwks_bot::app::Principal;
    ///
    /// fn is_billing(who: &Principal) -> bool {
    ///     who.as_str() == "billing-service"
    /// }
    /// ```
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The principal `App::check` probes a handler with.
    pub(super) fn probe() -> Self {
        Self("app.probe".into())
    }

    /// The bytes this principal adds to a message.
    pub(super) fn weight(&self) -> usize {
        self.0.len()
    }
}

/// A read-only view of one request's headers, for an authenticator.
///
/// # Examples
///
/// ```
/// use lgwks_bot::app::RequestHeaders;
///
/// fn key<'request>(headers: &RequestHeaders<'request>) -> Option<&'request str> {
///     headers.bearer()
/// }
/// ```
#[derive(Debug, Clone, Copy)]
pub struct RequestHeaders<'request> {
    /// The headers as the edge read them.
    map: &'request HeaderMap,
}

impl<'request> RequestHeaders<'request> {
    /// The view over `map`, or `None` when the head is past the edge's bounds.
    pub(super) fn bounded(map: &'request HeaderMap) -> Option<Self> {
        let mut bytes = 0_usize;
        for (name, value) in map {
            bytes = bytes
                .saturating_add(name.as_str().len())
                .saturating_add(value.len());
        }
        if map.len() > MAX_HEADERS || bytes > MAX_HEADER_BYTES {
            lgwks_std::trace::debug!(
                count = map.len(),
                bytes,
                "app: a request head is over the edge's bounds"
            );
            return None;
        }
        Some(Self { map })
    }

    /// The one value of the header `name`, in any case.
    ///
    /// A header sent twice, or with bytes that are not text, reads as absent. A proxy
    /// and this edge could disagree about which of two counts, so neither does.
    ///
    /// # Examples
    ///
    /// ```
    /// use lgwks_bot::app::RequestHeaders;
    ///
    /// fn api_key<'request>(headers: &RequestHeaders<'request>) -> Option<&'request str> {
    ///     headers.get("x-api-key")
    /// }
    /// ```
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&'request str> {
        let mut values = self.map.get_all(name).iter();
        let value = values.next()?;
        if values.next().is_some() {
            lgwks_std::trace::debug!(name, "app: a header was sent twice");
            return None;
        }
        value.to_str().ok()
    }

    /// The token of an `Authorization: Bearer` header (RFC 6750 section 2.1).
    ///
    /// # Examples
    ///
    /// ```
    /// use lgwks_bot::app::RequestHeaders;
    ///
    /// fn has_key(headers: &RequestHeaders<'_>) -> bool {
    ///     headers.bearer().is_some()
    /// }
    /// ```
    #[must_use]
    pub fn bearer(&self) -> Option<&'request str> {
        let (scheme, token) = self.get("authorization")?.split_once(' ')?;
        let well_formed =
            scheme.eq_ignore_ascii_case("bearer") && !token.is_empty() && !token.contains(' ');
        well_formed.then_some(token)
    }
}
