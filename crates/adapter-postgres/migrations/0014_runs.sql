-- A run is a command started in a workspace (ADR 0051). Its id comes from the sequence of
-- `jobs.id`, so that it is the id its job is claimed by. The workspace is named by its principal,
-- name and instance, with no foreign key, because the row of a closed instance is gone. Paths are
-- `/`-joined components from the repository root, the empty value for the whole repository. The
-- files sent are the parallel arrays `file_paths`, `file_kinds` and `file_contents`, and the
-- cache refs' trees at the start are `cache_paths` and `cache_trees`; trees and contents are the
-- 32 bytes of an address.
create table runs (
    id bigint primary key default nextval(pg_get_serial_sequence('jobs', 'id')),
    organisation text not null,
    repository bigint not null,
    issuer text not null,
    subject text not null,
    draft text not null,
    instance bigint not null,
    version bigint not null,
    base bytea check (base is null or length(base) = 32),
    command text not null,
    prefix bytea not null,
    add_paths bytea[] not null,
    outputs bytea[] not null,
    labels text[] not null,
    timeout_seconds bigint not null check (timeout_seconds > 0),
    cache_paths bytea[] not null,
    cache_trees bytea[] not null,
    file_paths bytea[] not null,
    file_kinds text[] not null,
    file_contents bytea[] not null,
    state text not null check (state in ('queued', 'running', 'cancelled')),
    lease_expires_at bigint,
    deadline bigint,
    created_at timestamptz not null default now(),
    updated_at timestamptz not null default now(),
    constraint runs_cache_arrays check (cardinality(cache_paths) = cardinality(cache_trees)),
    constraint runs_file_arrays check (
        cardinality(file_paths) = cardinality(file_kinds)
        and cardinality(file_paths) = cardinality(file_contents)
    ),
    constraint runs_repository foreign key (repository) references repositories (id)
);

create index runs_workspace on runs (repository, issuer, subject, draft, instance, id);
