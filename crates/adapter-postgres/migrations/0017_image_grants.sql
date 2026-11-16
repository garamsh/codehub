-- A grant may allow `image_pull` or `image_push` (ADR 0053). The constraint is added without
-- scanning the table under its lock and validated after, which takes a lock that lets reads and
-- writes continue.
alter table grants drop constraint grants_access_check;
alter table grants
    add constraint grants_access_check
    check (access in ('read', 'write', 'run', 'image_pull', 'image_push')) not valid;
alter table grants validate constraint grants_access_check;
