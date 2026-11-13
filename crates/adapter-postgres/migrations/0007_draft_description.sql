-- A draft is a workspace with a description, which a landing without a message takes as its message
-- (ADR 0048). A draft staged before the description existed has an empty one.
alter table drafts add column description text not null default '';
