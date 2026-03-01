//! OpenTelemetry initialization and configuration.
//!
//! Sets up tracing-subscriber with OTel layers. If an OTLP endpoint
//! is configured, exports traces, metrics, and logs there; otherwise
//! uses stdout for dev.

pub mod genai;
pub mod metrics;
pub mod work;

use crate::error::{Error, Result};

/// Configuration for telemetry initialization.
pub struct TelemetryConfig {
    /// Optional OTLP endpoint (e.g. "http://localhost:4317").
    /// When `None`, telemetry uses a simple fmt layer for local dev.
    pub endpoint: Option<String>,
    /// The service name reported in telemetry signals.
    pub service_name: String,
}

/// Guard that shuts down OTel providers on drop.
///
/// Must be held for the lifetime of the application. When dropped,
/// all OTel pipelines are flushed and shut down.
pub struct TelemetryGuard {
    tracer_provider: Option<opentelemetry_sdk::trace::SdkTracerProvider>,
    meter_provider: Option<opentelemetry_sdk::metrics::SdkMeterProvider>,
    logger_provider: Option<opentelemetry_sdk::logs::SdkLoggerProvider>,
}

impl TelemetryGuard {
    /// Force-flush all telemetry pipelines.
    ///
    /// Useful in tests to ensure data is exported before querying backends.
    pub fn force_flush(&self) {
        if let Some(ref provider) = self.tracer_provider {
            let _ = provider.force_flush();
        }
        if let Some(ref provider) = self.meter_provider {
            let _ = provider.force_flush();
        }
        if let Some(ref provider) = self.logger_provider {
            let _ = provider.force_flush();
        }
    }
}

impl Drop for TelemetryGuard {
    fn drop(&mut self) {
        if let Some(provider) = self.logger_provider.take() {
            let _ = provider.shutdown();
        }
        if let Some(provider) = self.meter_provider.take() {
            let _ = provider.shutdown();
        }
        if let Some(provider) = self.tracer_provider.take() {
            let _ = provider.shutdown();
        }
    }
}

/// Initialize telemetry (tracing + metrics + logs via OTel).
///
/// Returns a guard that must be held for the lifetime of the application.
/// When the guard is dropped, all OTel pipelines are flushed and shut down.
///
/// # Errors
///
/// Returns an error if any OTLP exporter fails to build or the tracing
/// subscriber cannot be initialized (e.g. if one was already set).
pub fn init_telemetry(config: TelemetryConfig) -> Result<TelemetryGuard> {
    use opentelemetry::trace::TracerProvider as _;
    use tracing_subscriber::EnvFilter;
    use tracing_subscriber::layer::SubscriberExt as _;
    use tracing_subscriber::util::SubscriberInitExt as _;

    let env_filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));

    if let Some(endpoint) = config.endpoint {
        // OTLP endpoint configured — set up all three pipelines.
        use opentelemetry_otlp::WithExportConfig as _;

        let resource = opentelemetry_sdk::Resource::builder()
            .with_service_name(config.service_name)
            .build();

        // --- Traces ---
        let span_exporter = opentelemetry_otlp::SpanExporter::builder()
            .with_tonic()
            .with_endpoint(&endpoint)
            .build()
            .map_err(|e| Error::Other(format!("failed to create OTLP span exporter: {e}")))?;

        let tracer_provider = opentelemetry_sdk::trace::SdkTracerProvider::builder()
            .with_batch_exporter(span_exporter)
            .with_resource(resource.clone())
            .build();

        let tracer = tracer_provider.tracer("animus-rs");
        let otel_trace_layer = tracing_opentelemetry::layer().with_tracer(tracer);

        // --- Metrics ---
        let metric_exporter = opentelemetry_otlp::MetricExporter::builder()
            .with_tonic()
            .with_endpoint(&endpoint)
            .build()
            .map_err(|e| Error::Other(format!("failed to create OTLP metric exporter: {e}")))?;

        let meter_provider = opentelemetry_sdk::metrics::SdkMeterProvider::builder()
            .with_periodic_exporter(metric_exporter)
            .with_resource(resource.clone())
            .build();

        opentelemetry::global::set_meter_provider(meter_provider.clone());

        // --- Logs ---
        let log_exporter = opentelemetry_otlp::LogExporter::builder()
            .with_tonic()
            .with_endpoint(&endpoint)
            .build()
            .map_err(|e| Error::Other(format!("failed to create OTLP log exporter: {e}")))?;

        let logger_provider = opentelemetry_sdk::logs::SdkLoggerProvider::builder()
            .with_batch_exporter(log_exporter)
            .with_resource(resource)
            .build();

        let otel_log_layer = opentelemetry_appender_tracing::layer::OpenTelemetryTracingBridge::new(
            &logger_provider,
        );

        // --- Subscriber ---
        // Both OTel export AND stderr output — see what the agent is doing
        tracing_subscriber::registry()
            .with(env_filter)
            .with(tracing_subscriber::fmt::layer().compact())
            .with(otel_trace_layer)
            .with(otel_log_layer)
            .try_init()
            .map_err(|e| Error::Other(format!("failed to init tracing subscriber: {e}")))?;

        Ok(TelemetryGuard {
            tracer_provider: Some(tracer_provider),
            meter_provider: Some(meter_provider),
            logger_provider: Some(logger_provider),
        })
    } else {
        // No OTLP endpoint — just use tracing-subscriber with fmt.
        tracing_subscriber::registry()
            .with(env_filter)
            .with(tracing_subscriber::fmt::layer())
            .try_init()
            .map_err(|e| Error::Other(format!("failed to init tracing subscriber: {e}")))?;

        Ok(TelemetryGuard {
            tracer_provider: None,
            meter_provider: None,
            logger_provider: None,
        })
    }
}
