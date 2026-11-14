-- A check of a landed changeset waits until the checks it needs have passed, and is then queued, or
-- passed from the result cache, or skipped (ADR 0049). `check_key` is what the result depends on
-- and `output_tree` the tree the run recorded; both are the 32 bytes of an address, null until the
-- job has them. A job queued before this migration has no key, so it is never answered from the
-- cache.
alter table jobs
    add column check_key bytea check (length(check_key) = 32),
    add column output_tree bytea check (length(output_tree) = 32);

alter table jobs drop constraint jobs_state_check;
alter table jobs add constraint jobs_state_check check (
    state in ('waiting', 'queued', 'running', 'passed', 'failed', 'timed_out', 'cancelled',
              'skipped', 'cached'));

-- The result cache asks whether a key passed in a repository.
create index jobs_passed_key on jobs (repository, check_key) where state = 'passed';
