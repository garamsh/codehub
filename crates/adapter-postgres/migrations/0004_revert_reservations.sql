-- The revert of a changeset is reserved for one of its jobs before it lands (ADR 0035). The
-- primary key admits one reservation per changeset, and a reservation is never removed.
create table revert_reservations (
    repository bigint not null,
    changeset bytea not null,
    job bigint not null references jobs (id),
    primary key (repository, changeset),
    constraint revert_reservations_changeset foreign key (repository, changeset)
        references changesets (repository, id)
);

-- A revert recorded before this table existed holds its reservation.
insert into revert_reservations (repository, changeset, job)
select distinct on (repository, changeset) repository, changeset, id
from jobs
where revert_landed is not null or revert_conflicts is not null
order by repository, changeset, id;
