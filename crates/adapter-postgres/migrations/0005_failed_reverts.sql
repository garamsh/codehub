-- A revert that failed after its reservation is recorded with the job as the text of the failure
-- (ADR 0035). A revert is recorded as the changeset it landed as, the paths it conflicted at, or
-- why it failed, never more than one.
alter table jobs add column revert_failed text;

alter table jobs drop constraint jobs_revert;
alter table jobs add constraint jobs_revert
    check (num_nonnulls(revert_landed, revert_conflicts, revert_failed) <= 1);
