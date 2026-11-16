//! Telemetry: the log, the trace export and the metrics recorder this process installs, and the
//! tasks that sample what no call reports (ADR 0056).

use std::io::Write;
use std::num::NonZeroU32;
use std::sync::Arc;
use std::time::{Duration, Instant};

use adapter_http::Scrape;
use adapter_postgres::Database;
use app::observability::{self, BacklogGauges};
use app::port::driven::backlog::Backlog;
use clap::{Args, ValueEnum};
use metrics_exporter_prometheus::{Matcher, PrometheusBuilder, PrometheusHandle};
use metrics_util::MetricKindMask;
use opentelemetry::KeyValue;
use opentelemetry::trace::TracerProvider as _;
use opentelemetry_otlp::{Protocol, SpanExporter, WithExportConfig};
use opentelemetry_sdk::propagation::TraceContextPropagator;
use opentelemetry_sdk::resource::Resource;
use opentelemetry_sdk::trace::SdkTracerProvider;
use tokio::task::JoinHandle;
use tracing_subscriber::filter::LevelFilter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, Layer};

use crate::Error;
use crate::settings::Role;

/// How often the database pool is sampled.
const POOL_SAMPLE_INTERVAL: Duration = Duration::from_secs(5);

/// The upper bounds of the duration histograms, in seconds.
const SECONDS_BUCKETS: &[f64] = &[
    0.001, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0, 30.0, 60.0,
];

/// How long a gauge that nothing sets is still rendered. The backlog gauges of a queue that has
/// emptied are set once to zero and then left alone, and this lets the recorder forget them.
const GAUGE_IDLE_TIMEOUT: Duration = Duration::from_secs(300);

/// The longest `--backlog-interval`, in seconds: a gauge is read often enough that
/// [`GAUGE_IDLE_TIMEOUT`] never drops one that is still set.
const MAX_BACKLOG_INTERVAL: u32 = 120;

fn backlog_interval(value: &str) -> Result<NonZeroU32, String> {
    let seconds: NonZeroU32 = value
        .parse()
        .map_err(|error| format!("{error}: a number of seconds, 1 to {MAX_BACKLOG_INTERVAL}"))?;
    if seconds.get() > MAX_BACKLOG_INTERVAL {
        return Err(format!("at most {MAX_BACKLOG_INTERVAL} seconds"));
    }
    Ok(seconds)
}

/// The upper bounds of the landing attempts histogram.
const ATTEMPT_BUCKETS: &[f64] = &[1.0, 2.0, 3.0, 4.0, 8.0, 16.0, 32.0];

/// How a log line is written.
#[derive(Clone, Copy, PartialEq, Eq, Debug, ValueEnum)]
pub enum LogFormat {
    /// One JSON object per line.
    Json,
    /// Human-readable text.
    Text,
}

/// How the process logs.
#[derive(Args, Debug)]
pub struct TelemetryOptions {
    /// How a log line is written, on standard output.
    #[arg(long, env = "CODEHUB_LOG_FORMAT", value_enum, default_value = "json")]
    pub log_format: LogFormat,
    /// Which log lines are kept: a level, or `target=level` directives separated by commas. It
    /// does not filter the spans exported to the trace collector.
    #[arg(long, env = "CODEHUB_LOG", default_value = "info")]
    pub log_filter: String,
}

/// How often the queue and the outbox are measured, by the roles that run the background work.
#[derive(Args, Debug)]
pub struct BacklogOptions {
    /// How often the depth and age of the queues and the size of the directory outbox are read,
    /// in seconds, from 1 to 120.
    #[arg(
        long,
        env = "CODEHUB_BACKLOG_INTERVAL",
        default_value_t = NonZeroU32::MIN.saturating_add(14),
        value_parser = backlog_interval
    )]
    pub backlog_interval: NonZeroU32,
}

impl Role {
    /// The logging options of the role.
    pub(crate) fn telemetry(&self) -> &TelemetryOptions {
        match self {
            Self::Api(options) => &options.telemetry,
            Self::RunnerApi(options) => &options.telemetry,
            Self::Worker(options) => &options.telemetry,
            Self::Migrate(options) => &options.telemetry,
            Self::All(options) => &options.telemetry,
        }
    }

    /// The name the role is run by.
    pub(crate) const fn name(&self) -> &'static str {
        match self {
            Self::Api(_) => "api",
            Self::RunnerApi(_) => "runner-api",
            Self::Worker(_) => "worker",
            Self::Migrate(_) => "migrate",
            Self::All(_) => "all",
        }
    }
}

/// The installed telemetry: the metrics recorder the scrape renders, and the tracer provider whose
/// spans are flushed when the process stops.
pub(crate) struct Telemetry {
    metrics: PrometheusHandle,
    tracer: SdkTracerProvider,
}

struct Rendered(PrometheusHandle);

impl Scrape for Rendered {
    fn render(&self) -> String {
        self.0.render()
    }
}

