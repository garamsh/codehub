# Quickstart

Starting the check services, running `codehub` against them, and landing and reading a file with `ch`, from a clean checkout.

The output below was produced on 2026-11-11 from the commit `e4fe6227eb08e05d515d77ac625515cc6054340b` plus this file's change, with the services of `compose.yaml` running. Steps 5 and 6 changed afterwards only in the path of the scratch file, and were run again on 2026-11-12 from the commit `33ec12de6559b810aaf1a93494716df864dc62e0` plus that change: their output lines are the same apart from the changeset identifiers. Changeset identifiers contain a timestamp, so yours differ; every other line is the same.

## Before you start

- `docker` with the compose plugin, `curl`, `jq`, `openssl` and a Rust toolchain (`rust-toolchain.toml` pins it).
- Free host ports 15432 (PostgreSQL), 19000 (object store), 18080 (identity provider), 18081 (Keycloak, federated section), 18082 (Cerbos, federated section) and 8080 (`codehub`).
- All commands run from the repository root.

## 1. Start the services

```sh
make services
```

This runs `docker compose up --detach --wait`, creates the object-store bucket `CODEHUB_TEST_S3_BUCKET` names (`scripts/services-up.sh`), and smoke-tests the three services (`scripts/check-services.sh`). Without `make`, run `docker compose up --detach --wait` and create the bucket yourself with the signed `PUT` that `scripts/services-up.sh` sends.

Create a database for the server; the services start with none of its own:

```sh
docker compose exec postgres createdb -U codehub quickstart
```

## 2. Run `codehub`

```sh
cargo build --locked -p codehub -p ch
set -a; . dev/check-env.sh; set +a
export CODEHUB_POSTGRES_URL="${CODEHUB_TEST_POSTGRES_URL%/*}/quickstart"
export CODEHUB_S3_ENDPOINT=$CODEHUB_TEST_S3_ENDPOINT
export CODEHUB_S3_REGION=$CODEHUB_TEST_S3_REGION
export CODEHUB_S3_BUCKET=$CODEHUB_TEST_S3_BUCKET
export CODEHUB_S3_ACCESS_KEY=$CODEHUB_TEST_S3_ACCESS_KEY
export CODEHUB_S3_SECRET_KEY=$CODEHUB_TEST_S3_SECRET_KEY
export CODEHUB_OIDC_ISSUER=$CODEHUB_TEST_OIDC_ISSUER
export CODEHUB_OIDC_AUDIENCE=ch
target/debug/codehub
```

The audience is `ch` because the identity provider issues the device grant's access token to the client `ch`; the token's `aud` claim is `ch`. The server runs in this terminal until you stop it with Ctrl-C. Its first line of output is:

```text
codehub listening on 127.0.0.1:8080
```

`codehub --help` lists every setting; each is a flag and a `CODEHUB_*` variable.

## 3. Log in

In a second terminal, from the repository root:

```sh
export PATH=$PWD/target/debug:$PATH
ch login http://127.0.0.1:8080 --issuer http://127.0.0.1:18080/dex
```

`ch` prints a code on stderr and waits:

```text
To log in, open http://127.0.0.1:18080/dex/device and enter the code RLVT-BLNF
Or open http://127.0.0.1:18080/dex/device?user_code=RLVT-BLNF
```

Open the address in a browser, enter the code, and log in as `alice@example.test` with the password `CODEHUB_TEST_OIDC_PASSWORD_ALICE` holds in `dev/check-env.sh` (the account is in `dev/dex/config.yaml`). The command then prints the subject and issuer of the principal, and exits 0:

```text
logged in as CiQwMDAwMDAwMC0wMDAwLTQwMDAtODAwMC0wMDAwMDAwMGExMWMSBWxvY2Fs at http://127.0.0.1:18080/dex
```

`ch` keeps the server, the token and the selected repository in `$XDG_CONFIG_HOME/ch/state.json` (`~/.config/ch/state.json` where the variable is unset), readable by you only.

## 4. Create an organisation and a repository, and select it

```console
$ ch org create acme
created organisation acme
$ ch repo create acme/app
created repository acme/app
$ ch use acme/app
```

`ch use` prints nothing.

## 5. Stage and land a file

```console
$ tmp=$(mktemp -d)
$ echo 'hello from codehub' > "$tmp/hello.txt"
$ ch put docs/hello.txt "$tmp/hello.txt"
$ ch land -m first
sha256:db8a02d35b9d85376a1ec639a8b16832debe88b4baf128c66825d2a66fcb9375
```

The file is written under a temporary directory, so the repository is left as it was. `ch put` prints nothing; it stages the file in the draft `default`. `ch land` prints the landed changeset's identifier.

## 6. Read the history and the content

