# Quickstart

Starting the check services, running `codehub` against them, and changing, landing and reading a file with `ch`, from a clean checkout.

## Contents
- Before you start
- 1. Start the services
- 2. Run `codehub`
- 3. Log in
- 4. Create an organisation and a repository, and select it
- 5. Change the repository in a view
- 6. Read the history and the content
- 7. Change a file without a view
- Stop and clean up
- Federated mode
- garam as issuer and decision point

The output of steps 1 to 4 was produced on 2026-11-11 from the commit `e4fe6227eb08e05d515d77ac625515cc6054340b` plus this file's change, with the services of `compose.yaml` running. The output of steps 5 to 7 was produced on 2026-11-14 from the commit `a1decc3fe4f9c1c421e6c20c5fe949ba3f1d994c` plus this file's change, with the check stack running and `codehub` listening on port 18090; its path lines differ from yours in the view's directory. Changeset identifiers contain a timestamp, so yours differ; every other line is the same.

## Before you start

- `docker` with the compose plugin, `curl`, `jq`, `openssl` and a Rust toolchain (`rust-toolchain.toml` pins it).
- Free host ports 15432 (PostgreSQL), 19000 (object store), 18080 (identity provider), 18081 (Keycloak, federated section), 18082 (Cerbos, federated section) and 8080 (`codehub`).
- All commands run from the repository root.

## 1. Start the services

```sh
make services
```

This runs `docker compose up --detach --wait`, creates the object-store bucket `CODEHUB_TEST_S3_BUCKET` names (`scripts/services-up.sh`), and smoke-tests the five services (`scripts/check-services.sh`). Without `make`, run `docker compose up --detach --wait` and create the bucket yourself with the signed `PUT` that `scripts/services-up.sh` sends.

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

## 5. Change the repository in a view

A view is a directory that holds the repository's files and the server-side workspace they belong to (ADR 0048). Open one, edit it with ordinary tools, and land it. The repository is empty, so the view starts empty:

```console
$ ch open work
$ cd work
$ mkdir docs
$ echo 'hello from codehub' > docs/hello.txt
$ ch status
added docs/hello.txt
$ ch describe add the greeting
$ ch land
sha256:7e9655a6f8f134d7b1475513155629f081296fba558f77de4702e9c487fad3a1
```

`ch open` and `ch describe` print nothing. `ch status` snapshots the view first, so it lists what the workspace now holds against its base. `ch land` lands the workspace with its description as the message and prints the landed changeset's identifier. After it, the view is closed and a later command in it is refused.

Open a second view to change the file. `ch diff` shows the server's diff, and `ch land -m` lands with a message in place of the description:

```console
$ cd ..
$ ch open second
$ cd second
$ echo 'hello again from codehub' > docs/hello.txt
$ ch diff docs/hello.txt
modified docs/hello.txt
--- a/docs/hello.txt
+++ b/docs/hello.txt
@@ -1,1 +1,1 @@
-hello from codehub
+hello again from codehub
$ ch land -m greet again
sha256:5dd4239a37630f32050a9903ecd9da567c4a07cce16e839c547b689405c786e1
$ ch status
ch: this view is closed, its workspace landed; run `ch open <directory>` to open a new one
```

The last command exits 2. `ch restore <path>` returns a path to the base's content, and `ch open <directory> --prefix <path>` holds only the files under a path.

## 6. Read the history and the content

```console
$ cd ..
$ ch log docs/hello.txt
sha256:5dd4239a37630f32050a9903ecd9da567c4a07cce16e839c547b689405c786e1 sha256:14fc3169e3290826388d13a6b1a3a3e39f517d1cdfb7a8c4d79648c2a1f7cd15 greet again
sha256:7e9655a6f8f134d7b1475513155629f081296fba558f77de4702e9c487fad3a1 sha256:ca8c8fd1dca71b3bbfb9d42472525ebe830054c35ca192a2db6d200360bf6a54 add the greeting
$ ch read docs/hello.txt
hello again from codehub
```

Each `log` line is a changeset, the address of the file's content in it, and the first line of the message it landed with, newest first. `ch log --json` prints the whole message.

## 7. Change a file without a view

A caller with no directory to work in stages one file at a time with `ch put`, then lands:

```console
$ echo 'put without a view' > seed.txt
$ ch put docs/put.txt seed.txt
$ ch land -m put a file
sha256:9f0ae20d00da6d470dba304932c349e3277c50654f38b7855aedbbcfa983ae13
```

`ch put` prints nothing; it stages the file in the draft `default`, which `ch land` outside a view lands and which needs `--message`. `ch put --delete docs/hello.txt` stages a deletion. Inside a view, `ch put` is refused, because it would change the workspace behind the view's back:

