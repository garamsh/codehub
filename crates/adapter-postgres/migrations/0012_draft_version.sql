-- A draft has a version: 0 when it opens, and one more each time its changes are recorded
-- (ADR 0051). Its row's identity is its instance, which landing closes by deleting the row.
alter table drafts add column version bigint not null default 0 check (version >= 0);
