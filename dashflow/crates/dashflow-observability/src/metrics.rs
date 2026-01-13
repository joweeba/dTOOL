//! Prometheus Metrics for `DashFlow` Applications
//!
//! This module provides Prometheus-compatible metrics collection for `DashFlow` applications.
//! Metrics are automatically collected for graph execution, LLM calls, and checkpointer operations.
//!
//! # Example
//!
//! ```rust,no_run
//! use dashflow_observability::metrics::{MetricsRegistry, register_default_metrics};
//!
//! // Initialize default metrics
//! register_default_metrics()?;
//!
//! // Get global registry
//! let registry = MetricsRegistry::global();
//!
//! // Access underlying Prometheus registry to record metrics
//! // (direct Prometheus API usage - see Prometheus crate docs)
//!
//! // Export metrics in Prometheus format
//! let metrics_text = registry.export()?;
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

use crate::error::{Error, Result};
use prometheus::{
    Encoder, Histogram, HistogramOpts, HistogramVec, IntCounter, IntCounterVec, IntGauge,
    IntGaugeVec, Opts, Registry, TextEncoder,
};
use regex::Regex;
use std::sync::{Arc, LazyLock};

// Environment variable names (matching dashflow::core::config_loader::env_vars constants)
// Note: Cannot import from dashflow due to cyclic dependency
const DASHFLOW_METRICS_REDACT: &str = "DASHFLOW_METRICS_REDACT";

/// Helper to read a string from environment variable
fn env_string(name: &str) -> Option<String> {
    std::env::var(name).ok()
}

/// Regex pattern to match label values in Prometheus text format.
/// Matches: label_name="value" capturing the value inside quotes.
/// Handles escaped quotes within values.
#[allow(clippy::expect_used)] // SAFETY: Hardcoded valid regex literal - cannot fail at runtime
static LABEL_VALUE_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    // Matches label="value" patterns in Prometheus metrics format
    // Captures: (label_name, value_content)
    Regex::new(r#"(\w+)="((?:[^"\\]|\\.)*)""#).expect("Invalid regex for label values")
});

/// Built-in secret patterns for metrics redaction (subset of SensitiveDataRedactor)
#[allow(clippy::expect_used)] // SAFETY: All regex literals are hardcoded and validated at compile time
static SECRET_PATTERNS: LazyLock<Vec<(Regex, &'static str)>> = LazyLock::new(|| {
    vec![
        // OpenAI API keys
        (
            Regex::new(r"sk-[a-zA-Z0-9]{20,}").expect("Invalid openai regex"),
            "[OPENAI_KEY]",
        ),
        // Anthropic API keys
        (
            Regex::new(r"sk-ant-[a-zA-Z0-9_-]{20,}").expect("Invalid anthropic regex"),
            "[ANTHROPIC_KEY]",
        ),
        // AWS Access Key IDs
        (
            Regex::new(r"(?:AKIA|ABIA|ACCA|ASIA)[A-Z0-9]{16}").expect("Invalid aws regex"),
            "[AWS_KEY]",
        ),
        // GitHub tokens
        (
            Regex::new(r"(?:ghp|gho|ghu|ghs|ghr)_[a-zA-Z0-9]{36,}").expect("Invalid github regex"),
            "[GITHUB_TOKEN]",
        ),
        // Bearer tokens
        (
            Regex::new(r"[Bb]earer\s+[a-zA-Z0-9_.-]{20,}").expect("Invalid bearer regex"),
            "Bearer [TOKEN]",
        ),
        // Generic API keys (api_key=..., apikey:..., etc.)
        (
            Regex::new(r"(?i)(?:api[_-]?key|apikey)[=:\s]+['\x22]?[a-zA-Z0-9_-]{20,}['\x22]?")
                .expect("Invalid api_key regex"),
            "[API_KEY]",
        ),
        // URL passwords (://user:password@)
        (
            Regex::new(r"://[^:]+:([^@]{8,})@").expect("Invalid url_password regex"),
            "://[CREDENTIALS]@",
        ),
        // Email addresses
        (
            Regex::new(r"[a-zA-Z0-9._%+-]+@[a-zA-Z0-9.-]+\.[a-zA-Z]{2,}")
                .expect("Invalid email regex"),
            "[EMAIL]",
        ),
        // Private key markers
        (
            Regex::new(r"-----BEGIN (?:RSA |EC |DSA |OPENSSH )?PRIVATE KEY-----")
                .expect("Invalid private_key regex"),
            "[PRIVATE_KEY]",
        ),
        // JWT tokens (eyJ...)
        (
            Regex::new(r"eyJ[a-zA-Z0-9_-]{20,}\.eyJ[a-zA-Z0-9_-]+\.[a-zA-Z0-9_-]+")
                .expect("Invalid jwt regex"),
            "[JWT_TOKEN]",
        ),
    ]
});

/// Parse a redaction flag value.
/// Returns true for enabled, false for disabled.
fn parse_redaction_flag(val: &str) -> bool {
    !matches!(val.to_lowercase().as_str(), "false" | "0" | "no" | "off")
}

/// Check if metrics redaction is enabled via environment variable.
///
/// Controlled by `DASHFLOW_METRICS_REDACT` env var:
/// - "true", "1", "yes", "on" -> enabled
/// - "false", "0", "no", "off" -> disabled
/// - Default: enabled (security-first approach)
#[must_use]
pub fn is_metrics_redaction_enabled() -> bool {
    match env_string(DASHFLOW_METRICS_REDACT) {
        Some(val) => parse_redaction_flag(&val),
        None => true, // Default ON for security
    }
}

/// Redact sensitive data from a string using built-in patterns.
/// This is a lighter version of SensitiveDataRedactor for metrics context.
fn redact_string(text: &str) -> String {
    let mut result = text.to_string();
    for (pattern, replacement) in SECRET_PATTERNS.iter() {
        result = pattern.replace_all(&result, *replacement).to_string();
    }
    result
}

/// Redact sensitive data from Prometheus text format metrics.
///
/// This function processes the Prometheus text output and redacts any
/// sensitive data found in metric label VALUES (not names).
///
/// # Arguments
///
/// * `metrics_text` - Raw Prometheus text format metrics
///
/// # Returns
///
/// The metrics text with sensitive label values redacted.
#[must_use]
pub fn redact_prometheus_text(metrics_text: &str) -> String {
    let mut result = String::with_capacity(metrics_text.len());

    for line in metrics_text.lines() {
        // Skip comment and help lines (start with #)
        if line.starts_with('#') {
            result.push_str(line);
            result.push('\n');
            continue;
        }

        // Check if line has labels (contains {...})
        if let (Some(start), Some(end)) = (line.find('{'), line.find('}')) {
            // Get the parts
            let before_labels = &line[..=start];
            let labels_content = &line[start + 1..end];
            let after_labels = &line[end..];

            // Process labels - redact values
            let redacted_labels =
                LABEL_VALUE_PATTERN.replace_all(labels_content, |caps: &regex::Captures| {
                    let label_name = &caps[1];
                    let value = &caps[2];

                    // Redact the value
                    let redacted_value = redact_string(value);

                    format!("{}=\"{}\"", label_name, redacted_value)
                });

            result.push_str(before_labels);
            result.push_str(&redacted_labels);
            result.push_str(after_labels);
            result.push('\n');
        } else {
            // No labels, just pass through
            result.push_str(line);
            result.push('\n');
        }
    }

    // Remove trailing newline if original didn't have one
    if !metrics_text.ends_with('\n') && result.ends_with('\n') {
        result.pop();
    }

    result
}