```console
$ cd second
$ ch put docs/hello.txt ../seed.txt
ch: put is for a caller without a view, and this is inside the view at /tmp/tmp.zuwSbGG02n/second; edit its files and run `ch status`, `ch describe` and `ch land`
```

The command exits 2.

## Stop and clean up

Stop `codehub` with Ctrl-C. The services keep running; stop them and delete their data with `make services-down`. To start over against the same services, drop the database with `docker compose exec postgres dropdb -U codehub quickstart` and create it again.

## Federated mode

The same `ch` and `codehub`, with an external identity provider and an external decision point instead of the built-in ones: `codehub` verifies each token by introspection (RFC 7662) against Keycloak, and asks Cerbos, over AuthZEN 1.0, whether each request is allowed. Members and organisations are then administered outside `codehub`: `ch org create` and `ch member add` are not used.

The output below was produced on 2026-11-12 from the commit `ccd033a216e799dd232531a55f496b344e38a3f4` plus this file's change, with the services of `compose.yaml` running and port 8080 free. Changeset identifiers contain a timestamp, so yours differ; every other line is the same.

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

### Run checks on a runner

The output below was produced on 2026-11-13 from the commit `76660678f22722c791083986a3b82208c5561ff8` plus this file's change; changeset identifiers contain a timestamp, so yours differ, and every other line is the same. Cerbos reads `dev/cerbos/policies` when it starts, so a `make services` started before the policy granted `administer_runners` answers `ch: the action is not allowed` to step 1 until the services are started again.

A runner is a principal registered to claim the jobs of an organisation's checks; `codehub-runner` claims them and runs each command on this machine. This section runs in federated mode, with the second terminal's `ch` configured as above and `acme/app` selected, against `codehub` started with either of the identity settings above. The runner authenticates with a token file, the same way `ch` does: bob's token from the password grant of the client `codehub-test`, because that client and bob's password are already in `dev/check-env.sh` and compose, so no client has to be created in Keycloak to get one. The other way, a token endpoint with the client-credentials grant (`--token-endpoint`, `--client-id`, `--client-secret-env`), needs a Keycloak client with a service account, which `dev/keycloak/codehub-realm.json` does not have.

1. Register bob as a runner of `acme`. Alice is allowed to in `acme` (`administer_runners` in `dev/cerbos/policies/organisation.yaml`). Bob's subject and the issuer are in `dev/check-env.sh`:

   ```console
   $ ch runner register "$CODEHUB_TEST_KEYCLOAK_SUBJECT_BOB" --issuer "$CODEHUB_TEST_KEYCLOAK_ISSUER"
   1
   $ ch runner list
   1 organisation http://127.0.0.1:18081/realms/codehub 00000000-0000-4000-8000-0000000000b0 -
   ```

2. Start `codehub-runner` in a third terminal, from the repository root, with bob's token in a file. It stays in the foreground and prints a line when a job starts and when it ends, for example `job 1 ended: Passed`. The token expires after a few minutes and the runner reads the file again before each request, so write the token again with the same `curl` when `ch checks` stops making progress:

   ```sh
   cargo build --locked -p codehub-runner
   set -a; . dev/check-env.sh; set +a
   export RUNNER_TOKEN_FILE=$(mktemp)
   curl -s -u "$CODEHUB_TEST_KEYCLOAK_TEST_CLIENT_ID:$CODEHUB_TEST_KEYCLOAK_TEST_CLIENT_SECRET" \
     -d grant_type=password -d username=bob \
     --data-urlencode "password=$CODEHUB_TEST_KEYCLOAK_PASSWORD_BOB" \
     "$CODEHUB_TEST_KEYCLOAK_ISSUER/protocol/openid-connect/token" | jq -r .access_token > "$RUNNER_TOKEN_FILE"
   target/debug/codehub-runner --server http://127.0.0.1:8080 --token-file "$RUNNER_TOKEN_FILE"
   ```

3. In the second terminal, land a change with a `.codehub/checks.toml`. Each `[[check]]` is a job; a check blocks unless it says `blocking = false`:

   ```console
   $ cat > "$tmp/checks.toml" <<'EOF'
   [[check]]
   name = "unit"
   command = "echo unit ok"

   [[check]]
   name = "lint"
   command = "test -f docs/hello.txt"
   EOF
   $ ch put .codehub/checks.toml "$tmp/checks.toml"
   $ ch land -m 'add checks'
   sha256:3e1aed923ae11adc21cd3f6dc0a244b36002e73dfcf7f1944883da650acd1670
   ```

