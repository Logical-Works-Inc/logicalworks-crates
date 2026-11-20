//! The two shapes a completed exchange is read through: [`Response`], whose body
//! arrived with its head, and [`Head`], the part a streamed response shares.

use super::{Error, FailureCause, FailureKind, FailureStage, Truncation};

/// A completed HTTP exchange: status, headers, and body.
///
/// # Examples
///
/// ```no_run
/// let response = lgwks_std::http::get("https://example.com/")?;
/// println!("{} from {}", response.status(), response.final_target());
/// # Ok::<(), lgwks_std::http::Error>(())
/// ```
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct Response {
    /// HTTP status code, including 4xx/5xx.
    pub(super) status: u16,
    /// HTTP response headers.
    pub(super) headers: Vec<(String, String)>,
    /// Final request target without userinfo, query or fragment.
    pub(super) final_target: String,
    /// Redirect targets, including original and final, with userinfo, query
    /// and fragment removed.
    pub(super) redirect_chain: Vec<String>,
    /// Exact legal header value bytes with multiplicity, in upstream map order.
    pub(super) header_bytes: Vec<(String, Vec<u8>)>,
    /// Response body bytes, at most
    /// [`Options::max_body_bytes`](super::Options::max_body_bytes) long.
    pub(super) body: Vec<u8>,
    /// Whether [`Response::body`] is the whole body or a prefix of it.
    pub(super) truncation: Truncation,
}

impl Response {
    /// HTTP status code, including 4xx/5xx.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// let response = lgwks_std::http::get("https://example.com/")?;
    /// let found = response.status() == 200;
    /// # Ok::<(), lgwks_std::http::Error>(())
    /// ```
    #[must_use]
    pub const fn status(&self) -> u16 {
        self.status
    }

    /// Whether [`Response::body`] is the whole body or a prefix of it.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use lgwks_std::http::Truncation;
    ///
    /// let response = lgwks_std::http::get("https://example.com/")?;
    /// let whole = response.truncation() == Truncation::Complete;
    /// # Ok::<(), lgwks_std::http::Error>(())
    /// ```
    #[must_use]
    pub const fn truncation(&self) -> Truncation {
        self.truncation
    }

    /// Compatibility projection of headers. Values are lossy UTF-8 text and
    /// order follows the upstream `HeaderMap`, not wire arrival order.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// let response = lgwks_std::http::get("https://example.com/")?;
    /// let typed = response.headers().iter().any(|(name, _)| name == "content-type");
    /// # Ok::<(), lgwks_std::http::Error>(())
    /// ```
    #[must_use]
    pub fn headers(&self) -> &[(String, String)] {
        &self.headers
    }

    /// Iterate header names and exact legal value bytes, preserving repeated
    /// values. Order is upstream `HeaderMap` iteration order. Each item is a
    /// header name and its exact legal value bytes, so a repeated header yields
    /// one item per value.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// let response = lgwks_std::http::get("https://example.com/")?;
    /// let cookies = response.header_values().filter(|(name, _)| *name == "set-cookie").count();
    /// # Ok::<(), lgwks_std::http::Error>(())
    /// ```
    pub fn header_values(&self) -> impl ExactSizeIterator<Item = (&str, &[u8])> {
        self.header_bytes
            .iter()
            .map(|header| (header.0.as_str(), header.1.as_slice()))
    }

    /// The sanitized final request target after any followed redirects.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// let response = lgwks_std::http::get("https://example.com/")?;
    /// let landed = response.final_target().starts_with("https://");
    /// # Ok::<(), lgwks_std::http::Error>(())
    /// ```
    #[must_use]
    pub fn final_target(&self) -> &str {
        &self.final_target
    }

    /// Sanitized redirect history, including original and final targets.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// let response = lgwks_std::http::get("https://example.com/")?;
    /// let hops = response.redirect_chain().len().saturating_sub(1);
    /// # Ok::<(), lgwks_std::http::Error>(())
    /// ```
    #[must_use]
    pub fn redirect_chain(&self) -> &[String] {
        &self.redirect_chain
    }

    /// The body bytes, whole or previewed according to [`Response::truncation`].
    ///
    /// # Examples
    ///
    /// ```no_run
    /// let response = lgwks_std::http::get("https://example.com/")?;
    /// let size = response.body().len();
    /// # Ok::<(), lgwks_std::http::Error>(())
    /// ```
    #[must_use]
    pub fn body(&self) -> &[u8] {
        &self.body
    }

    /// The body as UTF-8, or a `TextDecode` / `InvalidUtf8` failure when it is
    /// not valid UTF-8. Decoding failure does not imply exchange failure.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// let response = lgwks_std::http::get("https://example.com/")?;
    /// let page: &str = response.text()?;
    /// # Ok::<(), lgwks_std::http::Error>(())
    /// ```
    pub fn text(&self) -> Result<&str, Error> {
        std::str::from_utf8(&self.body).map_err(|utf8_error| Error::Failure {
            stage: FailureStage::TextDecode,
            kind: FailureKind::InvalidUtf8,
            cause: Some(FailureCause::sanitized(format!(
                "valid UTF-8 ends at byte {}",
                utf8_error.valid_up_to()
            ))),
        })
    }
}

/// The part of a completed exchange that does not depend on how the body
/// arrives: status, headers, and provenance.
///
/// [`Response`] carries these fields inline, because its body arrives with
/// them in one value. [`StreamResponse`](super::StreamResponse) carries them
/// in this struct, because its body is still on the wire when the head is
/// read. Both answer the same four questions the same way, so a caller that
/// only needs the head never learns which shape the body took.
pub(super) struct Head {
    /// HTTP response headers.
    pub(super) headers: Vec<(String, String)>,
    /// Final request target without userinfo, query or fragment.
    pub(super) final_target: String,
    /// Redirect targets, including original and final, with userinfo, query
    /// and fragment removed.
    pub(super) redirect_chain: Vec<String>,
    /// Exact legal header value bytes with multiplicity, in upstream map order.
    pub(super) header_bytes: Vec<(String, Vec<u8>)>,
}

impl Head {
    /// Compatibility projection of headers. Values are lossy UTF-8 text and
    /// order follows the upstream `HeaderMap`, not wire arrival order.
    pub(super) fn headers(&self) -> &[(String, String)] {
        &self.headers
    }

    /// Iterate header names and exact legal value bytes, preserving repeated
    /// values. Order is upstream `HeaderMap` iteration order. Each item is a
    /// header name and its exact legal value bytes, so a repeated header yields
    /// one item per value.
    pub(super) fn header_values(&self) -> impl ExactSizeIterator<Item = (&str, &[u8])> {
        self.header_bytes
            .iter()
            .map(|header| (header.0.as_str(), header.1.as_slice()))
    }

    /// The sanitized final request target after any followed redirects.
    pub(super) fn final_target(&self) -> &str {
        &self.final_target
    }

    /// Sanitized redirect history, including original and final targets.
    pub(super) fn redirect_chain(&self) -> &[String] {
        &self.redirect_chain
    }
}
