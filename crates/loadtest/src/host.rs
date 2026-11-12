//! What the figures are of: the commit and the machine.

use std::process::Command;

use serde::Serialize;

/// The commit and the machine a run measured.
#[derive(Serialize)]
pub struct Host {
    /// `git rev-parse HEAD` of the working directory.
    commit: String,
    /// The first `model name` of `/proc/cpuinfo`.
    cpu_model: String,
    /// Logical CPUs available to the process.
    cpu_count: usize,
    /// `MemTotal` of `/proc/meminfo`, in bytes.
    memory_bytes: u64,
}

impl Host {
    /// Reads the commit and the machine; a figure that cannot be read is `unknown` or 0.
    pub fn read() -> Self {
        let commit = Command::new("git")
            .args(["rev-parse", "HEAD"])
            .output()
            .ok()
            .filter(|output| output.status.success())
            .map_or_else(
                || "unknown".to_owned(),
                |output| String::from_utf8_lossy(&output.stdout).trim().to_owned(),
            );
        let cpuinfo = std::fs::read_to_string("/proc/cpuinfo").unwrap_or_default();
        let meminfo = std::fs::read_to_string("/proc/meminfo").unwrap_or_default();
        Self {
            commit,
            cpu_model: field(&cpuinfo, "model name").unwrap_or_else(|| "unknown".to_owned()),
            cpu_count: std::thread::available_parallelism().map_or(0, usize::from),
            memory_bytes: field(&meminfo, "MemTotal")
                .and_then(|value| value.trim_end_matches("kB").trim().parse::<u64>().ok())
                .map_or(0, |kilobytes| kilobytes * 1024),
        }
    }

    /// The line the table opens with.
    pub fn line(&self) -> String {
        format!(
            "commit {}; {} x {}; {} MiB memory",
            self.commit,
            self.cpu_count,
            self.cpu_model,
            self.memory_bytes / (1024 * 1024)
        )
    }
}

/// The value of the first line of `text` that is `name`, a colon and a value.
fn field(text: &str, name: &str) -> Option<String> {
    text.lines().find_map(|line| {
        let (key, value) = line.split_once(':')?;
        (key.trim() == name).then(|| value.trim().to_owned())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_first_line_that_names_the_field() {
        let text =
            "processor\t: 0\nmodel name\t: Fast CPU\nmodel name\t: Other\nMemTotal:  2048 kB\n";
        assert_eq!(field(text, "model name").as_deref(), Some("Fast CPU"));
        assert_eq!(field(text, "MemTotal").as_deref(), Some("2048 kB"));
        assert_eq!(field(text, "missing"), None);
    }
}
