-- A grant's repository belongs to the grant's organisation.

alter table repositories
    add constraint repositories_id_organisation unique (id, organisation);

alter table grants
    add constraint grants_repository_organisation foreign key (repository, organisation)
        references repositories (id, organisation);
