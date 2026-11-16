use std::collections::{BTreeMap, BTreeSet};
use std::sync::Mutex;

use app::domain::address::Address;
use app::domain::check::CheckKey;
use app::domain::draft::Instance;
use app::domain::job::{
    Heartbeat, Job, JobId, JobResult, JobState, NewJob, Resolution, StopReason,
};
use app::domain::name::{DraftName, OrganisationName};
use app::domain::principal::Principal;
use app::domain::repository::RepositoryId;
use app::domain::revert::Revert;
use app::domain::run::{NewRun, Run, RunId, RunResult, RunState};
use app::domain::runner::{Registration, RegistrationId};
use app::port::driven::job_queue::{JobQueue, JobQueueError};

use crate::lock;

#[derive(Default)]
struct Jobs {
    issued: u64,
    held: BTreeMap<JobId, Job>,
    runs: BTreeMap<RunId, Run>,
    reserved: BTreeSet<(RepositoryId, Address)>,
}

impl Jobs {
    fn add(&mut self, new: NewJob, state: JobState, output: Vec<u8>) -> Job {
        self.issued = self.issued.saturating_add(1);
        let job = Job {
            id: JobId::new(self.issued),
            organisation: new.organisation,
            repository: new.repository,
            changeset: new.changeset,
            check: new.check,
            state,
            key: None,
            output_tree: None,
            runner: None,
            exit_code: None,
            output,
            revert: None,
        };
        self.held.insert(job.id, job.clone());
        job
    }
}

/// A job queue in memory; it numbers jobs from 1 in the order they were queued.
#[derive(Default)]
pub struct MemoryJobQueue {
    jobs: Mutex<Jobs>,
}

fn not_found(run: RunId) -> JobQueueError {
    JobQueueError::NotFound(JobId::new(run.value()))
}

fn seconds(timeout: u64) -> i64 {
    i64::try_from(timeout).unwrap_or(i64::MAX)
}

#[async_trait::async_trait]
impl JobQueue for MemoryJobQueue {
    async fn queue(&self, jobs: Vec<NewJob>) -> Result<Vec<JobId>, JobQueueError> {
        let mut held = lock(&self.jobs);
        Ok(jobs
            .into_iter()
            .map(|new| held.add(new, JobState::Waiting, Vec::new()).id)
            .collect())
    }

    async fn resolve(&self, job: JobId, resolution: Resolution) -> Result<bool, JobQueueError> {
        let mut held = lock(&self.jobs);
        let found = held
            .held
            .get_mut(&job)
            .ok_or(JobQueueError::NotFound(job))?;
        if found.state != JobState::Waiting {
            return Ok(false);
        }
        match resolution {
            Resolution::Queue { key } => {
                found.state = JobState::Queued;
                found.key = Some(key);
            }
            Resolution::Cache { key, output_tree } => {
                found.state = JobState::Cached;
                found.key = Some(key);
                found.output_tree = Some(output_tree);
            }
            Resolution::Skip => found.state = JobState::Skipped,
        }
        Ok(true)
    }

    async fn passed_with_key(
        &self,
        repository: RepositoryId,
        key: CheckKey,
    ) -> Result<Option<Address>, JobQueueError> {
        Ok(lock(&self.jobs)
            .held
            .values()
            .filter(|job| {
                job.repository == repository
                    && job.state == JobState::Passed
                    && job.key == Some(key)
            })
            .find_map(|job| job.output_tree))
    }

    async fn record_failed(&self, job: NewJob, output: Vec<u8>) -> Result<Job, JobQueueError> {
        Ok(lock(&self.jobs).add(job, JobState::Failed, output))
    }

    async fn claim(
        &self,
        registrations: &[Registration],
        now: i64,
        lease: i64,
    ) -> Result<Option<Job>, JobQueueError> {
        let mut held = lock(&self.jobs);
        for job in held.held.values_mut() {
            if job.state != JobState::Queued {
                continue;
            }
            let Some(registration) = registrations
                .iter()
                .find(|registration| registration.matches(job))
            else {
                continue;
            };
            job.state = JobState::Running {
                lease_expires_at: now.saturating_add(lease),
                deadline: now.saturating_add(seconds(job.check.timeout.seconds())),
            };
            job.runner = Some(registration.id);
            return Ok(Some(job.clone()));
        }
        Ok(None)
    }