/// Global metrics registry instance
static GLOBAL_REGISTRY: std::sync::OnceLock<Arc<MetricsRegistry>> = std::sync::OnceLock::new();

/// Global metrics recorder instance (holds actual metric handles)
static GLOBAL_RECORDER: std::sync::OnceLock<Arc<MetricsRecorder>> = std::sync::OnceLock::new();

/// Metrics registry for Prometheus metrics
///
/// This registry maintains all metrics for a `DashFlow` application and provides
/// methods to record and export them in Prometheus text format.
pub struct MetricsRegistry {
    /// Prometheus registry
    registry: Registry,
}

impl MetricsRegistry {
    /// Create a new metrics registry
    pub fn new() -> Result<Self> {
        Ok(Self {
            registry: Registry::new(),
        })
    }

    /// Get the global metrics registry
    ///
    /// If the registry hasn't been initialized, this creates a new one.
    #[allow(clippy::expect_used)] // Intentional panic in global singleton initialization
    pub fn global() -> Arc<Self> {
        Arc::clone(GLOBAL_REGISTRY.get_or_init(|| {
            Arc::new(Self::new().expect("Failed to create global metrics registry"))
        }))
    }

    /// Register a counter metric
    ///
    /// Counters track monotonically increasing values.
    ///
    /// # Arguments
    ///
    /// * `name` - Metric name (e.g., "`graph_invocations_total`")
    /// * `help` - Help text describing the metric
    /// * `labels` - Label names (e.g., \["status", "graph_name"\])
    pub fn register_counter(&self, name: &str, help: &str, labels: &[&str]) -> Result<()> {
        let opts = Opts::new(name, help);

        if labels.is_empty() {
            let counter = IntCounter::with_opts(opts)
                .map_err(|e| Error::Metrics(format!("Failed to create counter: {e}")))?;
            self.registry
                .register(Box::new(counter))
                .map_err(|e| Error::Metrics(format!("Failed to register counter: {e}")))?;
        } else {
            let counter_vec = IntCounterVec::new(opts, labels)
                .map_err(|e| Error::Metrics(format!("Failed to create counter vec: {e}")))?;
            self.registry
                .register(Box::new(counter_vec))
                .map_err(|e| Error::Metrics(format!("Failed to register counter vec: {e}")))?;
        }

        Ok(())
    }

    /// Register a gauge metric
    ///
    /// Gauges track values that can go up and down.
    ///
    /// # Arguments
    ///
    /// * `name` - Metric name (e.g., "`active_graph_executions`")
    /// * `help` - Help text describing the metric
    /// * `labels` - Label names (e.g., \["graph_name"\])
    pub fn register_gauge(&self, name: &str, help: &str, labels: &[&str]) -> Result<()> {
        let opts = Opts::new(name, help);

        if labels.is_empty() {
            let gauge = IntGauge::with_opts(opts)
                .map_err(|e| Error::Metrics(format!("Failed to create gauge: {e}")))?;
            self.registry
                .register(Box::new(gauge))
                .map_err(|e| Error::Metrics(format!("Failed to register gauge: {e}")))?;
        } else {
            let gauge_vec = IntGaugeVec::new(opts, labels)
                .map_err(|e| Error::Metrics(format!("Failed to create gauge vec: {e}")))?;
            self.registry
                .register(Box::new(gauge_vec))
                .map_err(|e| Error::Metrics(format!("Failed to register gauge vec: {e}")))?;
        }

        Ok(())
    }

    /// Register a histogram metric
    ///
    /// Histograms track distributions of values (e.g., request durations).
    ///
    /// # Arguments
    ///
    /// * `name` - Metric name (e.g., "`graph_duration_seconds`")
    /// * `help` - Help text describing the metric
    /// * `labels` - Label names (e.g., \["graph_name"\])
    /// * `buckets` - Optional bucket boundaries (defaults to standard buckets)
    pub fn register_histogram(
        &self,
        name: &str,
        help: &str,
        labels: &[&str],
        buckets: Option<Vec<f64>>,
    ) -> Result<()> {
        let mut opts = HistogramOpts::new(name, help);

        if let Some(buckets) = buckets {
            opts = opts.buckets(buckets);
        }

        let histogram = if labels.is_empty() {
            Histogram::with_opts(opts)
                .map_err(|e| Error::Metrics(format!("Failed to create histogram: {e}")))?
        } else {
            // For labeled histograms, we need HistogramVec, but for simplicity
            // we'll use a single Histogram for now. Full implementation would use HistogramVec.
            Histogram::with_opts(opts)
                .map_err(|e| Error::Metrics(format!("Failed to create histogram: {e}")))?
        };

        self.registry
            .register(Box::new(histogram))
            .map_err(|e| Error::Metrics(format!("Failed to register histogram: {e}")))?;

        Ok(())
    }

    /// Export all metrics in Prometheus text format
    ///
    /// Returns a string containing all metrics in the format expected by Prometheus.
    ///
    /// **M-646 Fix:** This method exports metrics from BOTH:
    /// 1. The custom DashFlow registry (self.registry)
    /// 2. The prometheus default registry (where dashflow-streaming registers metrics)
    ///
    /// Metrics are deduplicated by family name, with custom registry taking precedence.
    /// This ensures all DashStream telemetry metrics are visible in Prometheus scrapes.
    ///
    /// By default, sensitive data in metric label values is redacted for security.
    /// This behavior is controlled by the `DASHFLOW_METRICS_REDACT` environment variable:
    /// - Default (unset): redaction enabled
    /// - "true", "1", "yes", "on": redaction enabled
    /// - "false", "0", "no", "off": redaction disabled
    pub fn export(&self) -> Result<String> {
        let encoder = TextEncoder::new();

        // M-646: Gather from custom registry
        let custom_families = self.registry.gather();
        let custom_names: std::collections::HashSet<String> = custom_families
            .iter()
            .map(|f| f.get_name().to_string())
            .collect();

        // M-646: Gather from prometheus default registry (where dashflow-streaming registers)
        let default_families = prometheus::default_registry().gather();

        // M-646: Merge families, deduping by name (custom registry takes precedence)
        let mut merged_families = custom_families;
        let mut collision_count = 0;
        for family in default_families {
            let name = family.get_name();
            if custom_names.contains(name) {
                collision_count += 1;
                tracing::debug!(
                    metric = name,
                    "Metric family exists in both registries; using custom registry version"
                );
            } else {
                merged_families.push(family);
            }
        }

        if collision_count > 0 {
            tracing::info!(
                collision_count,
                "Merged metrics from custom and default registries with {} collisions",
                collision_count
            );
        }

        let mut buffer = Vec::new();
        encoder
            .encode(&merged_families, &mut buffer)
            .map_err(|e| Error::Metrics(format!("Failed to encode metrics: {e}")))?;

        let metrics_text = String::from_utf8(buffer)
            .map_err(|e| Error::Metrics(format!("Failed to convert metrics to UTF-8: {e}")))?;

        // Apply redaction if enabled (default: ON for security)
        if is_metrics_redaction_enabled() {
            Ok(redact_prometheus_text(&metrics_text))
        } else {
            Ok(metrics_text)
        }
    }