4. Wait for the checks of the landing:

   ```console
   $ ch checks --wait; echo "exit $?"
   last-green sha256:3e1aed923ae11adc21cd3f6dc0a244b36002e73dfcf7f1944883da650acd1670
   unit blocking passed 0
   lint blocking passed 0
   exit 0
   ```

5. `last-green` is the newest changeset whose blocking checks all passed. The first line above shows it at the changeset `ch land` printed; read it again with:

   ```console
   $ ch checks
   last-green sha256:3e1aed923ae11adc21cd3f6dc0a244b36002e73dfcf7f1944883da650acd1670
   unit blocking passed 0
   lint blocking passed 0
   ```

A blocking check that fails or times out has its changeset reverted as a new changeset; `ch checks` prints the revert next to the check, and `ch checks --wait` exits 4.

### Run the gaon driver against a real gaon

`codehub-runner --executor gaon` runs each job in a run-once MicroVM on gaon (ADR 0046). Its stub tests answer from a stand-in, so `make e2e-gaon` runs the driver against the real server: it fetches `garamsh/gaon` at the commit pinned in `dev/gaon/env.sh`, builds `gaon-apiserver` in a Go container, starts it with etcd under RBAC and ServiceAccount tokens, binds a ServiceAccount to the ClusterRole in `dev/gaon/runner-role.json`, runs `crates/codehub-runner/tests/gaon_live.rs` under that ServiceAccount's token, and removes the containers, the process and the temporary directory at every exit. It needs Docker, network access to GitHub and the two public images pinned in the same file, and no gaon checkout or credentials. The first run builds the server into `target/codehub-gaon-e2e/`; later runs reuse it. The output below was produced on 2026-11-13 from the commit `5f07a633af28f24acb1f229d83fbafdd69b70181` plus this change, after the first run had built the server:

```console
$ make e2e-gaon
sh dev/gaon/run.sh
e2e-gaon: gaon-apiserver built from garamsh/gaon ad26e9b7ffc9ab2446cb8c3ac4489aefec7b0438, etcd quay.io/coreos/etcd@sha256:a9c7cc12156f4b26ca41f0b57ba5b76c45037ff6c1e47ea3d68cb7908d48a965

running 4 tests
test the_service_account_is_refused_objects_without_the_runner_label ... ok
test start_deletes_the_leftovers_of_its_label_and_no_other_runner_s ... ok
test a_timeout_deletes_the_microvm_and_the_secret_and_a_labelled_list_shows_them_gone ... ok
test a_succeeded_microvm_ends_the_job_with_the_result_posted_to_the_listener ... ok

test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 6.12s

test: 4 tests ran (-p codehub-runner --test gaon_live -- --ignored)
```

gaon has no scheduler or node agent here, so the tests bind a MicroVM and write its phase as the administrator, and post the result a MicroVM would post. The order of the driver's two deletes is not visible through the API, and only the stub tests check it. The `make ci` run skips these tests, which are ignored.

### Stop and clean up

Stop `codehub` with Ctrl-C, and the runner. To start over, drop the database with `docker compose exec postgres dropdb -U codehub federated` and create it again.

## garam as issuer and decision point

The same `codehub` with garam as the OIDC issuer and the AuthZEN decision point, in the `grn` profile with `private_key_jwt`, and with garam told about each repository through the directory outbox. `docs/deployment.md` lists every setting; this section names the ones garam mode needs and runs the end-to-end test that shows them working. The decisions behind them are ADR 0042, 0043 and 0044 (`docs/architecture/`).

The output below was produced on 2026-11-13 from the commit `9d375124b4defdc790006f837013b20b9459ea31` plus this file's change, with garam 0.8.0's images in the local Docker image store. `make e2e-garam` starts the services of `compose.yaml` itself.

### Settings

Set these on `codehub`, besides its database and object-store settings. The middle column is the value the end-to-end test gives (`e2e/tests/common/garam.rs`, with the addresses of `dev/garam/env.sh`); the right column is what an operator supplies.

