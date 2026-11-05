//! OpenTelemetry: a GenAI span, an OTLP exporter built, a bridged trace span, a clean shutdown.

#[path = "support/boot.rs"]
mod boot;

use lgwks_deps::opentelemetry::KeyValue;
use lgwks_deps::opentelemetry::trace::{Span, Tracer, TracerProvider};
use lgwks_deps::opentelemetry_otlp::{SpanExporter, WithExportConfig};
use lgwks_deps::opentelemetry_sdk::trace::SdkTracerProvider;
use lgwks_deps::tracing_subscriber::layer::SubscriberExt;

/// The GenAI semantic-convention keys (opentelemetry.io gen-ai spans); the
/// conventions crate deprecated its copies in 0.33, so the journey names them.
const GEN_AI_OPERATION_NAME: &str = "gen_ai.operation.name";
/// The model a GenAI request names.
const GEN_AI_REQUEST_MODEL: &str = "gen_ai.request.model";

fn main() {
    if !boot::run_with_tls(journey()) {
        std::process::exit(1);
    }
}

async fn journey() -> bool {
    let Ok(exporter) = SpanExporter::builder()
        .with_http()
        .with_endpoint("http://127.0.0.1:4318/v1/traces")
        .build()
    else {
        return false;
    };
    drop(exporter);
    let provider = SdkTracerProvider::builder().build();
    let tracer = provider.tracer("larry");
    let mut span = tracer.start("chat deepseek-chat");
    span.set_attribute(KeyValue::new(GEN_AI_OPERATION_NAME, "chat"));
    span.set_attribute(KeyValue::new(GEN_AI_REQUEST_MODEL, "deepseek-chat"));
    span.end();
    let layer =
        lgwks_deps::tracing_opentelemetry::layer::<lgwks_deps::tracing_subscriber::Registry>()
            .with_tracer(provider.tracer("larry-tracing"));
    let subscriber = lgwks_deps::tracing_subscriber::registry().with(layer);
    lgwks_std::trace::tracing::subscriber::with_default(subscriber, || {
        lgwks_std::trace::info_span!("execute_tool", gen_ai.tool.name = "add").in_scope(|| {});
    });
    provider.shutdown().is_ok()
}
