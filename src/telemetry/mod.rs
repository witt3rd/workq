//! OpenTelemetry initialization and configuration.
//!
//! Sets up tracing-subscriber with an OTel layer. If an OTLP endpoint
//! is configured, exports traces there; otherwise uses stdout for dev.

pub mod genai;
pub mod work;

use crate::error::{Error, Result};

/// Configuration for telemetry initialization.
pub struct TelemetryConfig {
    /// Optional OTLP endpoint (e.g. "http://localhost:4317").
    /// When `None`, telemetry uses a simple fmt layer for local dev.
    pub endpoint: Option<String>,
    /// The service name reported in traces.
    pub service_name: String,
}

/// Guard that shuts down the OTel tracer provider on drop.
///
/// Must be held for the lifetime of the application. When dropped,
/// the OTel pipeline is flushed and shut down.
pub struct TelemetryGuard {
    provider: Option<opentelemetry_sdk::trace::SdkTracerProvider>,
}

impl Drop for TelemetryGuard {
    fn drop(&mut self) {
        if let Some(provider) = self.provider.take() {
            let _ = provider.shutdown();
        }
    }
}

/// Initialize telemetry (tracing + OTel).
///
/// Returns a guard that must be held for the lifetime of the application.
/// When the guard is dropped, the OTel pipeline is flushed and shut down.
///
/// # Errors
///
/// Returns an error if the OTLP exporter fails to build or the tracing
/// subscriber cannot be initialized (e.g. if one was already set).
pub fn init_telemetry(config: TelemetryConfig) -> Result<TelemetryGuard> {
    use opentelemetry::trace::TracerProvider as _;
    use tracing_subscriber::EnvFilter;
    use tracing_subscriber::layer::SubscriberExt as _;
    use tracing_subscriber::util::SubscriberInitExt as _;

    let env_filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));

    if let Some(endpoint) = config.endpoint {
        // OTLP exporter configured — set up the full pipeline.
        use opentelemetry_otlp::WithExportConfig as _;

        let exporter = opentelemetry_otlp::SpanExporter::builder()
            .with_tonic()
            .with_endpoint(endpoint)
            .build()
            .map_err(|e| Error::Other(format!("failed to create OTLP exporter: {e}")))?;

        let tracer_provider = opentelemetry_sdk::trace::SdkTracerProvider::builder()
            .with_batch_exporter(exporter)
            .with_resource(
                opentelemetry_sdk::Resource::builder()
                    .with_service_name(config.service_name)
                    .build(),
            )
            .build();

        let tracer = tracer_provider.tracer("animus-rs");

        let otel_layer = tracing_opentelemetry::layer().with_tracer(tracer);

        tracing_subscriber::registry()
            .with(env_filter)
            .with(otel_layer)
            .try_init()
            .map_err(|e| Error::Other(format!("failed to init tracing subscriber: {e}")))?;

        Ok(TelemetryGuard {
            provider: Some(tracer_provider),
        })
    } else {
        // No OTLP endpoint — just use tracing-subscriber with fmt.
        tracing_subscriber::registry()
            .with(env_filter)
            .with(tracing_subscriber::fmt::layer())
            .try_init()
            .map_err(|e| Error::Other(format!("failed to init tracing subscriber: {e}")))?;

        Ok(TelemetryGuard { provider: None })
    }
}
