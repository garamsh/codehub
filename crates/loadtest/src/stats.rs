//! What an agent records, what a scenario reports, and how the report is printed.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::time::Duration;

use serde::Serialize;

/// The kind of operation a latency belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    /// Upload grant, upload to the store and stage call for one path.
    Stage,
    /// The land call.
    Land,
    /// The file lookup and the download from the store.
    Read,
    /// The history call.
    Log,
}

impl Kind {
    /// Every kind, in the order a report lists them.
    pub const ALL: [Self; 4] = [Self::Stage, Self::Land, Self::Read, Self::Log];

    const fn index(self) -> usize {
        match self {
            Self::Stage => 0,
            Self::Land => 1,
            Self::Read => 2,
            Self::Log => 3,
        }
    }

    /// The name a report prints.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Stage => "stage",
            Self::Land => "land",
            Self::Read => "read",
            Self::Log => "log",
        }
    }
}

/// The status an operation ended with; 0 where no answer came at all.
pub type Status = u16;

/// Whether `status` is a success.
pub const fn succeeded(status: Status) -> bool {
    matches!(status, 200..=299)
}

/// Whether a non-success `status` of `kind` is one the harness expects: a landing that lost the
/// compare-and-swap, or conflicted with a landing made since its base, answers 409 and is retried.
const fn expected(kind: Kind, status: Status) -> bool {
    matches!(kind, Kind::Land) && status == 409
}

/// What one agent measured; recorders merge into the scenario's.
#[derive(Default)]
pub struct Recorder {
    latencies: [Vec<Duration>; 4],
    errors: BTreeMap<Status, ErrorCount>,
    operations: u64,
    landings: u64,
    landings_retried: u64,
    retries: u64,
    abandoned: u64,
}

#[derive(Default, Clone, Copy)]
struct ErrorCount {
    expected: u64,
    unexpected: u64,
}

impl Recorder {
    /// Records one timed operation of `kind` that ended with `status`.
    pub fn record(&mut self, kind: Kind, latency: Duration, status: Status) {
        if let Some(samples) = self.latencies.get_mut(kind.index()) {
            samples.push(latency);
        }
        if succeeded(status) {
            return;
        }
        let count = self.errors.entry(status).or_default();
        if expected(kind, status) {
            count.expected += 1;
        } else {
            count.unexpected += 1;
        }
    }

    /// Records one finished operation of a scenario: a landing, a read or a log.
    pub const fn operation(&mut self) {
        self.operations += 1;
    }

    /// Records a landing that succeeded after `retries` lost attempts.
    pub const fn landed(&mut self, retries: u64) {
        self.landings += 1;
        self.retries += retries;
        if retries > 0 {
            self.landings_retried += 1;
        }
    }

    /// Records the lost attempts of a landing that did not succeed.
    pub const fn gave_up(&mut self, retries: u64) {
        self.retries += retries;
    }

    /// Records a landing given up on after losing every attempt it was allowed.
    pub const fn abandoned(&mut self) {
        self.abandoned += 1;
    }

    /// Adds what `other` recorded.
    pub fn merge(&mut self, other: Self) {
        for (mine, theirs) in self.latencies.iter_mut().zip(other.latencies) {
            mine.extend(theirs);
        }
        for (status, count) in other.errors {
            let mine = self.errors.entry(status).or_default();
            mine.expected += count.expected;
            mine.unexpected += count.unexpected;
        }
        self.operations += other.operations;
        self.landings += other.landings;
        self.landings_retried += other.landings_retried;
        self.retries += other.retries;
        self.abandoned += other.abandoned;
    }

