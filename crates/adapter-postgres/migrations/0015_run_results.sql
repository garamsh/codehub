-- A run is held by a runner and ends with a result (ADR 0051). `runner` names a registration
-- without a foreign key, as `jobs.runner` does. A run a runner reported is `finished` and holds
-- the result; a run that was cancelled or timed out holds none. The result is `result_outcome`,
-- the exit code, the two streams (an address and whether it was cut, both null where the command
-- never started), the object counts, the output tree, the changes as the parallel arrays
-- `change_paths`, `change_kinds` and `change_contents` (the address's 32 bytes, empty for a
-- deletion), the refused paths, and the write-back as `write_reason`, null where the changes were
-- written, and `write_paths`.
alter table runs drop constraint runs_state_check;
alter table runs add constraint runs_state_check
    check (state in ('queued', 'running', 'cancelled', 'timed_out', 'finished'));

alter table runs
    add column runner bigint,
    add column result_outcome text,
    add column exit_code integer,
    add column stdout_address bytea,
    add column stdout_cut boolean,
    add column stderr_address bytea,
    add column stderr_cut boolean,
    add column objects_downloaded bigint,
    add column objects_from_cache bigint,
    add column output_tree bytea,
    add column change_paths bytea[],
    add column change_kinds text[],
    add column change_contents bytea[],
    add column refused text[],
    add column write_reason text,
    add column write_paths bytea[],
    add constraint runs_queued_has_no_runner check (state <> 'queued' or runner is null),
    add constraint runs_finished_has_result check (
        (state = 'finished') = (result_outcome is not null)
    ),
    add constraint runs_change_arrays check (
        cardinality(change_paths) = cardinality(change_kinds)
        and cardinality(change_paths) = cardinality(change_contents)
    );