    async fn heartbeat(
        &self,
        job: JobId,
        runners: &[RegistrationId],
        now: i64,
        lease: i64,
    ) -> Result<Heartbeat, JobQueueError> {
        let mut held = lock(&self.jobs);
        let found = held
            .held
            .get_mut(&job)
            .filter(|found| found.runner.is_some_and(|runner| runners.contains(&runner)))
            .ok_or(JobQueueError::NotFound(job))?;
        match found.state {
            JobState::Running { deadline, .. } if deadline <= now => {
                Ok(Heartbeat::Stop(StopReason::TimedOut))
            }
            JobState::Running { deadline, .. } => {
                found.state = JobState::Running {
                    lease_expires_at: now.saturating_add(lease),
                    deadline,
                };
                Ok(Heartbeat::Continue)
            }
            JobState::Cancelled => Ok(Heartbeat::Stop(StopReason::Cancelled)),
            JobState::TimedOut => Ok(Heartbeat::Stop(StopReason::TimedOut)),
            JobState::Waiting
            | JobState::Queued
            | JobState::Passed
            | JobState::Failed
            | JobState::Skipped
            | JobState::Cached => Err(JobQueueError::NotFound(job)),
        }
    }

    async fn running(&self, job: JobId, runners: &[RegistrationId]) -> Result<Job, JobQueueError> {
        lock(&self.jobs)
            .held
            .get(&job)
            .filter(|found| {
                matches!(found.state, JobState::Running { .. })
                    && found.runner.is_some_and(|runner| runners.contains(&runner))
            })
            .cloned()
            .ok_or(JobQueueError::NotFound(job))
    }

    async fn complete(
        &self,
        job: JobId,
        runners: &[RegistrationId],
        result: JobResult,
        output_tree: Address,
    ) -> Result<Job, JobQueueError> {
        let mut held = lock(&self.jobs);
        let found = held
            .held
            .get_mut(&job)
            .filter(|found| {
                matches!(found.state, JobState::Running { .. })
                    && found.runner.is_some_and(|runner| runners.contains(&runner))
            })
            .ok_or(JobQueueError::NotFound(job))?;
        found.state = result.outcome.state();
        found.output_tree = Some(output_tree);
        found.exit_code = result.exit_code;
        found.output = result.output;
        Ok(found.clone())
    }

    async fn cancel(
        &self,
        organisation: &OrganisationName,
        job: JobId,
    ) -> Result<Job, JobQueueError> {
        let mut held = lock(&self.jobs);
        let found = held
            .held
            .get_mut(&job)
            .filter(|found| found.organisation == *organisation)
            .ok_or(JobQueueError::NotFound(job))?;
        if found.state.is_finished() {
            return Err(JobQueueError::Finished(job));
        }
        found.state = JobState::Cancelled;
        Ok(found.clone())
    }

    async fn reserve_revert(&self, job: JobId) -> Result<bool, JobQueueError> {
        let mut held = lock(&self.jobs);
        let found = held.held.get(&job).ok_or(JobQueueError::NotFound(job))?;
        let key = (found.repository, found.changeset);
        Ok(held.reserved.insert(key))
    }

    async fn record_revert(&self, job: JobId, revert: Revert) -> Result<Job, JobQueueError> {
        let mut held = lock(&self.jobs);
        let found = held
            .held
            .get_mut(&job)
            .ok_or(JobQueueError::NotFound(job))?;
        found.revert = Some(revert);
        Ok(found.clone())
    }

    async fn of_changeset(
        &self,
        repository: RepositoryId,
        changeset: Address,
    ) -> Result<Vec<Job>, JobQueueError> {
        Ok(lock(&self.jobs)
            .held
            .values()
            .filter(|job| job.repository == repository && job.changeset == changeset)
            .cloned()
            .collect())
    }

    async fn queue_run(&self, run: NewRun) -> Result<Run, JobQueueError> {
        let mut held = lock(&self.jobs);
        held.issued = held.issued.saturating_add(1);
        let run = Run {
            id: RunId::new(held.issued),
            state: RunState::Queued,
            request: run,
            runner: None,
            result: None,
        };
        held.runs.insert(run.id, run.clone());
        Ok(run)
    }

    async fn run(&self, run: RunId) -> Result<Run, JobQueueError> {
        lock(&self.jobs)
            .runs
            .get(&run)
            .cloned()
            .ok_or_else(|| not_found(run))
    }

    async fn cancel_run(&self, run: RunId) -> Result<Run, JobQueueError> {
        let mut held = lock(&self.jobs);
        let found = held.runs.get_mut(&run).ok_or_else(|| not_found(run))?;
        if found.state.ended() {
            return Err(JobQueueError::Finished(JobId::new(run.value())));
        }
        found.state = RunState::Cancelled;
        Ok(found.clone())
    }