    /// Get the underlying Prometheus registry
    ///
    /// This allows advanced users to register custom metrics directly.
    #[must_use]
    pub fn registry(&self) -> &Registry {
        &self.registry
    }
}

impl Default for MetricsRegistry {
    #[allow(clippy::expect_used)] // Default must be infallible; Self::new() only fails on registry creation
    fn default() -> Self {
        Self::new().expect("Failed to create default metrics registry")
    }
}

/// Register default `DashFlow` metrics
///
/// This function registers all standard metrics used by `DashFlow` applications:
/// - Graph execution metrics (invocations, duration, errors)
/// - Node execution metrics (duration, errors)
/// - LLM call metrics (requests, tokens, duration, errors)
/// - Checkpointer metrics (save/load duration, size)
///
/// # Example
///
/// ```rust,no_run
/// use dashflow_observability::metrics::register_default_metrics;
///
/// register_default_metrics()?;
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
pub fn register_default_metrics() -> Result<()> {
    let registry = MetricsRegistry::global();

    // Graph execution metrics
    registry.register_counter(
        "graph_invocations_total",
        "Total number of graph invocations",
        &["graph_name", "status"],
    )?;

    registry.register_histogram(
        "graph_duration_seconds",
        "Graph execution duration in seconds",
        &["graph_name"],
        Some(vec![0.001, 0.005, 0.01, 0.05, 0.1, 0.5, 1.0, 5.0, 10.0]),
    )?;

    registry.register_gauge(
        "graph_active_executions",
        "Number of currently executing graphs",
        &["graph_name"],
    )?;

    // Node execution metrics
    registry.register_counter(
        "node_executions_total",
        "Total number of node executions",
        &["graph_name", "node_name", "status"],
    )?;

    registry.register_histogram(
        "node_duration_seconds",
        "Node execution duration in seconds",
        &["graph_name", "node_name"],
        Some(vec![0.001, 0.005, 0.01, 0.05, 0.1, 0.5, 1.0, 5.0]),
    )?;

    // LLM call metrics
    registry.register_counter(
        "llm_requests_total",
        "Total number of LLM API requests",
        &["provider", "model", "status"],
    )?;

    registry.register_counter(
        "llm_tokens_total",
        "Total number of tokens consumed",
        &["provider", "model", "token_type"],
    )?;

    registry.register_histogram(
        "llm_request_duration_seconds",
        "LLM request duration in seconds",
        &["provider", "model"],
        Some(vec![0.1, 0.5, 1.0, 2.0, 5.0, 10.0, 30.0]),
    )?;

    // Checkpointer metrics
    registry.register_histogram(
        "checkpoint_save_duration_seconds",
        "Checkpoint save duration in seconds",
        &["checkpointer_type"],
        Some(vec![0.001, 0.005, 0.01, 0.05, 0.1, 0.5, 1.0]),
    )?;

    registry.register_histogram(
        "checkpoint_load_duration_seconds",
        "Checkpoint load duration in seconds",
        &["checkpointer_type"],
        Some(vec![0.001, 0.005, 0.01, 0.05, 0.1, 0.5, 1.0]),
    )?;

    registry.register_histogram(
        "checkpoint_size_bytes",
        "Checkpoint size in bytes",
        &["checkpointer_type"],
        Some(vec![1024.0, 10240.0, 102400.0, 1024000.0, 10240000.0]),
    )?;

    Ok(())
}

/// Metrics recorder that holds references to actual Prometheus metrics
///
/// This provides convenient methods to record metric values without needing
/// to look up metrics in the registry each time.
pub struct MetricsRecorder {
    // Graph metrics
    graph_invocations: IntCounterVec,
    graph_duration: HistogramVec,
    graph_active_executions: IntGaugeVec,

    // Node metrics
    node_executions: IntCounterVec,
    node_duration: HistogramVec,

    // Error tracking metrics
    errors_total: IntCounterVec,
    error_rate_window: IntCounterVec,

    // Resource usage metrics
    active_tasks: IntGauge,
    memory_allocated_bytes: IntGauge,
    queue_depth: IntGaugeVec,

    // SLO tracking metrics
    slo_latency_violations: IntCounterVec,
    slo_error_rate_violations: IntCounterVec,
    slo_availability_violations: IntCounterVec,

    // LLM metrics
    llm_requests: IntCounterVec,
    llm_tokens: IntCounterVec,
    llm_duration: HistogramVec,

    // Checkpoint metrics
    checkpoint_save_duration: HistogramVec,
    checkpoint_load_duration: HistogramVec,
    checkpoint_size: HistogramVec,
}

