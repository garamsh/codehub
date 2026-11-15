//! The CPU the compose PostgreSQL service uses during a scenario, read every second from its
//! cgroup's `cpu.stat` counter.

use std::time::{Duration, Instant};

use serde::Serialize;
use tokio::process::Command;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

use crate::Failure;

/// The compose project and service whose container is sampled.
const PROJECT: &str = "codehub";
const SERVICE: &str = "postgres";

/// Where the cgroup v2 hierarchy is mounted.
const CGROUP_ROOT: &str = "/sys/fs/cgroup";

/// The time between two readings.
const INTERVAL: Duration = Duration::from_secs(1);

/// The CPU used over one interval.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct Sample {
    /// Milliseconds from the start of the sampling to the end of the interval.
    pub t_ms: u64,
    /// CPU time used per second of wall time over the interval.
    pub cores: f64,
}

/// What a scenario's samples add up to.
#[derive(Serialize)]
pub struct PostgresCpu {
    pub median_cores: f64,
    pub max_cores: f64,
    pub samples: Vec<Sample>,
}

impl PostgresCpu {
    /// The median and the maximum of `samples`; 0 for none.
    pub fn of(samples: Vec<Sample>) -> Self {
        let mut cores: Vec<f64> = samples.iter().map(|sample| sample.cores).collect();
        cores.sort_by(f64::total_cmp);
        let at = |index: usize| cores.get(index).copied().unwrap_or(0.0);
        // The two middle samples, which are one where the count is odd.
        let median = f64::midpoint(at(cores.len().saturating_sub(1) / 2), at(cores.len() / 2));
        Self {
            median_cores: median,
            max_cores: cores.last().copied().unwrap_or(0.0),
            samples,
        }
    }
}

/// Cores used between two readings of `usage_usec` taken `wall` apart.
fn cores(before: u64, after: u64, wall: Duration) -> f64 {
    let seconds = wall.as_secs_f64();
    if seconds > 0.0 {
        after.saturating_sub(before) as f64 / 1_000_000.0 / seconds
    } else {
        0.0
    }
}

/// The `usage_usec` line of a cgroup v2 `cpu.stat`.
fn usage_usec(stat: &str) -> Option<u64> {
    stat.lines()
        .find_map(|line| line.strip_prefix("usage_usec "))
        .and_then(|value| value.trim().parse().ok())
}

/// The cgroup v2 path in the contents of a `/proc/<pid>/cgroup`, which is the line `0::<path>`.
fn cgroup_path(proc_cgroup: &str) -> Option<&str> {
    proc_cgroup
        .lines()
        .find_map(|line| line.strip_prefix("0::"))
}

/// The output of `program` with `arguments`, trimmed.
async fn output(program: &str, arguments: &[&str]) -> Result<String, Failure> {
    let run = Command::new(program).args(arguments).output().await?;
    if !run.status.success() {
        return Err(Failure::new(format!(
            "`{program} {}` failed: {}",
            arguments.join(" "),
            run.status
        )));
    }
    Ok(String::from_utf8_lossy(&run.stdout).trim().to_owned())
}

/// The `cpu.stat` of the compose PostgreSQL container.
async fn stat_path() -> Result<String, Failure> {
    let container = output("docker", &["compose", "-p", PROJECT, "ps", "-q", SERVICE]).await?;
    if container.is_empty() {
        return Err(Failure::new(format!(
            "no running `{SERVICE}` container in compose project `{PROJECT}`; run `make services`"
        )));
    }
    let pid = output(
        "docker",
        &["inspect", "--format", "{{.State.Pid}}", &container],
    )
    .await?;
    let proc_path = format!("/proc/{pid}/cgroup");
    let proc_cgroup = std::fs::read_to_string(&proc_path)?;
    let path = cgroup_path(&proc_cgroup)
        .ok_or_else(|| Failure::new(format!("{proc_path} names no cgroup v2 path")))?;
    Ok(format!("{CGROUP_ROOT}{path}/cpu.stat"))
}

/// Reads the counter in `path`, in microseconds.
fn read_usage(path: &str) -> Result<u64, Failure> {
    let stat = std::fs::read_to_string(path)
        .map_err(|error| Failure::new(format!("cannot read {path}: {error}")))?;
    usage_usec(&stat).ok_or_else(|| Failure::new(format!("{path} has no usage_usec")))
}

