-- One schema for every organisation; each row below the repository directory carries its
-- repository's identifier (ADR 0022).

create table organisations (
    name text primary key
);

create table repositories (
    id bigint generated always as identity primary key,
    organisation text not null references organisations (name),
    name text not null,
    unique (organisation, name)
);

create table changesets (
    repository bigint not null references repositories (id),
    id bytea not null check (length(id) = 32),
    encoding bytea not null,
    primary key (repository, id)
);

create table refs (
    repository bigint not null references repositories (id),
    name text not null,
    target bytea not null check (length(target) = 32),
    primary key (repository, name)
);

create table trees (
    repository bigint not null references repositories (id),
    address bytea not null check (length(address) = 32),
    encoding bytea not null,
    primary key (repository, address)
);

create table tree_roots (
    repository bigint not null references repositories (id),
    changeset bytea not null check (length(changeset) = 32),
    root bytea not null check (length(root) = 32),
    primary key (repository, changeset)
);

create table file_version_nodes (
    repository bigint not null references repositories (id),
    address bytea not null check (length(address) = 32),
    encoding bytea not null,
    primary key (repository, address)
);

-- A row per changeset whose file versions are derived, so that an empty value differs from none.
create table file_version_sets (
    repository bigint not null references repositories (id),
    changeset bytea not null check (length(changeset) = 32),
    primary key (repository, changeset)
);

-- A path is its components joined by '/'; a component holds no '/'.
create table file_version_entries (
    repository bigint not null,
    changeset bytea not null,
    path bytea not null,
    node bytea not null check (length(node) = 32),
    primary key (repository, changeset, path),
    foreign key (repository, changeset)
        references file_version_sets (repository, changeset) on delete cascade
);

create table drafts (
    id bigint generated always as identity primary key,
    repository bigint not null references repositories (id),
    issuer text not null,
    subject text not null,
    name text not null,
    base bytea check (length(base) = 32),
    unique (repository, issuer, subject, name)
);

create table draft_changes (
    draft bigint not null references drafts (id) on delete cascade,
    path bytea not null,
    kind text not null check (kind in ('deletion', 'regular', 'executable')),
    content bytea check (length(content) = 32),
    primary key (draft, path),
    check ((kind = 'deletion') = (content is null))
);

create table members (
    organisation text not null references organisations (name),
    issuer text not null,
    subject text not null,
    role text not null check (role in ('admin', 'member')),
    primary key (organisation, issuer, subject)
);

-- An empty prefix is the whole repository; otherwise components joined by '/'.
create table grants (
    organisation text not null,
    issuer text not null,
    subject text not null,
    repository bigint not null references repositories (id),
    prefix bytea not null,
    access text not null check (access in ('read', 'write')),
    primary key (organisation, issuer, subject, repository, prefix, access),
    constraint grants_member foreign key (organisation, issuer, subject)
        references members (organisation, issuer, subject) on delete cascade
);