    /// The figures of a scenario that ran for `elapsed`.
    pub fn report(mut self, scenario: &str, agents: usize, elapsed: Duration) -> Report {
        let latency = Kind::ALL
            .iter()
            .filter_map(|kind| {
                let samples = self.latencies.get_mut(kind.index())?;
                samples.sort_unstable();
                Some(Latency {
                    kind: *kind,
                    count: samples.len() as u64,
                    p50_ms: percentile(samples, 50),
                    p95_ms: percentile(samples, 95),
                    p99_ms: percentile(samples, 99),
                })
            })
            .collect();
        let seconds = elapsed.as_secs_f64();
        Report {
            scenario: scenario.to_owned(),
            agents,
            elapsed_seconds: seconds,
            operations: self.operations,
            operations_per_second: ratio(self.operations, seconds),
            latency,
            landings: self.landings,
            landings_retried: self.landings_retried,
            retries: self.retries,
            retries_per_landing: ratio(self.retries, self.landings as f64),
            abandoned: self.abandoned,
            errors: self
                .errors
                .into_iter()
                .map(|(status, count)| StatusCount {
                    status,
                    expected: count.expected,
                    unexpected: count.unexpected,
                })
                .collect(),
            external_calls: None,
        }
    }
}

fn ratio(count: u64, per: f64) -> f64 {
    if per > 0.0 { count as f64 / per } else { 0.0 }
}

/// The nearest-rank percentile `p` of the sorted `samples`, in milliseconds; 0 for no samples.
fn percentile(samples: &[Duration], p: usize) -> f64 {
    let rank = (samples.len() * p).div_ceil(100).max(1);
    samples
        .get(rank - 1)
        .map_or(0.0, |sample| sample.as_secs_f64() * 1000.0)
}

/// Latency percentiles of one kind of operation.
#[derive(Serialize)]
pub struct Latency {
    kind: Kind,
    count: u64,
    p50_ms: f64,
    p95_ms: f64,
    p99_ms: f64,
}

/// How many times one non-success status was answered.
#[derive(Serialize)]
pub struct StatusCount {
    status: Status,
    /// A landing's 409, retried.
    expected: u64,
    unexpected: u64,
}

/// What the counting proxies saw during a scenario's measured period.
#[derive(Serialize)]
pub struct ExternalCalls {
    /// Requests the agents made to the API.
    pub api_requests: u64,
    /// Requests that reached the decision point.
    pub decision_calls: u64,
    /// Requests that reached the introspection endpoint.
    pub introspection_calls: u64,
    /// `decision_calls` per API request.
    pub decisions_per_request: f64,
    /// `introspection_calls` per API request.
    pub introspections_per_request: f64,
}

impl ExternalCalls {
    /// The calls the proxies counted against `api_requests`.
    pub fn new(api_requests: u64, decision_calls: u64, introspection_calls: u64) -> Self {
        let requests = api_requests as f64;
        Self {
            api_requests,
            decision_calls,
            introspection_calls,
            decisions_per_request: ratio(decision_calls, requests),
            introspections_per_request: ratio(introspection_calls, requests),
        }
    }
}

/// The figures of one scenario.
#[derive(Serialize)]
pub struct Report {
    scenario: String,
    agents: usize,
    elapsed_seconds: f64,
    operations: u64,
    operations_per_second: f64,
    latency: Vec<Latency>,
    landings: u64,
    landings_retried: u64,
    retries: u64,
    retries_per_landing: f64,
    /// Landings given up on after losing every attempt.
    abandoned: u64,
    errors: Vec<StatusCount>,
    /// Set by the `external` scenario.
    pub external_calls: Option<ExternalCalls>,
}

impl Report {
    /// Whether no operation answered a status the scenario does not expect.
    pub fn clean(&self) -> bool {
        self.abandoned == 0 && self.errors.iter().all(|count| count.unexpected == 0)
    }