```console
$ ch log docs/hello.txt
sha256:db8a02d35b9d85376a1ec639a8b16832debe88b4baf128c66825d2a66fcb9375 sha256:ca8c8fd1dca71b3bbfb9d42472525ebe830054c35ca192a2db6d200360bf6a54
$ ch read docs/hello.txt
hello from codehub
```

Each `log` line is a changeset and the address of the file's content in it, newest first. Land a second version to see the chain:

```console
$ echo 'hello again from codehub' > "$tmp/hello.txt"
$ ch put docs/hello.txt "$tmp/hello.txt"
$ ch land -m second
sha256:a2c2e7d07690c05c7fbc8f3cdeac500f3772fed22b0516eed6d012ddc3ac9617
$ ch log docs/hello.txt
sha256:a2c2e7d07690c05c7fbc8f3cdeac500f3772fed22b0516eed6d012ddc3ac9617 sha256:14fc3169e3290826388d13a6b1a3a3e39f517d1cdfb7a8c4d79648c2a1f7cd15
sha256:db8a02d35b9d85376a1ec639a8b16832debe88b4baf128c66825d2a66fcb9375 sha256:ca8c8fd1dca71b3bbfb9d42472525ebe830054c35ca192a2db6d200360bf6a54
$ ch read docs/hello.txt
hello again from codehub
```

## Stop and clean up

Stop `codehub` with Ctrl-C. The services keep running; stop them and delete their data with `make services-down`. To start over against the same services, drop the database with `docker compose exec postgres dropdb -U codehub quickstart` and create it again.

## Federated mode

The same `ch` and `codehub`, with an external identity provider and an external decision point instead of the built-in ones: `codehub` verifies each token by introspection (RFC 7662) against Keycloak, and asks Cerbos, over AuthZEN 1.0, whether each request is allowed. Members and organisations are then administered outside `codehub`: `ch org create` and `ch member add` are not used.

The output below was produced on 2026-11-12 from the commit `130a8d9fe84a00af29c69d6b0c8d84f71fa3a2da` plus this file's change, with the services of `compose.yaml` running and port 8080 free. Changeset identifiers contain a timestamp, so yours differ; every other line is the same.

### Run `codehub` federated

Stop the standalone `codehub` first if it still runs, then create a separate database and start the server from the repository root:

```sh
docker compose exec postgres createdb -U codehub federated
cargo build --locked -p codehub -p ch
set -a; . dev/check-env.sh; set +a
export CODEHUB_POSTGRES_URL="${CODEHUB_TEST_POSTGRES_URL%/*}/federated"
export CODEHUB_S3_ENDPOINT=$CODEHUB_TEST_S3_ENDPOINT
export CODEHUB_S3_REGION=$CODEHUB_TEST_S3_REGION
export CODEHUB_S3_BUCKET=$CODEHUB_TEST_S3_BUCKET
export CODEHUB_S3_ACCESS_KEY=$CODEHUB_TEST_S3_ACCESS_KEY
export CODEHUB_S3_SECRET_KEY=$CODEHUB_TEST_S3_SECRET_KEY
export CODEHUB_IDENTITY=introspection
export CODEHUB_INTROSPECTION_ENDPOINT=$CODEHUB_TEST_KEYCLOAK_ISSUER/protocol/openid-connect/token/introspect
export CODEHUB_INTROSPECTION_CLIENT_ID=$CODEHUB_TEST_KEYCLOAK_INTROSPECTION_CLIENT_ID
export CODEHUB_INTROSPECTION_CLIENT_SECRET=$CODEHUB_TEST_KEYCLOAK_INTROSPECTION_CLIENT_SECRET
export CODEHUB_INTROSPECTION_ISSUER=$CODEHUB_TEST_KEYCLOAK_ISSUER
export CODEHUB_INTROSPECTION_AUDIENCE=$CODEHUB_TEST_KEYCLOAK_AUDIENCE
export CODEHUB_AUTHORISATION=authzen
export CODEHUB_AUTHZEN_URL=$CODEHUB_TEST_AUTHZEN_URL
export CODEHUB_AUTHZEN_TOKEN_ENDPOINT=$CODEHUB_TEST_KEYCLOAK_ISSUER/protocol/openid-connect/token
export CODEHUB_AUTHZEN_CLIENT_ID=$CODEHUB_TEST_KEYCLOAK_AUTHZEN_CLIENT_ID
export CODEHUB_AUTHZEN_CLIENT_SECRET=$CODEHUB_TEST_KEYCLOAK_AUTHZEN_CLIENT_SECRET
target/debug/codehub
```

The first line of output is the same as in the standalone run:

```text
codehub listening on 127.0.0.1:8080
```

The audience is `codehub`: Keycloak puts it in the `aud` claim of the tokens of the client `codehub-test`, and introspection accepts only tokens issued for it. The `codehub-authzen` client is the one `codehub` authenticates as to reach Cerbos.

