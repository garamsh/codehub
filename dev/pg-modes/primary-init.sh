#!/bin/sh
# Runs once when the primary's data directory is made: the role a standby replicates with, the
# rule that lets it connect from the compose network, and the synchronous replication that makes a
# commit wait for the standby. The last is set here, not on the command line, because the temporary
# server of this initialisation would then wait for a standby that does not exist yet.
set -eu
psql -v ON_ERROR_STOP=1 --username "$POSTGRES_USER" --dbname postgres \
  -c "create role replicator with replication login password 'replicator'" \
  -c "alter system set synchronous_standby_names = '*'"
echo "host replication replicator all scram-sha-256" >> "$PGDATA/pg_hba.conf"