    /// The table printed to stdout.
    pub fn table(&self) -> String {
        let mut text = String::new();
        let _ = writeln!(
            text,
            "scenario {}: {} agents, {:.1} s, {} operations, {:.1} operations/s",
            self.scenario,
            self.agents,
            self.elapsed_seconds,
            self.operations,
            self.operations_per_second
        );
        let _ = writeln!(
            text,
            "  {:<6} {:>8} {:>10} {:>10} {:>10}",
            "kind", "count", "p50 ms", "p95 ms", "p99 ms"
        );
        for latency in &self.latency {
            let _ = writeln!(
                text,
                "  {:<6} {:>8} {:>10.1} {:>10.1} {:>10.1}",
                latency.kind.name(),
                latency.count,
                latency.p50_ms,
                latency.p95_ms,
                latency.p99_ms
            );
        }
        let _ = writeln!(
            text,
            "  landings {}, of which retried {}; retries {} ({:.3} per landing)",
            self.landings, self.landings_retried, self.retries, self.retries_per_landing
        );
        if self.abandoned > 0 {
            let _ = writeln!(
                text,
                "  landings abandoned after repeated 409: {}",
                self.abandoned
            );
        }
        if self.errors.is_empty() {
            let _ = writeln!(text, "  errors: none");
        }
        for count in &self.errors {
            let _ = writeln!(
                text,
                "  errors: status {}: {} expected, {} unexpected",
                count.status, count.expected, count.unexpected
            );
        }
        if let Some(calls) = &self.external_calls {
            let _ = writeln!(
                text,
                "  external: {} API requests; decision point {} calls ({:.2} per request); introspection {} calls ({:.2} per request)",
                calls.api_requests,
                calls.decision_calls,
                calls.decisions_per_request,
                calls.introspection_calls,
                calls.introspections_per_request
            );
        }
        text
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn millis(values: &[u64]) -> Vec<Duration> {
        values.iter().map(|ms| Duration::from_millis(*ms)).collect()
    }

    #[test]
    fn takes_the_nearest_rank_percentile_of_sorted_samples() {
        let samples = millis(&(1..=100).collect::<Vec<_>>());
        assert_eq!(percentile(&samples, 50), 50.0);
        assert_eq!(percentile(&samples, 95), 95.0);
        assert_eq!(percentile(&samples, 99), 99.0);
        assert_eq!(percentile(&millis(&[7]), 99), 7.0);
        assert_eq!(percentile(&[], 50), 0.0);
    }

    #[test]
    fn counts_a_lost_landing_as_expected_and_any_other_failure_as_unexpected() {
        let mut recorder = Recorder::default();
        recorder.record(Kind::Land, Duration::ZERO, 409);
        recorder.record(Kind::Land, Duration::ZERO, 200);
        assert!(recorder.report("t", 1, Duration::from_secs(1)).clean());

        // The control: the same status of another kind, and another status of a landing, are not.
        for (kind, status) in [(Kind::Read, 409), (Kind::Land, 500), (Kind::Stage, 0)] {
            let mut recorder = Recorder::default();
            recorder.record(kind, Duration::ZERO, status);
            assert!(!recorder.report("t", 1, Duration::from_secs(1)).clean());
        }

        // A landing given up on after repeated 409s is not clean either.
        let mut recorder = Recorder::default();
        recorder.abandoned();
        assert!(!recorder.report("t", 1, Duration::from_secs(1)).clean());
    }

    #[test]
    fn merges_what_agents_recorded_and_derives_retries_per_landing() {
        let mut first = Recorder::default();
        first.record(Kind::Land, Duration::from_millis(10), 409);
        first.landed(1);
        first.operation();
        let mut second = Recorder::default();
        second.record(Kind::Land, Duration::from_millis(30), 200);
        second.landed(0);
        second.operation();
        first.merge(second);

        let report = first.report("t", 2, Duration::from_secs(2));
        assert_eq!(report.operations, 2);
        assert_eq!(report.landings, 2);
        assert_eq!(report.landings_retried, 1);
        assert_eq!(report.retries, 1);
        assert_eq!(report.retries_per_landing, 0.5);
        assert_eq!(report.operations_per_second, 1.0);
        let land = report.latency.iter().find(|l| l.kind == Kind::Land);
        assert_eq!(land.map(|l| l.count), Some(2));
    }
}
