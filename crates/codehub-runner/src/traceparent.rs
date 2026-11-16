//! The `traceparent` header the runner sends with every call to the server (ADR 0056), so that the
//! server's spans for one job's calls share one trace.
//!
//! The runner exports no spans of its own: it names the trace and the server records it. A call
//! for a job takes its trace id from the job's identifier, so a job's heartbeats and its result
//! are one trace; a call for no job, the claim, starts a trace of its own.

use rustix::rand::{GetRandomFlags, getrandom};
use sha2::{Digest, Sha256};

fn random<const N: usize>() -> Option<[u8; N]> {
    let mut bytes = [0_u8; N];
    let mut filled = 0;
    while filled < N {
        let rest = bytes.get_mut(filled..)?;
        filled += getrandom(rest, GetRandomFlags::empty()).ok()?;
    }
    Some(bytes)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// The job a runner path is for: `/v1/runner/jobs/<id>/...`, other than the claim.
fn job_of(path: &str) -> Option<&str> {
    let id = path.strip_prefix("/v1/runner/jobs/")?.split('/').next()?;
    (!id.is_empty() && id != "claim").then_some(id)
}

/// The header value for a call to `path`, or `None` where the system has no random bytes to make
/// one of; a call goes without it then.
pub(crate) fn for_path(path: &str) -> Option<String> {
    let trace = match job_of(path) {
        Some(job) => {
            let digest = Sha256::digest(format!("codehub-job:{job}").as_bytes());
            hex(digest.get(..16)?)
        }
        None => hex(&random::<16>()?),
    };
    // The span id of the call: the server's span for it is a child of this id.
    let span = hex(&random::<8>()?);
    Some(format!("00-{trace}-{span}-01"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn trace(header: &str) -> &str {
        header.split('-').nth(1).unwrap()
    }

    #[test]
    fn is_a_w3c_traceparent() {
        let header = for_path("/v1/runner/jobs/7/heartbeat").unwrap();
        let parts: Vec<&str> = header.split('-').collect();
        assert_eq!(parts.len(), 4, "{header}");
        assert_eq!(parts[0], "00");
        assert_eq!((parts[1].len(), parts[2].len(), parts[3]), (32, 16, "01"));
        assert!(
            header
                .bytes()
                .all(|byte| byte == b'-' || byte.is_ascii_hexdigit())
        );
    }

    #[test]
    fn one_jobs_calls_share_a_trace_and_each_has_a_span() {
        let heartbeat = for_path("/v1/runner/jobs/7/heartbeat").unwrap();
        let result = for_path("/v1/runner/jobs/7/result").unwrap();
        let other = for_path("/v1/runner/jobs/8/heartbeat").unwrap();
        assert_eq!(trace(&heartbeat), trace(&result));
        assert_ne!(trace(&heartbeat), trace(&other));
        assert_ne!(heartbeat, result);
    }

    #[test]
    fn a_claim_starts_a_trace_of_its_own() {
        let first = for_path("/v1/runner/jobs/claim").unwrap();
        let second = for_path("/v1/runner/jobs/claim").unwrap();
        assert_ne!(trace(&first), trace(&second));
    }
}