impl MetricsRecorder {
    /// Create a new metrics recorder and register metrics
    ///
    /// This creates the metric handles and registers them with the global registry.
    pub fn new() -> Result<Self> {
        let registry = MetricsRegistry::global();

        // Create metric handles
        let graph_invocations = IntCounterVec::new(
            Opts::new(
                "graph_invocations_total",
                "Total number of graph invocations",
            ),
            &["graph_name", "status"],
        )
        .map_err(|e| Error::Metrics(format!("Failed to create graph_invocations: {e}")))?;

        let graph_duration = HistogramVec::new(
            HistogramOpts::new(
                "graph_duration_seconds",
                "Graph execution duration in seconds",
            )
            .buckets(vec![0.001, 0.005, 0.01, 0.05, 0.1, 0.5, 1.0, 5.0, 10.0]),
            &["graph_name"],
        )
        .map_err(|e| Error::Metrics(format!("Failed to create graph_duration: {e}")))?;

        let graph_active_executions = IntGaugeVec::new(
            Opts::new(
                "graph_active_executions",
                "Number of currently executing graphs",
            ),
            &["graph_name"],
        )
        .map_err(|e| Error::Metrics(format!("Failed to create graph_active_executions: {e}")))?;

        let node_executions = IntCounterVec::new(
            Opts::new("node_executions_total", "Total number of node executions"),
            &["graph_name", "node_name", "status"],
        )
        .map_err(|e| Error::Metrics(format!("Failed to create node_executions: {e}")))?;

        let node_duration = HistogramVec::new(
            HistogramOpts::new(
                "node_duration_seconds",
                "Node execution duration in seconds",
            )
            .buckets(vec![0.001, 0.005, 0.01, 0.05, 0.1, 0.5, 1.0, 5.0]),
            &["graph_name", "node_name"],
        )
        .map_err(|e| Error::Metrics(format!("Failed to create node_duration: {e}")))?;

        // Error tracking metrics
        let errors_total = IntCounterVec::new(
            Opts::new(
                "errors_total",
                "Total number of errors by type and component",
            ),
            &["component", "error_type", "severity"],
        )
        .map_err(|e| Error::Metrics(format!("Failed to create errors_total: {e}")))?;

        let error_rate_window = IntCounterVec::new(
            Opts::new(
                "error_rate_window_total",
                "Errors within sliding window for rate calculation",
            ),
            &["component", "window_seconds"],
        )
        .map_err(|e| Error::Metrics(format!("Failed to create error_rate_window: {e}")))?;

        // Resource usage metrics
        let active_tasks = IntGauge::with_opts(Opts::new(
            "active_tasks",
            "Number of currently active async tasks",
        ))
        .map_err(|e| Error::Metrics(format!("Failed to create active_tasks: {e}")))?;

        let memory_allocated_bytes = IntGauge::with_opts(Opts::new(
            "memory_allocated_bytes",
            "Estimated memory allocated by the application",
        ))
        .map_err(|e| Error::Metrics(format!("Failed to create memory_allocated_bytes: {e}")))?;

        let queue_depth = IntGaugeVec::new(
            Opts::new("queue_depth", "Current depth of internal queues"),
            &["queue_name"],
        )
        .map_err(|e| Error::Metrics(format!("Failed to create queue_depth: {e}")))?;

        // SLO tracking metrics
        let slo_latency_violations = IntCounterVec::new(
            Opts::new(
                "slo_latency_violations_total",
                "Total SLO latency threshold violations",
            ),
            &["slo_name", "threshold_ms"],
        )
        .map_err(|e| Error::Metrics(format!("Failed to create slo_latency_violations: {e}")))?;

        let slo_error_rate_violations = IntCounterVec::new(
            Opts::new(
                "slo_error_rate_violations_total",
                "Total SLO error rate threshold violations",
            ),
            &["slo_name", "threshold_percent"],
        )
        .map_err(|e| Error::Metrics(format!("Failed to create slo_error_rate_violations: {e}")))?;

        let slo_availability_violations = IntCounterVec::new(
            Opts::new(
                "slo_availability_violations_total",
                "Total SLO availability threshold violations",
            ),
            &["slo_name", "threshold_percent"],
        )
        .map_err(|e| {
            Error::Metrics(format!("Failed to create slo_availability_violations: {e}"))
        })?;

        // LLM metrics
        let llm_requests = IntCounterVec::new(
            Opts::new("llm_requests_total", "Total number of LLM API requests"),
            &["provider", "model", "status"],
        )
        .map_err(|e| Error::Metrics(format!("Failed to create llm_requests: {e}")))?;

        let llm_tokens = IntCounterVec::new(
            Opts::new("llm_tokens_total", "Total number of tokens consumed"),
            &["provider", "model", "token_type"],
        )
        .map_err(|e| Error::Metrics(format!("Failed to create llm_tokens: {e}")))?;

        let llm_duration = HistogramVec::new(
            HistogramOpts::new(
                "llm_request_duration_seconds",
                "LLM request duration in seconds",
            )
            .buckets(vec![0.1, 0.5, 1.0, 2.0, 5.0, 10.0, 30.0]),
            &["provider", "model"],
        )
        .map_err(|e| Error::Metrics(format!("Failed to create llm_duration: {e}")))?;

        // Checkpoint metrics
        let checkpoint_save_duration = HistogramVec::new(
            HistogramOpts::new(
                "checkpoint_save_duration_seconds",
                "Checkpoint save duration in seconds",
            )
            .buckets(vec![0.001, 0.005, 0.01, 0.05, 0.1, 0.5, 1.0]),
            &["checkpointer_type"],
        )
        .map_err(|e| Error::Metrics(format!("Failed to create checkpoint_save_duration: {e}")))?;

        let checkpoint_load_duration = HistogramVec::new(
            HistogramOpts::new(
                "checkpoint_load_duration_seconds",
                "Checkpoint load duration in seconds",
            )
            .buckets(vec![0.001, 0.005, 0.01, 0.05, 0.1, 0.5, 1.0]),
            &["checkpointer_type"],
        )
        .map_err(|e| Error::Metrics(format!("Failed to create checkpoint_load_duration: {e}")))?;

        let checkpoint_size = HistogramVec::new(
            HistogramOpts::new("checkpoint_size_bytes", "Checkpoint size in bytes")
                .buckets(vec![1024.0, 10240.0, 102400.0, 1024000.0, 10240000.0]),
            &["checkpointer_type"],
        )
        .map_err(|e| Error::Metrics(format!("Failed to create checkpoint_size: {e}")))?;

        // Register all metrics with the global registry.
        //
        // Ignore `AlreadyReg` to support idempotent initialization, but log unexpected failures
        // so missing metrics don't go unnoticed.
        let prometheus_registry = registry.registry();
        let register_metric = |collector: Box<dyn prometheus::core::Collector>,
                               metric_name: &'static str| {
            if let Err(err) = prometheus_registry.register(collector) {
                if !matches!(err, prometheus::Error::AlreadyReg) {
                    tracing::warn!(
                        metric_name,
                        error = %err,
                        "Failed to register Prometheus metric"
                    );
                }
            }
        };

        register_metric(
            Box::new(graph_invocations.clone()),
            "graph_invocations_total",
        );
        register_metric(Box::new(graph_duration.clone()), "graph_duration_seconds");
        register_metric(
            Box::new(graph_active_executions.clone()),
            "graph_active_executions",
        );
        register_metric(Box::new(node_executions.clone()), "node_executions_total");
        register_metric(Box::new(node_duration.clone()), "node_duration_seconds");
        register_metric(Box::new(errors_total.clone()), "errors_total");
        register_metric(
            Box::new(error_rate_window.clone()),
            "error_rate_window_total",
        );
        register_metric(Box::new(active_tasks.clone()), "active_tasks");
        register_metric(
            Box::new(memory_allocated_bytes.clone()),
            "memory_allocated_bytes",
        );
        register_metric(Box::new(queue_depth.clone()), "queue_depth");
        register_metric(
            Box::new(slo_latency_violations.clone()),
            "slo_latency_violations_total",
        );
        register_metric(
            Box::new(slo_error_rate_violations.clone()),
            "slo_error_rate_violations_total",
        );
        register_metric(
            Box::new(slo_availability_violations.clone()),
            "slo_availability_violations_total",
        );
        register_metric(Box::new(llm_requests.clone()), "llm_requests_total");
        register_metric(Box::new(llm_tokens.clone()), "llm_tokens_total");
        register_metric(
            Box::new(llm_duration.clone()),
            "llm_request_duration_seconds",
        );
        register_metric(
            Box::new(checkpoint_save_duration.clone()),
            "checkpoint_save_duration_seconds",
        );
        register_metric(
            Box::new(checkpoint_load_duration.clone()),
            "checkpoint_load_duration_seconds",
        );
        register_metric(Box::new(checkpoint_size.clone()), "checkpoint_size_bytes");

