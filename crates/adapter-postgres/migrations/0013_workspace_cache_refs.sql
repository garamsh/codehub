-- A workspace cache ref maps a repository, a workspace instance and a cache path to the tree the
-- latest run of that instance left there (ADR 0051). The instance is the id of the draft row that
-- opened it; it has no foreign key, because the row of a closed instance is gone and its refs are
-- removed by the application. The cache path is `/`-joined components from the repository root;
-- the tree is the 32 bytes of an address.
create table workspace_cache_refs (
    repository bigint not null,
    instance bigint not null,
    cache_path bytea not null,
    tree bytea not null check (length(tree) = 32),
    updated_at timestamptz not null default now(),
    primary key (repository, instance, cache_path),
    constraint workspace_cache_refs_repository foreign key (repository) references repositories (id)
);
