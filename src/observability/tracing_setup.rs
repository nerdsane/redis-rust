//! Tracing and APM Setup
//!
//! Initializes tracing-subscriber with OpenTelemetry for Datadog APM.

use opentelemetry_datadog::DatadogPropagator;
use opentelemetry_sdk::trace::Sampler;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::Layer;
use tracing_subscriber::filter;

use super::config::DatadogConfig;

/// Initialize the complete observability stack
///
/// Sets up:
/// - OpenTelemetry with Datadog exporter for distributed tracing
/// - tracing-subscriber with environment-based filtering
/// - Gossip/replication spans are excluded from OTel to prevent unbounded
///   memory growth from the batch exporter accumulating high-frequency spans
pub fn init(config: &DatadogConfig) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // Set global propagator for distributed tracing context
    opentelemetry::global::set_text_map_propagator(DatadogPropagator::default());

    // Create Datadog exporter pipeline - returns a Tracer directly
    let tracer = opentelemetry_datadog::new_pipeline()
        .with_service_name(&config.service_name)
        .with_agent_endpoint(&config.trace_addr)
        .with_trace_config(
            opentelemetry_sdk::trace::Config::default()
                .with_sampler(Sampler::TraceIdRatioBased(config.trace_sample_rate))
                .with_resource(opentelemetry_sdk::Resource::new(vec![
                    opentelemetry::KeyValue::new("service.name", config.service_name.clone()),
                    opentelemetry::KeyValue::new("service.version", config.version.clone()),
                    opentelemetry::KeyValue::new("deployment.environment", config.env.clone()),
                ])),
        )
        .install_batch(opentelemetry_sdk::runtime::Tokio)?;

    // Create OpenTelemetry tracing layer with a target filter that excludes
    // high-frequency gossip/replication modules. These produce spans at 1-10Hz
    // per peer which the OTel batch exporter accumulates faster than the DD
    // agent can drain, causing unbounded memory growth (200Mi/min).
    let otel_layer = tracing_opentelemetry::layer()
        .with_tracer(tracer)
        .with_filter(
            filter::Targets::new()
                .with_default(tracing::Level::INFO)
                .with_target("redis_sim::production::gossip_manager", tracing::Level::ERROR)
                .with_target("redis_sim::production::gossip_actor", tracing::Level::ERROR)
                .with_target("redis_sim::replication::gossip", tracing::Level::ERROR)
                .with_target("redis_sim::replication::anti_entropy", tracing::Level::ERROR)
        );

    // Environment filter for log levels (applies to fmt layer — gossip still logs to stdout)
    let env_filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));

    // Build subscriber with all layers
    // fmt layer: logs everything per env_filter (including gossip — visible in kubectl logs)
    // otel layer: excludes gossip targets (prevents span accumulation OOM)
    tracing_subscriber::registry()
        .with(env_filter)
        .with(tracing_subscriber::fmt::layer())
        .with(otel_layer)
        .init();

    tracing::info!(
        service = %config.service_name,
        env = %config.env,
        version = %config.version,
        sample_rate = %config.trace_sample_rate,
        "Datadog observability initialized"
    );

    Ok(())
}

/// Shutdown tracing gracefully
///
/// Flushes any pending spans to the Datadog agent.
/// Should be called before application exit.
pub fn shutdown() {
    tracing::info!("Shutting down Datadog tracing...");
    opentelemetry::global::shutdown_tracer_provider();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_creation() {
        // Just verify config can be created without panicking
        let _config = DatadogConfig::from_env();
    }
}