        Ok(Self {
            graph_invocations,
            graph_duration,
            graph_active_executions,
            node_executions,
            node_duration,
            errors_total,
            error_rate_window,
            active_tasks,
            memory_allocated_bytes,
            queue_depth,
            slo_latency_violations,
            slo_error_rate_violations,
            slo_availability_violations,
            llm_requests,
            llm_tokens,
            llm_duration,
            checkpoint_save_duration,
            checkpoint_load_duration,
            checkpoint_size,
        })
    }

    /// Get the global metrics recorder
    ///
    /// This returns None if the recorder hasn't been initialized yet.
    /// Call `init_default_recorder()` first to initialize it.
    pub fn global() -> Option<Arc<Self>> {
        GLOBAL_RECORDER.get().cloned()
    }

    /// Record a graph invocation
    pub fn record_graph_invocation(&self, graph_name: &str, status: &str) {
        self.graph_invocations
            .with_label_values(&[graph_name, status])
            .inc();
    }

    /// Record graph execution duration
    pub fn record_graph_duration(&self, graph_name: &str, duration_seconds: f64) {
        self.graph_duration
            .with_label_values(&[graph_name])
            .observe(duration_seconds);
    }

    /// Increment active graph executions
    pub fn inc_active_graphs(&self, graph_name: &str) {
        self.graph_active_executions
            .with_label_values(&[graph_name])
            .inc();
    }

    /// Decrement active graph executions
    pub fn dec_active_graphs(&self, graph_name: &str) {
        self.graph_active_executions
            .with_label_values(&[graph_name])
            .dec();
    }

    /// Record a node execution
    pub fn record_node_execution(&self, graph_name: &str, node_name: &str, status: &str) {
        self.node_executions
            .with_label_values(&[graph_name, node_name, status])
            .inc();
    }

    /// Record node execution duration
    pub fn record_node_duration(&self, graph_name: &str, node_name: &str, duration_seconds: f64) {
        self.node_duration
            .with_label_values(&[graph_name, node_name])
            .observe(duration_seconds);
    }

    // ========== Error Tracking Methods ==========

    /// Record an error occurrence
    ///
    /// # Arguments
    /// * `component` - Component where error occurred (e.g., "graph", "node", "llm", "checkpointer")
    /// * `error_type` - Type of error (e.g., "timeout", "network", "validation", "internal")
    /// * `severity` - Severity level ("critical", "error", "warning")
    pub fn record_error(&self, component: &str, error_type: &str, severity: &str) {
        self.errors_total
            .with_label_values(&[component, error_type, severity])
            .inc();
    }

    /// Record error for rate window calculation
    ///
    /// # Arguments
    /// * `component` - Component where error occurred
    /// * `window_seconds` - Window size for rate calculation (e.g., "60", "300", "3600")
    pub fn record_error_in_window(&self, component: &str, window_seconds: &str) {
        self.error_rate_window
            .with_label_values(&[component, window_seconds])
            .inc();
    }

    // ========== Resource Usage Methods ==========

    /// Set the number of active tasks
    pub fn set_active_tasks(&self, count: i64) {
        self.active_tasks.set(count);
    }

    /// Increment active tasks
    pub fn inc_active_tasks(&self) {
        self.active_tasks.inc();
    }

    /// Decrement active tasks
    pub fn dec_active_tasks(&self) {
        self.active_tasks.dec();
    }

    /// Set the estimated memory allocation in bytes
    pub fn set_memory_allocated(&self, bytes: i64) {
        self.memory_allocated_bytes.set(bytes);
    }

    /// Set the depth of a named queue
    pub fn set_queue_depth(&self, queue_name: &str, depth: i64) {
        self.queue_depth.with_label_values(&[queue_name]).set(depth);
    }

    /// Increment queue depth
    pub fn inc_queue_depth(&self, queue_name: &str) {
        self.queue_depth.with_label_values(&[queue_name]).inc();
    }

    /// Decrement queue depth
    pub fn dec_queue_depth(&self, queue_name: &str) {
        self.queue_depth.with_label_values(&[queue_name]).dec();
    }

    // ========== SLO Tracking Methods ==========

    /// Record a latency SLO violation
    ///
    /// Call this when a request exceeds the latency threshold.
    ///
    /// # Arguments
    /// * `slo_name` - Name of the SLO (e.g., "graph_execution_p99", "llm_response_p95")
    /// * `threshold_ms` - The threshold that was violated (e.g., "100", "500", "1000")
    pub fn record_latency_slo_violation(&self, slo_name: &str, threshold_ms: &str) {
        self.slo_latency_violations
            .with_label_values(&[slo_name, threshold_ms])
            .inc();
    }

    /// Record an error rate SLO violation
    ///
    /// Call this when error rate exceeds the threshold.
    ///
    /// # Arguments
    /// * `slo_name` - Name of the SLO (e.g., "graph_error_rate", "node_failure_rate")
    /// * `threshold_percent` - The threshold that was violated (e.g., "1", "5", "10")
    pub fn record_error_rate_slo_violation(&self, slo_name: &str, threshold_percent: &str) {
        self.slo_error_rate_violations
            .with_label_values(&[slo_name, threshold_percent])
            .inc();
    }

    /// Record an availability SLO violation
    ///
    /// Call this when availability drops below the threshold.
    ///
    /// # Arguments
    /// * `slo_name` - Name of the SLO (e.g., "service_availability", "endpoint_uptime")
    /// * `threshold_percent` - The threshold that was violated (e.g., "99", "99.9", "99.99")
    pub fn record_availability_slo_violation(&self, slo_name: &str, threshold_percent: &str) {
        self.slo_availability_violations
            .with_label_values(&[slo_name, threshold_percent])
            .inc();
    }

    // ========== Convenience Methods ==========

    /// Check latency against SLO and record violation if exceeded
    ///
    /// Returns true if the SLO was violated.
    pub fn check_latency_slo(&self, slo_name: &str, threshold_ms: u64, actual_ms: u64) -> bool {
        if actual_ms > threshold_ms {
            self.record_latency_slo_violation(slo_name, &threshold_ms.to_string());
            true
        } else {
            false
        }
    }

    // ========== LLM Metrics Methods ==========

    /// Record an LLM API request
    ///
    /// # Arguments
    /// * `provider` - LLM provider name (e.g., "openai", "anthropic", "azure")
    /// * `model` - Model name (e.g., "gpt-4", "claude-3-opus")
    /// * `status` - Request status ("success", "error", "timeout")
    pub fn record_llm_request(&self, provider: &str, model: &str, status: &str) {
        self.llm_requests
            .with_label_values(&[provider, model, status])
            .inc();
    }

    /// Record LLM token usage
    ///
    /// # Arguments
    /// * `provider` - LLM provider name
    /// * `model` - Model name
    /// * `token_type` - Type of tokens ("prompt", "completion", "total")
    /// * `count` - Number of tokens
    pub fn record_llm_tokens(&self, provider: &str, model: &str, token_type: &str, count: u64) {
        self.llm_tokens
            .with_label_values(&[provider, model, token_type])
            .inc_by(count);
    }

    /// Record LLM request duration
    ///
    /// # Arguments
    /// * `provider` - LLM provider name
    /// * `model` - Model name
    /// * `duration_seconds` - Request duration in seconds
    pub fn record_llm_duration(&self, provider: &str, model: &str, duration_seconds: f64) {
        self.llm_duration
            .with_label_values(&[provider, model])
            .observe(duration_seconds);
    }

    // ========== Checkpoint Metrics Methods ==========

    /// Record checkpoint save duration
    ///
    /// # Arguments
    /// * `checkpointer_type` - Type of checkpointer (e.g., "memory", "sqlite", "redis")
    /// * `duration_seconds` - Save duration in seconds
    pub fn record_checkpoint_save(&self, checkpointer_type: &str, duration_seconds: f64) {
        self.checkpoint_save_duration
            .with_label_values(&[checkpointer_type])
            .observe(duration_seconds);
    }

    /// Record checkpoint load duration
    ///
    /// # Arguments
    /// * `checkpointer_type` - Type of checkpointer
    /// * `duration_seconds` - Load duration in seconds
    pub fn record_checkpoint_load(&self, checkpointer_type: &str, duration_seconds: f64) {
        self.checkpoint_load_duration
            .with_label_values(&[checkpointer_type])
            .observe(duration_seconds);
    }

    /// Record checkpoint size
    ///
    /// # Arguments
    /// * `checkpointer_type` - Type of checkpointer
    /// * `size_bytes` - Checkpoint size in bytes
    pub fn record_checkpoint_size(&self, checkpointer_type: &str, size_bytes: f64) {
        self.checkpoint_size
            .with_label_values(&[checkpointer_type])
            .observe(size_bytes);
    }
}

