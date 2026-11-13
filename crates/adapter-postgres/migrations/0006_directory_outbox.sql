-- The directory outbox (ADR 0044): each repository change is written as an entry in the
-- transaction that makes it, and deleted once the receiver accepted it.
--
-- A repository's change counter rises with every change to it and starts at 1 with its creation.
-- Every repository that exists now has been created once, so its counter is 1.
alter table repositories add column change_sequence bigint not null default 1;

-- No foreign key to repositories: a deletion's entry must outlive the row it reports.
-- An entry is claimable where none of its repository's entries has a lease_expires_at after the
-- claimer's now; claiming sets it, and an entry never claimed has none.
create table directory_outbox (
    id bigint generated always as identity primary key,
    repository bigint not null,
    organisation text not null,
    name text not null,
    kind text not null check (kind in ('created', 'deleted')),
    sequence bigint not null check (sequence >= 1),
    lease_expires_at bigint,
    unique (repository, sequence)
);

insert into directory_outbox (repository, organisation, name, kind, sequence)
select id, organisation, name, 'created', 1 from repositories order by id;