/// Whether the environment names an OTLP endpoint to export traces to.
fn exporting() -> bool {
    [
        "OTEL_EXPORTER_OTLP_TRACES_ENDPOINT",
        "OTEL_EXPORTER_OTLP_ENDPOINT",
    ]
    .iter()
    .any(|name| std::env::var(name).is_ok_and(|value| !value.is_empty()))
}

/// The protocol the environment names for the trace export; `http/protobuf` where it names none.
fn protocol() -> Result<Protocol, Error> {
    let named = [
        "OTEL_EXPORTER_OTLP_TRACES_PROTOCOL",
        "OTEL_EXPORTER_OTLP_PROTOCOL",
    ]
    .iter()
    .find_map(|name| std::env::var(name).ok().filter(|value| !value.is_empty()));
    match named.as_deref() {
        None | Some("http/protobuf") => Ok(Protocol::HttpBinary),
        Some("http/json") => Ok(Protocol::HttpJson),
        Some(other) => Err(Error::Telemetry(format!(
            "the OTLP protocol {other:?} is not supported; use http/protobuf or http/json"
        ))),
    }
}

/// Builds the span exporter on a thread of its own: its blocking HTTP client may not be built on
/// a runtime's thread.
fn exporter() -> Result<SpanExporter, Error> {
    let protocol = protocol()?;
    std::thread::spawn(move || {
        SpanExporter::builder()
            .with_http()
            .with_protocol(protocol)
            .build()
            .map_err(|error| Error::Telemetry(format!("building the trace exporter: {error}")))
    })
    .join()
    .map_err(|_| Error::Telemetry("building the trace exporter panicked".to_owned()))?
}

fn describe() {
    metrics::describe_histogram!(
        "codehub_http_request_duration_seconds",
        metrics::Unit::Seconds,
        "API requests, by method, route template and status"
    );
    metrics::describe_histogram!(
        observability::LANDING_GATE_WAIT,
        metrics::Unit::Seconds,
        "Time a landing waited at its repository's gate"
    );
    metrics::describe_histogram!(
        observability::LANDING_GATE_HOLD,
        metrics::Unit::Seconds,
        "Time a landing held its repository's gate"
    );
    metrics::describe_histogram!(
        observability::LANDING_ATTEMPTS,
        "Attempts a landing made at the swap of main"
    );
    metrics::describe_counter!(
        observability::LANDING_SWAPS_LOST,
        "Compare-and-swaps of main lost to a main that moved first"
    );
    metrics::describe_histogram!(
        observability::OUTBOUND_DURATION,
        metrics::Unit::Seconds,
        "Calls to a dependency, by dependency"
    );
    metrics::describe_counter!(
        observability::OUTBOUND_ERRORS,
        "Calls to a dependency that failed, by dependency"
    );
    metrics::describe_gauge!(
        observability::QUEUE_DEPTH,
        "Queued jobs and runs, by kind and labels"
    );
    metrics::describe_gauge!(
        observability::QUEUE_OLDEST_AGE,
        metrics::Unit::Seconds,
        "Seconds the oldest queued job or run has waited, by kind and labels"
    );
    metrics::describe_gauge!(
        observability::OUTBOX_PENDING,
        "Directory outbox entries not yet delivered"
    );
    metrics::describe_gauge!(
        observability::DB_POOL_CONNECTIONS,
        "Connections the database pool has open"
    );
    metrics::describe_gauge!(
        observability::DB_POOL_IN_USE,
        "Connections the database pool has lent out"
    );
    metrics::describe_gauge!(
        observability::DB_POOL_MAX,
        "Connections the database pool opens at most"
    );
    metrics::describe_histogram!(
        observability::DB_POOL_ACQUIRE,
        metrics::Unit::Seconds,
        "Time to take a connection from the database pool, or to give up on taking one, probed every few seconds"
    );
    metrics::describe_counter!(
        observability::DB_POOL_ACQUIRE_FAILURES,
        "Probes that failed to take a connection from the database pool"
    );
}

impl Telemetry {
    /// Installs the metrics recorder, the trace context propagator and the subscriber that logs and
    /// exports spans, for the process of `role`. Spans are exported where the environment names an
    /// OTLP endpoint (`OTEL_EXPORTER_OTLP_ENDPOINT`); they always carry trace ids, so a log line
    /// names its trace either way.
    pub(crate) fn install(options: &TelemetryOptions, role: &'static str) -> Result<Self, Error> {
        let metrics = PrometheusBuilder::new()
            .set_buckets(SECONDS_BUCKETS)
            .map_err(|error| Error::Telemetry(format!("setting the buckets: {error}")))?
            .set_buckets_for_metric(
                Matcher::Full(observability::LANDING_ATTEMPTS.to_owned()),
                ATTEMPT_BUCKETS,
            )
            .map_err(|error| Error::Telemetry(format!("setting the buckets: {error}")))?
            .idle_timeout(MetricKindMask::GAUGE, Some(GAUGE_IDLE_TIMEOUT))
            .install_recorder()
            .map_err(|error| Error::Telemetry(format!("installing the recorder: {error}")))?;
        describe();

        opentelemetry::global::set_text_map_propagator(TraceContextPropagator::new());
        let service = std::env::var("OTEL_SERVICE_NAME")
            .ok()
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| "codehub".to_owned());
        let resource = Resource::builder()
            .with_service_name(service)
            .with_attribute(KeyValue::new("codehub.role", role))
            .build();
        let mut provider = SdkTracerProvider::builder().with_resource(resource);
        if exporting() {
            provider = provider.with_batch_exporter(exporter()?);
        }
        let tracer = provider.build();