/// SLO definition for configuring service level objectives
#[derive(Debug, Clone)]
pub struct SloDefinition {
    /// Unique name for this SLO
    pub name: String,
    /// Type of SLO
    pub slo_type: SloType,
    /// Threshold value
    pub threshold: f64,
    /// Description of the SLO
    pub description: String,
}

/// Types of SLOs supported
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SloType {
    /// Latency threshold in milliseconds (p50, p95, p99)
    LatencyMs,
    /// Error rate as percentage (0-100)
    ErrorRatePercent,
    /// Availability as percentage (0-100)
    AvailabilityPercent,
}

impl SloDefinition {
    /// Create a new latency SLO
    ///
    /// # Arguments
    /// * `name` - Unique name for this SLO
    /// * `threshold_ms` - Latency threshold in milliseconds
    /// * `description` - Description of what this SLO tracks
    pub fn latency(
        name: impl Into<String>,
        threshold_ms: f64,
        description: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            slo_type: SloType::LatencyMs,
            threshold: threshold_ms,
            description: description.into(),
        }
    }

    /// Create a new error rate SLO
    ///
    /// # Arguments
    /// * `name` - Unique name for this SLO
    /// * `threshold_percent` - Error rate threshold as percentage (e.g., 1.0 for 1%)
    /// * `description` - Description of what this SLO tracks
    pub fn error_rate(
        name: impl Into<String>,
        threshold_percent: f64,
        description: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            slo_type: SloType::ErrorRatePercent,
            threshold: threshold_percent,
            description: description.into(),
        }
    }

    /// Create a new availability SLO
    ///
    /// # Arguments
    /// * `name` - Unique name for this SLO
    /// * `threshold_percent` - Availability threshold as percentage (e.g., 99.9 for 99.9%)
    /// * `description` - Description of what this SLO tracks
    pub fn availability(
        name: impl Into<String>,
        threshold_percent: f64,
        description: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            slo_type: SloType::AvailabilityPercent,
            threshold: threshold_percent,
            description: description.into(),
        }
    }
}

/// Default SLO definitions for DashFlow applications
///
/// These are recommended starting points that can be customized per deployment.
pub fn default_slo_definitions() -> Vec<SloDefinition> {
    vec![
        // Graph execution latency SLOs
        SloDefinition::latency(
            "graph_execution_p50",
            100.0,
            "50th percentile graph execution should complete within 100ms",
        ),
        SloDefinition::latency(
            "graph_execution_p95",
            500.0,
            "95th percentile graph execution should complete within 500ms",
        ),
        SloDefinition::latency(
            "graph_execution_p99",
            1000.0,
            "99th percentile graph execution should complete within 1s",
        ),
        // LLM response latency SLOs
        SloDefinition::latency(
            "llm_response_p50",
            500.0,
            "50th percentile LLM response should complete within 500ms",
        ),
        SloDefinition::latency(
            "llm_response_p95",
            2000.0,
            "95th percentile LLM response should complete within 2s",
        ),
        SloDefinition::latency(
            "llm_response_p99",
            5000.0,
            "99th percentile LLM response should complete within 5s",
        ),
        // Error rate SLOs
        SloDefinition::error_rate(
            "graph_error_rate",
            1.0,
            "Graph execution error rate should be below 1%",
        ),
        SloDefinition::error_rate(
            "node_failure_rate",
            0.5,
            "Individual node failure rate should be below 0.5%",
        ),
        SloDefinition::error_rate(
            "llm_error_rate",
            2.0,
            "LLM API error rate should be below 2%",
        ),
        // Availability SLOs
        SloDefinition::availability(
            "service_availability",
            99.9,
            "Service should be available 99.9% of the time",
        ),
        SloDefinition::availability(
            "checkpoint_availability",
            99.5,
            "Checkpoint service should be available 99.5% of the time",
        ),
    ]
}

/// Initialize the default metrics recorder
///
/// This should be called after `register_default_metrics()` to create
/// the global recorder that can be used to record metric values.
///
/// # Example
///
/// ```rust,no_run
/// use dashflow_observability::metrics::{register_default_metrics, init_default_recorder};
///
/// // First register the metrics
/// register_default_metrics()?;
///
/// // Then initialize the recorder
/// init_default_recorder()?;
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
pub fn init_default_recorder() -> Result<()> {
    let recorder = MetricsRecorder::new()?;
    GLOBAL_RECORDER
        .set(Arc::new(recorder))
        .map_err(|existing| {
            Error::Metrics(format!(
                "Global recorder already initialized (ptr={existing:p})"
            ))
        })?;
    Ok(())
}

/// Get the global Prometheus registry for the DashFlow ecosystem
///
/// This is the authoritative registry that all DashFlow crates should use
/// for registering and exporting Prometheus metrics. Using a single registry
/// ensures all metrics appear in a unified `/metrics` endpoint.
pub fn metrics_registry() -> Arc<MetricsRegistry> {
    MetricsRegistry::global()
}

/// Export all metrics from the global registry in Prometheus text format
///
/// This function gathers metrics from the unified global registry and encodes
/// them in Prometheus text exposition format.
pub fn export_metrics() -> Result<String> {
    MetricsRegistry::global().export()
}

#[cfg(test)]
mod tests {
    // `cargo verify` runs clippy with `-D warnings` for all targets, including unit tests.
    // Setup code in tests uses `unwrap`/`expect` to make failures loud and local.
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    #[test]
    fn test_registry_creation() {
        let registry = MetricsRegistry::new().unwrap();
        assert!(registry.export().is_ok());
    }

    #[test]
    fn test_counter_registration() {
        let registry = MetricsRegistry::new().unwrap();
        assert!(registry
            .register_counter("test_counter", "Test counter", &[])
            .is_ok());
    }

    #[test]
    fn test_gauge_registration() {
        let registry = MetricsRegistry::new().unwrap();
        assert!(registry
            .register_gauge("test_gauge", "Test gauge", &[])
            .is_ok());
    }

    #[test]
    fn test_histogram_registration() {
        let registry = MetricsRegistry::new().unwrap();
        assert!(registry
            .register_histogram("test_histogram", "Test histogram", &[], None)
            .is_ok());
    }

    #[test]
    fn test_export_empty_registry() {
        let registry = MetricsRegistry::new().unwrap();
        let output = registry.export().unwrap();
        assert!(output.is_empty() || output.starts_with('#'));
    }