### Use `ch` with a token file

In a second terminal, from the repository root. A separate configuration directory keeps this run's `ch` state apart from the standalone one. The token is obtained as alice with the password grant of the client `codehub-test` and kept in a file; `ch` reads the file on every command, so it never logs in:

```sh
set -a; . dev/check-env.sh; set +a
export PATH=$PWD/target/debug:$PATH
export XDG_CONFIG_HOME=$(mktemp -d)
export CH_SERVER=http://127.0.0.1:8080
export CH_TOKEN_FILE=$(mktemp)
curl -s -u "$CODEHUB_TEST_KEYCLOAK_TEST_CLIENT_ID:$CODEHUB_TEST_KEYCLOAK_TEST_CLIENT_SECRET" \
  -d grant_type=password -d username=alice \
  --data-urlencode "password=$CODEHUB_TEST_KEYCLOAK_PASSWORD_ALICE" \
  "$CODEHUB_TEST_KEYCLOAK_ISSUER/protocol/openid-connect/token" | jq -r .access_token > "$CH_TOKEN_FILE"
```

The token expires after a few minutes. When `ch` exits 3 with `ch: the token was rejected`, run the `curl` again.

There is no `ch org create`: the policy lets alice create repositories and, in `acme`, the organisation is created on first use.

```console
$ ch repo create acme/app
created repository acme/app
$ ch use acme/app
$ tmp=$(mktemp -d)
$ echo 'hello from codehub' > "$tmp/hello.txt"
$ ch put docs/hello.txt "$tmp/hello.txt"
$ ch land -m first
sha256:91fef0218463c7314bef33a2008d17316c550a305652c7f204ad84153c5351b9
$ ch read docs/hello.txt
hello from codehub
$ ch log docs/hello.txt
sha256:91fef0218463c7314bef33a2008d17316c550a305652c7f204ad84153c5351b9 sha256:ca8c8fd1dca71b3bbfb9d42472525ebe830054c35ca192a2db6d200360bf6a54
```

A token issued to another client, here `codehub-test-upstream`, is for another audience and is rejected:

```console
$ upstream=$(mktemp)
$ curl -s -u "$CODEHUB_TEST_KEYCLOAK_UPSTREAM_CLIENT_ID:$CODEHUB_TEST_KEYCLOAK_UPSTREAM_CLIENT_SECRET" \
    -d grant_type=password -d username=alice \
    --data-urlencode "password=$CODEHUB_TEST_KEYCLOAK_PASSWORD_ALICE" \
    "$CODEHUB_TEST_KEYCLOAK_ISSUER/protocol/openid-connect/token" | jq -r .access_token > "$upstream"
$ ch --token-file "$upstream" read docs/hello.txt; echo "exit $?"
ch: the token was rejected
exit 3
```

### Exchange a token (RFC 8693)

`ch` can exchange that token for one the server accepts, with the client `codehub-exchange`. This run needs `codehub` verifying the exchanged token locally, because Keycloak issues it for the audience `codehub` without naming the introspection client; stop the server above and start it again with the identity settings below in place of the `CODEHUB_IDENTITY` and `CODEHUB_INTROSPECTION_*` exports (the database and the authorisation settings stay):

```sh
unset CODEHUB_IDENTITY CODEHUB_INTROSPECTION_ENDPOINT CODEHUB_INTROSPECTION_CLIENT_ID CODEHUB_INTROSPECTION_CLIENT_SECRET CODEHUB_INTROSPECTION_ISSUER CODEHUB_INTROSPECTION_AUDIENCE
export CODEHUB_OIDC_ISSUER=$CODEHUB_TEST_KEYCLOAK_ISSUER
export CODEHUB_OIDC_AUDIENCE=$CODEHUB_TEST_KEYCLOAK_AUDIENCE
target/debug/codehub
```

Then, in the second terminal, with the token of `codehub-test-upstream` in `$upstream` and `ch use acme/app` run for this `XDG_CONFIG_HOME`:

```console
$ ch --token-file "$upstream" read docs/hello.txt; echo "exit $?"
ch: the token was rejected
exit 3
$ ch --token-file "$upstream" \
    --exchange-endpoint "$CODEHUB_TEST_KEYCLOAK_ISSUER/protocol/openid-connect/token" \
    --exchange-audience codehub \
    --exchange-client-id "$CODEHUB_TEST_KEYCLOAK_EXCHANGE_CLIENT_ID" \
    --exchange-client-secret "$CODEHUB_TEST_KEYCLOAK_EXCHANGE_CLIENT_SECRET" \
    read docs/hello.txt; echo "exit $?"
hello from codehub
exit 0
```

### Stop and clean up

Stop `codehub` with Ctrl-C. To start over, drop the database with `docker compose exec postgres dropdb -U codehub federated` and create it again.
