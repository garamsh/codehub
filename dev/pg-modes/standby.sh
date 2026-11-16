#!/bin/sh
# Starts a hot standby of pg-primary: the first start copies the primary's data directory and the
# configuration that makes it follow, every later start only runs the server.
set -eu
if [ ! -s "$PGDATA/PG_VERSION" ]; then
  mkdir -p "$PGDATA"
  chmod 0700 "$PGDATA"
  PGPASSWORD=replicator pg_basebackup --host pg-primary --username replicator \
    --pgdata "$PGDATA" --write-recovery-conf --wal-method stream --no-password
fi
exec postgres
