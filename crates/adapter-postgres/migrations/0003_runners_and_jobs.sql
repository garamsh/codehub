-- Runner registrations and the job queue (ADR 0034, ADR 0035). `last-green` is a row of `refs`.

-- A registration is of an organisation, or of one repository in it; no repository means all.
create table runner_registrations (
    id bigint generated always as identity primary key,
    organisation text not null references organisations (name),
    repository bigint,
    issuer text not null,
    subject text not null,
    labels text[] not null,
    constraint runner_registrations_repository_organisation foreign key (repository, organisation)
        references repositories (id, organisation)
);

create index runner_registrations_of_organisation on runner_registrations (organisation, id);
create index runner_registrations_of_principal on runner_registrations (issuer, subject, id);

-- A job is one check of one changeset. `runner` names a registration without a foreign key: a job
-- keeps naming its runner after the registration is removed. A running job holds a lease and a
-- deadline, in seconds since 1970-01-01T00:00:00Z; no other state does. A revert is recorded as
-- the changeset it landed as, or the paths it conflicted at, never both.
create table jobs (
    id bigint generated always as identity primary key,
    organisation text not null,
    repository bigint not null,
    changeset bytea not null check (length(changeset) = 32),
    check_name text not null,
    command text not null,
    check_labels text[] not null,
    blocking boolean not null,
    timeout_seconds bigint not null check (timeout_seconds between 1 and 86400),
    state text not null
        check (state in ('queued', 'running', 'passed', 'failed', 'timed_out', 'cancelled')),
    runner bigint,
    lease_expires_at bigint,
    deadline bigint,
    exit_code integer,
    output bytea not null default ''::bytea,
    revert_landed bytea check (length(revert_landed) = 32),
    revert_conflicts bytea[],
    created_at timestamptz not null default now(),
    updated_at timestamptz not null default now(),
    constraint jobs_repository_organisation foreign key (repository, organisation)
        references repositories (id, organisation),
    constraint jobs_changeset foreign key (repository, changeset)
        references changesets (repository, id),
    constraint jobs_lease check ((state = 'running') = (lease_expires_at is not null)),
    constraint jobs_deadline check ((state = 'running') = (deadline is not null)),
    constraint jobs_revert check (revert_landed is null or revert_conflicts is null),
    constraint jobs_queued_has_no_runner check (state <> 'queued' or runner is null)
);

-- The claim takes the oldest queued job.
create index jobs_queued on jobs (id) where state = 'queued';

-- Expiry finds the running jobs whose lease or deadline has passed.
create index jobs_running_expiry on jobs (least(lease_expires_at, deadline)) where state = 'running';

-- The jobs of a changeset, oldest first.
create index jobs_of_changeset on jobs (repository, changeset, id);
