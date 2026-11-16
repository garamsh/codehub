-- A grant may allow `image_pull` or `image_push` (ADR 0053).
alter table grants drop constraint grants_access_check;
alter table grants
    add constraint grants_access_check
    check (access in ('read', 'write', 'run', 'image_pull', 'image_push'));
