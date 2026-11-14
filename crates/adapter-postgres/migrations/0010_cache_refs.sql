-- A cache ref maps a repository, a folder and a cache path to the tree a job left there, and to
-- the landed changeset of that job, which a later job must be after to replace it (ADR 0049). The
-- folder and the cache path are `/`-joined components, the folder empty for the whole repository;
-- the tree and the changeset are the 32 bytes of an address.
create table cache_refs (
    repository bigint not null,
    folder bytea not null,
    cache_path bytea not null,
    tree bytea not null check (length(tree) = 32),
    changeset bytea not null check (length(changeset) = 32),
    updated_at timestamptz not null default now(),
    primary key (repository, folder, cache_path),
    constraint cache_refs_repository foreign key (repository) references repositories (id)
);