        let filter = EnvFilter::try_new(&options.log_filter)
            .map_err(|error| Error::Telemetry(format!("--log-filter: {error}")))?;
        let lines = tracing_subscriber::fmt::layer().with_writer(std::io::stdout);
        // The filter is the log's: the spans the trace export takes are the ones at INFO and
        // above whatever the log keeps.
        let lines = match options.log_format {
            LogFormat::Json => lines
                .json()
                .with_current_span(true)
                .with_span_list(false)
                .with_filter(filter)
                .boxed(),
            LogFormat::Text => lines.with_filter(filter).boxed(),
        };
        let spans = tracing_opentelemetry::layer()
            .with_tracer(tracer.tracer("codehub"))
            .with_filter(LevelFilter::INFO);
        tracing_subscriber::registry()
            .with(lines)
            .with(spans)
            .try_init()
            .map_err(|error| Error::Telemetry(format!("installing the subscriber: {error}")))?;
        Ok(Self { metrics, tracer })
    }

    /// What `/metrics` serves.
    pub(crate) fn scrape(&self) -> Arc<dyn Scrape> {
        Arc::new(Rendered(self.metrics.clone()))
    }

    /// Samples `database`'s pool every few seconds, and, where `backlog` is given, reads it every
    /// `interval`. The sampling stops when the returned guard is dropped.
    pub(crate) fn watch(
        database: Database,
        backlog: Option<(Arc<dyn Backlog>, Duration)>,
    ) -> Watching {
        let mut tasks = vec![tokio::spawn(sample_pool(database))];
        if let Some((backlog, interval)) = backlog {
            tasks.push(tokio::spawn(measure_backlog(backlog, interval)));
        }
        Watching(tasks)
    }

    /// Flushes the spans not yet exported and stops the exporter.
    pub(crate) async fn shutdown(self) {
        let tracer = self.tracer;
        let stopped = tokio::task::spawn_blocking(move || tracer.shutdown()).await;
        if !matches!(stopped, Ok(Ok(()))) {
            // Nowhere left to report a failed stderr write.
            let _ = writeln!(
                std::io::stderr(),
                "the trace exporter did not shut down cleanly"
            );
        }
    }
}

/// The tasks that sample the process, stopped on drop.
pub(crate) struct Watching(Vec<JoinHandle<()>>);

impl Drop for Watching {
    fn drop(&mut self) {
        for task in &self.0 {
            task.abort();
        }
    }
}

async fn sample_pool(database: Database) {
    loop {
        let state = database.pool_state();
        metrics::gauge!(observability::DB_POOL_CONNECTIONS).set(f64::from(state.open));
        metrics::gauge!(observability::DB_POOL_IN_USE)
            .set(f64::from(state.open.saturating_sub(state.idle)));
        metrics::gauge!(observability::DB_POOL_MAX).set(f64::from(state.max));
        let started = Instant::now();
        match database.probe_acquire().await {
            Ok(waited) => {
                metrics::histogram!(observability::DB_POOL_ACQUIRE).record(waited.as_secs_f64());
            }
            Err(error) => {
                // A pool that cannot lend a connection is the wait this histogram is for: the
                // time given up on counts, and so does the failure.
                metrics::histogram!(observability::DB_POOL_ACQUIRE)
                    .record(started.elapsed().as_secs_f64());
                metrics::counter!(observability::DB_POOL_ACQUIRE_FAILURES).increment(1);
                tracing::warn!(%error, "probing the database pool failed");
            }
        }
        tokio::time::sleep(POOL_SAMPLE_INTERVAL).await;
    }
}

async fn measure_backlog(backlog: Arc<dyn Backlog>, interval: Duration) {
    let gauges = BacklogGauges::new();
    loop {
        if let Err(error) = gauges.update(&*backlog).await {
            tracing::warn!(%error, "measuring the backlog failed");
        }
        tokio::time::sleep(interval).await;
    }
}

#[cfg(test)]
mod tests {
    use super::{MAX_BACKLOG_INTERVAL, backlog_interval};

    #[test]
    fn the_backlog_interval_is_from_one_to_the_longest_the_gauges_outlive() {
        assert_eq!(backlog_interval("1").unwrap().get(), 1);
        assert_eq!(
            backlog_interval(&MAX_BACKLOG_INTERVAL.to_string())
                .unwrap()
                .get(),
            MAX_BACKLOG_INTERVAL
        );
        assert!(backlog_interval("0").is_err());
        assert!(backlog_interval(&(MAX_BACKLOG_INTERVAL + 1).to_string()).is_err());
        assert!(backlog_interval("soon").is_err());
    }
}
