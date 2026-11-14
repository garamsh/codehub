-- A check is declared in a folder and may name inputs, needs, outputs and caches (ADR 0049). A job
-- keeps them with the check. `folder` is the components of the folder joined by `/`, empty for the
-- root. `inputs` and `needs` are the text a check file gives, `outputs` and `caches` are paths
-- joined by `/`. A job queued before these columns existed was a check of the root that read its
-- whole folder.
alter table jobs
    add column folder bytea not null default ''::bytea,
    add column inputs text[] not null default array['**']::text[],
    add column needs text[] not null default array[]::text[],
    add column outputs bytea[] not null default array[]::bytea[],
    add column caches bytea[] not null default array[]::bytea[];