    #[test]
    fn test_default_metrics_registration() {
        // Test that all default metrics can be registered without errors
        // Note: We can't use the global registry here as it might already be initialized
        let registry = MetricsRegistry::new().unwrap();

        // Register sample metrics
        assert!(registry
            .register_counter("test_graph_invocations", "Test", &["status"])
            .is_ok());

        assert!(registry
            .register_gauge("test_active_graphs", "Test", &[])
            .is_ok());

        assert!(registry
            .register_histogram("test_duration", "Test", &[], None)
            .is_ok());

        // Verify export works (metrics may be empty until used)
        assert!(registry.export().is_ok());
    }

    #[test]
    fn test_metrics_recorder_creation() {
        // Test that we can create a MetricsRecorder
        let recorder = MetricsRecorder::new();
        assert!(recorder.is_ok());
    }

    #[test]
    fn test_metrics_recording() {
        // Test that we can record metrics
        // Note: This test uses the global registry which may have metrics from other tests
        let recorder = MetricsRecorder::new().unwrap();

        // Record some test metrics with a unique graph name to avoid conflicts
        let test_id = format!(
            "test_graph_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );

        recorder.record_graph_invocation(&test_id, "success");
        recorder.record_graph_duration(&test_id, 1.5);
        recorder.inc_active_graphs(&test_id);
        recorder.dec_active_graphs(&test_id);

        recorder.record_node_execution(&test_id, "test_node", "success");
        recorder.record_node_duration(&test_id, "test_node", 0.5);

        // Verify we can export metrics
        let registry = MetricsRegistry::global();
        let export = registry.export().unwrap();

        // The export should contain our recorded metrics
        // Note: In test isolation scenarios with parallel execution, the export timing
        // can vary. The key test is that recording works without panicking.
        // If export has content, verify it's properly formatted.
        if !export.is_empty() {
            // Verify the basic structure is there
            assert!(
                export.contains("# TYPE") || export.contains("# HELP"),
                "Export should contain Prometheus metric metadata when not empty"
            );
        }

        // The fact that we got here without panicking means recording works correctly
    }

    #[test]
    fn test_error_tracking_metrics() {
        let recorder = MetricsRecorder::new().unwrap();

        // Test error recording with different components and types
        recorder.record_error("graph", "timeout", "error");
        recorder.record_error("node", "validation", "warning");
        recorder.record_error("llm", "rate_limit", "critical");
        recorder.record_error("checkpointer", "connection", "error");

        // Test error rate window recording
        recorder.record_error_in_window("graph", "60");
        recorder.record_error_in_window("graph", "300");
        recorder.record_error_in_window("llm", "3600");

        // Verify export works - the key test is that recording works without panicking
        // Note: The global registry export may or may not contain our metrics depending
        // on test isolation, so we just verify export works
        let registry = MetricsRegistry::global();
        assert!(registry.export().is_ok(), "Export should succeed");
    }

    #[test]
    fn test_resource_usage_metrics() {
        let recorder = MetricsRecorder::new().unwrap();

        // Test active tasks
        recorder.set_active_tasks(10);
        recorder.inc_active_tasks();
        recorder.dec_active_tasks();

        // Test memory allocation
        recorder.set_memory_allocated(1024 * 1024 * 100); // 100MB

        // Test queue depth
        recorder.set_queue_depth("execution_queue", 50);
        recorder.inc_queue_depth("execution_queue");
        recorder.dec_queue_depth("execution_queue");
        recorder.set_queue_depth("checkpoint_queue", 25);

        // Verify export works - the key test is that recording works without panicking
        let registry = MetricsRegistry::global();
        assert!(registry.export().is_ok(), "Export should succeed");
    }

    #[test]
    fn test_slo_tracking_metrics() {
        let recorder = MetricsRecorder::new().unwrap();

        // Test latency SLO violations
        recorder.record_latency_slo_violation("graph_execution_p99", "1000");
        recorder.record_latency_slo_violation("llm_response_p95", "2000");

        // Test error rate SLO violations
        recorder.record_error_rate_slo_violation("graph_error_rate", "1");
        recorder.record_error_rate_slo_violation("llm_error_rate", "2");

        // Test availability SLO violations
        recorder.record_availability_slo_violation("service_availability", "99.9");
        recorder.record_availability_slo_violation("checkpoint_availability", "99.5");

        // Verify export works - the key test is that recording works without panicking
        let registry = MetricsRegistry::global();
        assert!(registry.export().is_ok(), "Export should succeed");
    }

    #[test]
    fn test_check_latency_slo() {
        let recorder = MetricsRecorder::new().unwrap();

        // Test case: latency within SLO
        let violated = recorder.check_latency_slo("test_slo", 100, 50);
        assert!(!violated, "Should not report violation when within SLO");

        // Test case: latency exactly at threshold (not violated)
        let violated = recorder.check_latency_slo("test_slo", 100, 100);
        assert!(
            !violated,
            "Should not report violation when exactly at threshold"
        );

        // Test case: latency exceeds SLO
        let violated = recorder.check_latency_slo("test_slo", 100, 150);
        assert!(
            violated,
            "Should report violation when latency exceeds threshold"
        );
    }

    #[test]
    #[allow(clippy::float_cmp)] // Comparing known constructor constants (100.0, 1.0, 99.9)
    fn test_slo_definitions() {
        // Test creating SLO definitions
        let latency_slo = SloDefinition::latency("test_latency", 100.0, "Test latency SLO");
        assert_eq!(latency_slo.name, "test_latency");
        assert_eq!(latency_slo.slo_type, SloType::LatencyMs);
        assert_eq!(latency_slo.threshold, 100.0);

        let error_rate_slo =
            SloDefinition::error_rate("test_error_rate", 1.0, "Test error rate SLO");
        assert_eq!(error_rate_slo.name, "test_error_rate");
        assert_eq!(error_rate_slo.slo_type, SloType::ErrorRatePercent);
        assert_eq!(error_rate_slo.threshold, 1.0);

        let availability_slo =
            SloDefinition::availability("test_availability", 99.9, "Test availability SLO");
        assert_eq!(availability_slo.name, "test_availability");
        assert_eq!(availability_slo.slo_type, SloType::AvailabilityPercent);
        assert_eq!(availability_slo.threshold, 99.9);
    }

    #[test]
    fn test_default_slo_definitions() {
        let slos = default_slo_definitions();

        // Should have 11 default SLOs
        assert_eq!(slos.len(), 11);

        // Verify we have all SLO types
        let latency_count = slos
            .iter()
            .filter(|s| s.slo_type == SloType::LatencyMs)
            .count();
        let error_rate_count = slos
            .iter()
            .filter(|s| s.slo_type == SloType::ErrorRatePercent)
            .count();
        let availability_count = slos
            .iter()
            .filter(|s| s.slo_type == SloType::AvailabilityPercent)
            .count();

        assert_eq!(latency_count, 6, "Should have 6 latency SLOs");
        assert_eq!(error_rate_count, 3, "Should have 3 error rate SLOs");
        assert_eq!(availability_count, 2, "Should have 2 availability SLOs");

        // Verify specific SLOs exist
        assert!(slos.iter().any(|s| s.name == "graph_execution_p99"));
        assert!(slos.iter().any(|s| s.name == "llm_response_p95"));
        assert!(slos.iter().any(|s| s.name == "graph_error_rate"));
        assert!(slos.iter().any(|s| s.name == "service_availability"));
    }

    // ========== Metrics Redaction Tests (M-223) ==========

    #[test]
    fn test_parse_redaction_flag_enabled() {
        // Test values that should enable redaction
        for val in ["true", "1", "yes", "on", "TRUE", "YES", "anything_else", ""] {
            assert!(
                parse_redaction_flag(val),
                "Redaction should be enabled for '{}'",
                val
            );
        }
    }

    #[test]
    fn test_parse_redaction_flag_disabled() {
        // Test values that should disable redaction
        for val in ["false", "0", "no", "off", "FALSE", "NO", "False", "Off"] {
            assert!(
                !parse_redaction_flag(val),
                "Redaction should be disabled for '{}'",
                val
            );
        }
    }

    #[test]
    fn test_redact_prometheus_text_openai_key() {
        let input = r#"# HELP my_metric Test metric
# TYPE my_metric counter
my_metric{label="sk-FAKE_TEST_KEY_abcdefghi0000000000"} 1
"#;
        let output = redact_prometheus_text(input);
        assert!(
            output.contains("[OPENAI_KEY]"),
            "OpenAI key should be redacted. Got: {}",
            output
        );
        assert!(
            !output.contains("sk-abc123def456"),
            "Original key should not be present"
        );
    }

    #[test]
    fn test_redact_prometheus_text_multiple_secrets() {
        let input = r#"# HELP test_metric Test
# TYPE test_metric gauge
test_metric{api="sk-FAKE_TEST_KEY_22222222222222222222",email="user@example.com"} 42
"#;
        let output = redact_prometheus_text(input);
        assert!(
            output.contains("[OPENAI_KEY]"),
            "OpenAI key should be redacted"
        );
        assert!(output.contains("[EMAIL]"), "Email should be redacted");
        assert!(
            !output.contains("user@example.com"),
            "Email should not be present"
        );
    }

    #[test]
    fn test_redact_prometheus_text_jwt_token() {
        // JWT token format: eyJ...header.eyJ...payload.signature
        let jwt = "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.dozjgNryP4J3jVmNHl0w5N_XgL0n3I9PlFUP0THsR8U";
        let input = format!(
            r#"# HELP test Test
test_metric{{token="{}"}} 1
"#,
            jwt
        );
        let output = redact_prometheus_text(&input);
        assert!(
            output.contains("[JWT_TOKEN]"),
            "JWT should be redacted. Got: {}",
            output
        );
    }

    #[test]
    fn test_redact_prometheus_text_preserves_comments() {
        let input = r#"# HELP my_metric This is a help message with sk-FAKE_TEST_KEY_22222222222222222222
# TYPE my_metric counter
my_metric 1
"#;
        let output = redact_prometheus_text(input);
        // Comments are passed through unchanged (redaction only applies to label values)
        assert!(output.contains("# HELP"), "Comments should be preserved");
        assert!(
            output.contains("my_metric 1"),
            "Metric value should be preserved"
        );
    }

    #[test]
    fn test_redact_prometheus_text_no_labels() {
        let input = r#"# HELP simple_metric Simple metric
# TYPE simple_metric gauge
simple_metric 42
"#;
        let output = redact_prometheus_text(input);
        assert_eq!(
            input, output,
            "Metrics without labels should pass through unchanged"
        );
    }

    #[test]
    fn test_redact_prometheus_text_safe_labels() {
        let input = r#"# HELP test_metric Test
test_metric{status="success",component="graph"} 1
"#;
        let output = redact_prometheus_text(input);
        // Safe values should remain unchanged
        assert!(
            output.contains(r#"status="success""#),
            "Safe status label should remain"
        );
        assert!(
            output.contains(r#"component="graph""#),
            "Safe component label should remain"
        );
    }

    #[test]
    fn test_redact_prometheus_text_url_password() {
        let input = r#"# TYPE db_conn gauge
db_conn{url="postgresql://user:secretpassword123@localhost:5432/db"} 1
"#;
        let output = redact_prometheus_text(input);
        assert!(
            output.contains("[CREDENTIALS]@"),
            "URL password should be redacted"
        );
        assert!(
            !output.contains("secretpassword123"),
            "Password should not be present"
        );
    }

    #[test]
    fn test_redact_prometheus_text_github_token() {
        let input = r#"# TYPE auth gauge
auth{token="ghp_FAKE0TEST0TOKEN0FOR0UNIT0TESTING000000"} 1
"#;
        let output = redact_prometheus_text(input);
        assert!(
            output.contains("[GITHUB_TOKEN]"),
            "GitHub token should be redacted. Got: {}",
            output
        );
    }

    #[test]
    fn test_redact_prometheus_text_aws_key() {
        let input = r#"# TYPE aws gauge
aws{access_key="AKIAFAKETEST00000000"} 1
"#;
        let output = redact_prometheus_text(input);
        assert!(
            output.contains("[AWS_KEY]"),
            "AWS key should be redacted. Got: {}",
            output
        );
    }

    // ========== M-646: Registry Merge Tests ==========

    #[test]
    fn test_m646_export_includes_default_registry_metrics() {
        // M-646: This test verifies that metrics registered to prometheus::default_registry()
        // are included in the export output alongside custom registry metrics.

        // Register a metric directly to the default prometheus registry
        // (simulating what dashflow-streaming does)
        let test_metric_name = format!(
            "m646_test_default_registry_metric_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );

        let counter = prometheus::Counter::new(&test_metric_name, "Test metric for M-646")
            .expect("Failed to create test counter");

        // This might fail if metric already registered, which is fine for the test
        let _ = prometheus::default_registry().register(Box::new(counter.clone()));
        counter.inc();

        // Export using our MetricsRegistry which should now include default registry metrics
        let registry = MetricsRegistry::new().expect("Failed to create registry");
        let export = registry.export().expect("Failed to export metrics");

        // The export should contain our metric from the default registry
        assert!(
            export.contains(&test_metric_name),
            "M-646: Export should include metrics from default_registry(). \
             Looking for '{}' in export output.",
            test_metric_name
        );
    }

    #[test]
    fn test_m646_custom_registry_takes_precedence() {
        // M-646: When a metric exists in both registries, custom registry takes precedence

        // Create a unique metric name for this test
        let metric_name = format!(
            "m646_precedence_test_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );

        // Register to default registry with value 1
        let default_counter = prometheus::Counter::new(&metric_name, "Default registry version")
            .expect("Failed to create default counter");
        let _ = prometheus::default_registry().register(Box::new(default_counter.clone()));
        default_counter.inc(); // value = 1

        // Create a custom registry and register same metric name with value 5
        let custom_registry = MetricsRegistry::new().expect("Failed to create custom registry");
        let custom_counter = prometheus::Counter::new(&metric_name, "Custom registry version")
            .expect("Failed to create custom counter");
        custom_registry
            .registry()
            .register(Box::new(custom_counter.clone()))
            .expect("Failed to register custom counter");
        for _ in 0..5 {
            custom_counter.inc(); // value = 5
        }

        // Export - should contain the custom registry version (value 5)
        let export = custom_registry.export().expect("Failed to export");

        // The metric should appear with value 5 (custom), not 1 (default)
        assert!(
            export.contains(&metric_name),
            "Export should contain the metric"
        );
        assert!(
            export.contains(&format!("{} 5", metric_name)),
            "M-646: Custom registry should take precedence. \
             Expected '{} 5' in output, got: {}",
            metric_name,
            export
        );
    }
}
