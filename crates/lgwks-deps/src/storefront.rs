//! The supported storefront: enable a feature and reach its engine here.
//!
//! Every path below is one audited edge this crate owns, so a consumer never
//! declares the upstream crate itself. No feature is on by default; the
//! feature table on the crate page names what each one enables.

#[cfg(feature = "merkle-log")]
pub use crate::Sha256;
#[cfg(feature = "appcui")]
pub use crate::appcui;
#[cfg(feature = "axum")]
pub use crate::axum;
#[cfg(feature = "bevy-app")]
pub use crate::bevy_app;
#[cfg(feature = "bevy-ecs")]
pub use crate::bevy_ecs;
#[cfg(feature = "bevy-state")]
pub use crate::bevy_state;
#[cfg(feature = "bevy-time")]
pub use crate::bevy_time;
#[cfg(feature = "sealed-aead")]
pub use crate::chacha20poly1305;
#[cfg(feature = "cose")]
pub use crate::coset;
#[cfg(feature = "merkle-log")]
pub use crate::ct_merkle;
#[cfg(feature = "model")]
pub use crate::genai;
#[cfg(feature = "gpui")]
pub use crate::gpui;
#[cfg(feature = "time-jiff")]
pub use crate::jiff;
#[cfg(feature = "json-schema")]
pub use crate::jsonschema;
#[cfg(feature = "llm-guard")]
pub use crate::llm_guard;
#[cfg(feature = "oauth2")]
pub use crate::oauth2;
#[cfg(feature = "mcp")]
pub use crate::rmcp;
#[cfg(feature = "sqlite")]
pub use crate::rusqlite;
#[cfg(feature = "sqlite")]
pub use crate::sqlite;
#[cfg(feature = "tls-ring")]
pub use crate::tls;
#[cfg(feature = "ml-tokenizers")]
pub use crate::tokenizers;
#[cfg(feature = "tokio")]
pub use crate::tokio;
#[cfg(feature = "tower-http")]
pub use crate::tower_http;
#[cfg(feature = "ml-candle")]
pub use crate::{candle_core, candle_nn, candle_transformers};
#[cfg(feature = "otel")]
pub use crate::{
    opentelemetry, opentelemetry_otlp, opentelemetry_sdk, tracing_opentelemetry, tracing_subscriber,
};
