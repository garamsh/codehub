-- A grant may allow `run` (ADR 0051).
alter table grants drop constraint grants_access_check;
alter table grants
    add constraint grants_access_check check (access in ('read', 'write', 'run'));