/// A task reading the PostgreSQL container's CPU every second until stopped.
pub struct Sampler {
    stop: oneshot::Sender<()>,
    task: JoinHandle<Result<Vec<Sample>, Failure>>,
}

impl Sampler {
    /// Finds the container's `cpu.stat`, takes the first reading and starts sampling.
    ///
    /// # Errors
    ///
    /// Fails where the container is not running or the host has no readable `cpu.stat` for it.
    pub async fn start() -> Result<Self, Failure> {
        let path = stat_path().await?;
        let mut before = read_usage(&path)?;
        let started = Instant::now();
        let (stop, mut stopped) = oneshot::channel();
        let task = tokio::spawn(async move {
            let mut samples = Vec::new();
            let mut last = started;
            let mut ticker =
                tokio::time::interval_at(tokio::time::Instant::now() + INTERVAL, INTERVAL);
            loop {
                tokio::select! {
                    _ = &mut stopped => return Ok(samples),
                    _ = ticker.tick() => {}
                }
                let after = read_usage(&path)?;
                let now = Instant::now();
                samples.push(Sample {
                    t_ms: u64::try_from(now.duration_since(started).as_millis())
                        .unwrap_or(u64::MAX),
                    cores: cores(before, after, now.duration_since(last)),
                });
                before = after;
                last = now;
            }
        });
        Ok(Self { stop, task })
    }

    /// Stops sampling and returns the samples.
    ///
    /// # Errors
    ///
    /// Fails where a reading failed during the run.
    pub async fn stop(self) -> Result<Vec<Sample>, Failure> {
        let _ = self.stop.send(());
        self.task
            .await
            .map_err(|error| Failure::new(format!("the CPU sampler failed: {error}")))?
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(cores: f64) -> Sample {
        Sample { t_ms: 0, cores }
    }

    #[test]
    fn derives_cores_from_two_counter_readings() {
        // 3 CPU-seconds over 2 s of wall time is 1.5 cores.
        assert_eq!(cores(1_000_000, 4_000_000, Duration::from_secs(2)), 1.5);
        // The control: the same readings over a different wall time give a different figure.
        assert_eq!(cores(1_000_000, 4_000_000, Duration::from_secs(1)), 3.0);
        // No wall time and a counter that went backwards are 0, not a made-up figure.
        assert_eq!(cores(0, 5, Duration::ZERO), 0.0);
        assert_eq!(cores(9, 5, Duration::from_secs(1)), 0.0);
    }

    #[test]
    fn takes_the_median_and_maximum_of_the_samples() {
        let odd = PostgresCpu::of(vec![sample(3.0), sample(1.0), sample(2.0)]);
        assert_eq!(odd.median_cores, 2.0);
        assert_eq!(odd.max_cores, 3.0);
        // An even count takes the mean of the two middle samples.
        let even = PostgresCpu::of(vec![sample(4.0), sample(1.0), sample(2.0), sample(3.0)]);
        assert_eq!(even.median_cores, 2.5);
        assert_eq!(even.max_cores, 4.0);
        let one = PostgresCpu::of(vec![sample(0.5)]);
        assert_eq!((one.median_cores, one.max_cores), (0.5, 0.5));
        let none = PostgresCpu::of(Vec::new());
        assert_eq!((none.median_cores, none.max_cores), (0.0, 0.0));
    }

    #[test]
    fn reads_usage_usec_and_the_cgroup_path() {
        let stat = "usage_usec 815025582\nuser_usec 509498641\nsystem_usec 305526941\n";
        assert_eq!(usage_usec(stat), Some(815_025_582));
        // The control: a line that only ends in the name, and a missing line, are not it.
        assert_eq!(usage_usec("user_usec 5\nnr_usage_usec 7\n"), None);
        assert_eq!(usage_usec(""), None);

        let proc_cgroup = "0::/system.slice/docker-abc.scope\n";
        assert_eq!(
            cgroup_path(proc_cgroup),
            Some("/system.slice/docker-abc.scope")
        );
        // A cgroup v1 listing has no `0::` line.
        assert_eq!(cgroup_path("12:cpu,cpuacct:/docker/abc\n"), None);
    }
}