| Variable | End-to-end test | Operator |
| --- | --- | --- |
| `CODEHUB_OIDC_ISSUER` | `https://localhost:28081/api` | garam's issuer, `https://<host>/api`: garam requires the path `/api` |
| `CODEHUB_OIDC_AUDIENCE` | `codehub` | the audience of the tokens garam issues for codehub |
| `CODEHUB_AUTHORISATION` | `authzen` | `authzen` |
| `CODEHUB_AUTHZEN_URL` | `https://localhost:28083` | garam's policy service |
| `CODEHUB_AUTHZEN_TOKEN_ENDPOINT` | `https://localhost:28082/oauth/token` | garam's machine listener, at `/oauth/token` |
| `CODEHUB_AUTHZEN_PROFILE` | `grn` | `grn` |
| `CODEHUB_AUTHZEN_CLIENT_AUTHENTICATION` | `private-key-jwt` | `private-key-jwt` |
| `CODEHUB_AUTHZEN_CLIENT_ID` | `grn:{organisation}/iam/service/codehub` | the same template: `{organisation}` is replaced by each organisation's name |
| `CODEHUB_AUTHZEN_KEY_FILE` | a throwaway `codehub.pem` | the PEM PKCS#8 private key whose public half is registered with garam for each organisation's service principal |
| `CODEHUB_AUTHZEN_KEY_ID` | the key's RFC 7638 thumbprint | the `kid` garam holds that key under |
| `CODEHUB_AUTHZEN_ISSUER` | `https://localhost:28081/api` | garam's issuer identifier |
| `CODEHUB_AUTHZEN_AUDIENCE` | `policy` | `policy` |
| `CODEHUB_DIRECTORY_CHANGES` | `garam` | `garam` |
| `CODEHUB_DIRECTORY_GARAM_URL` | `https://localhost:28083` | garam's policy service |

The end-to-end test also sets `SSL_CERT_FILE` to its throwaway certificate authority, because its TLS certificates are not publicly trusted; an operator whose garam serves publicly trusted certificates does not.

`CODEHUB_DIRECTORY_CHANGES=garam` has to be set before any repository garam must know is created. With the default `none`, `codehub` accepts and discards every change its outbox holds, so a repository created under `none` is never sent to garam later, and garam answers requests about it as for a repository it does not hold (`ch` exits 5). Creation answers after `CODEHUB_DIRECTORY_DELIVERY_WAIT` (2 seconds by default) whether or not garam has the repository yet, so a step that needs garam to know it waits for that.

### See it work

```console
$ make e2e-garam
```

The target starts garam's stack from `dev/garam/compose.yaml` (the server and policy images pinned by digest, two PostgreSQL containers and a Caddy gateway) with keys generated for the run, runs the tests of `e2e/tests/garam.rs` against the built `codehub` and `ch`, and removes the stack, its volumes and the keys at every exit. It needs the images in the local Docker image store (`pull_policy: never`), free host ports 28081 to 28083, and is not part of `make ci`. After the services of `compose.yaml` are healthy and the build is done, its output ends:

```text
policy feed started: 0 entities marked

running 7 tests
test garam_token_authenticates_a_person_and_another_audience_does_not ... ok
test garam_denies_creating_an_organisation_and_a_repository_until_the_owner_has_a_role_link ... ok
test garam_allows_an_owner_to_land_a_change ... ok
test garam_denies_a_path_its_guardrail_forbids_beside_one_it_allows ... ok
test garam_refuses_a_key_it_does_not_hold_and_the_request_allows_nothing ... ok
test garam_holds_a_repository_once_the_outbox_delivers_it ... ok
test garam_allows_an_owner_to_create_a_repository ... ok

test result: ok. 7 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 16.03s

test: 7 tests ran (-p e2e --test garam)
```

The tests run in parallel, so their order differs between runs. What each shows:

- `garam_token_authenticates_a_person_and_another_audience_does_not`: a token garam issues for `codehub` authenticates a person, and a token for the audience `policy` makes `ch` exit 3.
- `garam_denies_creating_an_organisation_and_a_repository_until_the_owner_has_a_role_link`: without a role link, garam's decision denies creating an organisation and a repository (`ch` exits 5); with one, both succeed.
- `garam_refuses_a_key_it_does_not_hold_and_the_request_allows_nothing`: a key garam does not hold gets no token from garam, a server error (`ch` exits 1); the key it holds reaches garam's decision.
- `garam_allows_an_owner_to_create_a_repository`: the owner creates a repository, and another person is denied the same.
- `garam_holds_a_repository_once_the_outbox_delivers_it`: garam's policy database holds `grn:<organisation>/codehub/repo/<id>` after the repository is created, and holds none for one created under `CODEHUB_DIRECTORY_CHANGES=none`.
- `garam_allows_an_owner_to_land_a_change`: `ch` creates a repository, stages a file and lands it, and reads it back.
- `garam_denies_a_path_its_guardrail_forbids_beside_one_it_allows`: a path outside a guardrail lands, and a write under `src` is denied (`ch` exits 5).

The people, role links and guardrails are written by the test where garam's console and platform would write them; the comments on `e2e/tests/common/garam.rs` name each.

After the run, nothing is left behind:

```console
$ docker ps -a --filter name=codehub-garam-e2e
CONTAINER ID   IMAGE     COMMAND   CREATED   STATUS    PORTS     NAMES
```
