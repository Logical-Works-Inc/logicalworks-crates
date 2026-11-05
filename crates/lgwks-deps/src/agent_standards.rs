//! The agent-standards storefront (ADR 0015, #869): one audited edge per open
//! protocol an agent harness speaks, each behind its own default-off feature,
//! re-exported at the crate root so a consumer names `lgwks_deps::rmcp`, never
//! the upstream crate.

/// One in-process TLS provider for every agent-standards client.
///
/// The clients behind `mcp`, `model` and `otel` build reqwest with
/// `rustls-no-provider`, so the process names its provider once before the
/// first client; [`tls::install_ring`] is that one call. Ring is the provider
/// the sealed #728 exception in `contract/PATCHES.toml` already excuses, so no
/// second provider and no C toolchain enters the tree.
#[cfg(feature = "tls-ring")]
pub mod tls {
    /// Why the process provider could not be set to ring.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use lgwks_deps::storefront::tls::{TlsError, install_ring};
    ///
    /// let refused = TlsError::OtherProviderInstalled;
    /// assert!(refused.to_string().contains("other than ring"));
    /// assert_eq!(install_ring(), Ok(()));
    /// ```
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    #[non_exhaustive]
    pub enum TlsError {
        /// Another provider was installed first, and it is not ring.
        OtherProviderInstalled,
    }

    impl std::fmt::Display for TlsError {
        fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            match *self {
                Self::OtherProviderInstalled => formatter.write_str(
                    "a TLS crypto provider other than ring is already installed for this process",
                ),
            }
        }
    }

    impl std::error::Error for TlsError {}

    /// Install ring as the process TLS provider; a second call is a no-op.
    ///
    /// The `model` and `otel` journeys of `tests/fixtures/storefront-matrix`
    /// call it twice and require both calls to succeed.
    ///
    /// # Errors
    ///
    /// [`TlsError::OtherProviderInstalled`] when a different provider was
    /// installed first: two providers in one process would make which one a
    /// handshake uses depend on call order.
    ///
    /// # Examples
    ///
    /// ```rust
    /// // Once per process, before the first MCP, model or OTLP client.
    /// lgwks_deps::storefront::tls::install_ring()?;
    /// // A second call finds ring already installed and succeeds.
    /// lgwks_deps::storefront::tls::install_ring()?;
    /// # Ok::<(), lgwks_deps::storefront::tls::TlsError>(())
    /// ```
    pub fn install_ring() -> Result<(), TlsError> {
        let ring = rustls::crypto::ring::default_provider();
        let wanted: Vec<_> = ring
            .cipher_suites
            .iter()
            .map(|suite| suite.suite())
            .collect();
        if ring.install_default().is_ok() {
            return Ok(());
        }
        // A refused install with no provider readable back is no proof of
        // ring, so it is refused like a foreign provider.
        let same = match rustls::crypto::CryptoProvider::get_default() {
            Some(installed) => installed
                .cipher_suites
                .iter()
                .map(|suite| suite.suite())
                .eq(wanted.iter().copied()),
            None => false,
        };
        if same {
            Ok(())
        } else {
            Err(TlsError::OtherProviderInstalled)
        }
    }
}

/// The Model Context Protocol SDK, selected through the `mcp` feature.
///
/// Hand-written `ServerHandler` and `ClientHandler` impls work through this
/// path; the upstream `macros` feature is declined because its expansions name
/// `::rmcp::` (ADR 0015). `storefront_mcp_runs_its_journey` serves and calls a
/// tool over an in-memory pipe through this path.
#[cfg(feature = "mcp")]
pub use rmcp;

/// One client for every model provider, selected through the `model` feature.
///
/// Its HTTP rides the ring provider of [`tls`]; call `tls::install_ring` once
/// before the first request. `storefront_model_runs_its_journey` resolves
/// providers offline through this path.
#[cfg(feature = "model")]
pub use genai;

/// JSON Schema 2020-12 validation, selected through the `json-schema` feature.
///
/// Remote `$ref` resolution is off (default features off); a schema is
/// validated against what it carries. `storefront_json_schema_runs_its_journey`
/// admits a good tool call and refuses four bad ones through this path.
#[cfg(feature = "json-schema")]
pub use jsonschema;

/// The OAuth 2.1 authorization-code flow with PKCE, selected through `oauth2`.
///
/// Default features off: no HTTP client of its own, so the token exchange runs
/// through a client the caller names. `storefront_oauth2_runs_its_journey`
/// builds two tenants' authorization URLs through this path.
#[cfg(feature = "oauth2")]
pub use oauth2;

/// COSE_Sign1 envelopes (RFC 9052), selected through the `cose` feature.
///
/// The envelope is the standard; the signature is `lgwks_std::sign`'s ed25519
/// seal, so the estate keeps one signer. `storefront_cose_runs_its_journey`
/// seals, reads and verifies an envelope through this path.
#[cfg(feature = "cose")]
pub use coset;

/// The RFC 6962 append-only log, selected through the `merkle-log` feature.
///
/// The tree is generic over its `digest::Digest` hasher; the SHA-256 type
/// RFC 6962 names lands with its own approval, since the estate's content hash
/// is `lgwks_std::hash` (BLAKE3). `storefront_merkle_log_runs_its_journey`
/// checks inclusion and consistency proofs through this path.
#[cfg(feature = "merkle-log")]
pub use ct_merkle;

/// The OpenTelemetry API, selected through the `otel` feature.
#[cfg(feature = "otel")]
pub use opentelemetry;

/// The OpenTelemetry SDK: providers, processors and the tokio exporter runtime.
#[cfg(feature = "otel")]
pub use opentelemetry_sdk;

/// The OTLP exporter over HTTP/protobuf, on the ring provider of [`tls`].
#[cfg(feature = "otel")]
pub use opentelemetry_otlp;

/// The bridge from `lgwks_std::trace` spans to OpenTelemetry spans.
///
/// `storefront_otel_runs_its_journey` records a GenAI span, bridges a
/// `tracing` span and shuts the provider down through this path.
#[cfg(feature = "otel")]
pub use tracing_opentelemetry;

/// The subscriber registry the OpenTelemetry bridge layers onto.
///
/// The same `tracing-subscriber` that `lgwks_std`'s `trace-install` resolves, so
/// no second copy enters the lockfile; only `registry` and `std` are selected.
#[cfg(feature = "otel")]
pub use tracing_subscriber;
