# Quickstart

Starting the check services, running `codehub` against them, and landing and reading a file with `ch`, from a clean checkout.

The output below was produced on 2026-11-11 from the commit `e4fe6227eb08e05d515d77ac625515cc6054340b` plus this file's change, with the services of `compose.yaml` running. Steps 5 and 6 changed afterwards only in the path of the scratch file, and were run again on 2026-11-12 from the commit `33ec12de6559b810aaf1a93494716df864dc62e0` plus that change: their output lines are the same apart from the changeset identifiers. Changeset identifiers contain a timestamp, so yours differ; every other line is the same.

## Before you start

- `docker` with the compose plugin, `curl`, `jq`, `openssl` and a Rust toolchain (`rust-toolchain.toml` pins it).
- Free host ports 15432 (PostgreSQL), 19000 (object store), 18080 (identity provider) and 8080 (`codehub`).
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