    async fn latest_run(
        &self,
        repository: RepositoryId,
        principal: &Principal,
        draft: &DraftName,
        instance: Instance,
    ) -> Result<Option<Run>, JobQueueError> {
        Ok(lock(&self.jobs)
            .runs
            .values()
            .rev()
            .find(|run| {
                run.state.ended()
                    && run.request.repository == repository
                    && run.request.principal == *principal
                    && run.request.draft == *draft
                    && run.request.instance == instance
            })
            .cloned())
    }

    async fn claim_run(
        &self,
        registrations: &[Registration],
        now: i64,
        lease: i64,
    ) -> Result<Option<Run>, JobQueueError> {
        let mut held = lock(&self.jobs);
        for run in held.runs.values_mut() {
            if run.state != RunState::Queued {
                continue;
            }
            let Some(registration) = registrations
                .iter()
                .find(|registration| registration.takes_run(&run.request))
            else {
                continue;
            };
            run.state = RunState::Running {
                lease_expires_at: now.saturating_add(lease),
                deadline: now.saturating_add(seconds(run.request.timeout.seconds())),
            };
            run.runner = Some(registration.id);
            return Ok(Some(run.clone()));
        }
        Ok(None)
    }

    async fn heartbeat_run(
        &self,
        run: RunId,
        runners: &[RegistrationId],
        now: i64,
        lease: i64,
    ) -> Result<Heartbeat, JobQueueError> {
        let mut held = lock(&self.jobs);
        let found = held
            .runs
            .get_mut(&run)
            .filter(|found| found.runner.is_some_and(|runner| runners.contains(&runner)))
            .ok_or_else(|| not_found(run))?;
        match found.state {
            RunState::Running { deadline, .. } if deadline <= now => {
                Ok(Heartbeat::Stop(StopReason::TimedOut))
            }
            RunState::Running { deadline, .. } => {
                found.state = RunState::Running {
                    lease_expires_at: now.saturating_add(lease),
                    deadline,
                };
                Ok(Heartbeat::Continue)
            }
            RunState::Cancelled => Ok(Heartbeat::Stop(StopReason::Cancelled)),
            RunState::TimedOut => Ok(Heartbeat::Stop(StopReason::TimedOut)),
            RunState::Queued | RunState::Finished => Err(not_found(run)),
        }
    }

    async fn running_run(
        &self,
        run: RunId,
        runners: &[RegistrationId],
    ) -> Result<Run, JobQueueError> {
        lock(&self.jobs)
            .runs
            .get(&run)
            .filter(|found| {
                matches!(found.state, RunState::Running { .. })
                    && found.runner.is_some_and(|runner| runners.contains(&runner))
            })
            .cloned()
            .ok_or_else(|| not_found(run))
    }

    async fn complete_run(
        &self,
        run: RunId,
        runners: &[RegistrationId],
        result: RunResult,
    ) -> Result<Run, JobQueueError> {
        let mut held = lock(&self.jobs);
        let found = held
            .runs
            .get_mut(&run)
            .filter(|found| {
                matches!(found.state, RunState::Running { .. })
                    && found.runner.is_some_and(|runner| runners.contains(&runner))
            })
            .ok_or_else(|| not_found(run))?;
        found.state = RunState::Finished;
        found.result = Some(result);
        Ok(found.clone())
    }

    async fn expire_runs(&self, now: i64) -> Result<(), JobQueueError> {
        for run in lock(&self.jobs).runs.values_mut() {
            let RunState::Running {
                lease_expires_at,
                deadline,
            } = run.state
            else {
                continue;
            };
            if deadline <= now {
                run.state = RunState::TimedOut;
            } else if lease_expires_at <= now {
                run.state = RunState::Queued;
                run.runner = None;
            }
        }
        Ok(())
    }

    async fn expire(&self, now: i64) -> Result<Vec<Job>, JobQueueError> {
        let mut timed_out = Vec::new();
        for job in lock(&self.jobs).held.values_mut() {
            let JobState::Running {
                lease_expires_at,
                deadline,
            } = job.state
            else {
                continue;
            };
            if deadline <= now {
                job.state = JobState::TimedOut;
                timed_out.push(job.clone());
            } else if lease_expires_at <= now {
                job.state = JobState::Queued;
                job.runner = None;
            }
        }
        Ok(timed_out)
    }
}
