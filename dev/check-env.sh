# Environment the checks read; the only place these values are written down.
# Sourced with allexport by the Makefile and by scripts/*.sh. Keep it in step
# with compose.yaml (ports, credentials), dev/dex/config.yaml and
# dev/keycloak/codehub-realm.json (issuers, clients, passwords).
CODEHUB_TEST_POSTGRES_URL=postgres://codehub:codehub@127.0.0.1:15432/postgres

CODEHUB_TEST_S3_ENDPOINT=http://127.0.0.1:19000
CODEHUB_TEST_S3_REGION=us-east-1
CODEHUB_TEST_S3_BUCKET=codehub-test
CODEHUB_TEST_S3_ACCESS_KEY=codehub-test-access
CODEHUB_TEST_S3_SECRET_KEY=codehub-test-secret-key

CODEHUB_TEST_OIDC_ISSUER=http://127.0.0.1:18080/dex
CODEHUB_TEST_OIDC_CLIENT_ID=ch
CODEHUB_TEST_OIDC_TEST_CLIENT_ID=codehub-test
CODEHUB_TEST_OIDC_TEST_CLIENT_SECRET=codehub-test-secret
CODEHUB_TEST_OIDC_PASSWORD_ALICE=alice-check-password
CODEHUB_TEST_OIDC_PASSWORD_BOB=bob-check-password
CODEHUB_TEST_OIDC_PASSWORD_CAROL=carol-check-password

# Keycloak realm codehub. The audience is the resource server's client id; the
# introspection client is a separate client that every test token also names in
# its audience, because Keycloak introspects only tokens addressed to the caller.
CODEHUB_TEST_KEYCLOAK_ISSUER=http://127.0.0.1:18081/realms/codehub
CODEHUB_TEST_KEYCLOAK_AUDIENCE=codehub
CODEHUB_TEST_KEYCLOAK_INTROSPECTION_CLIENT_ID=codehub-introspection
CODEHUB_TEST_KEYCLOAK_INTROSPECTION_CLIENT_SECRET=codehub-introspection-secret
# Password-grant clients: tokens for codehub's audience, for another audience, and
# for codehub's audience with a two-second lifetime.
CODEHUB_TEST_KEYCLOAK_TEST_CLIENT_ID=codehub-test
CODEHUB_TEST_KEYCLOAK_TEST_CLIENT_SECRET=codehub-test-secret
CODEHUB_TEST_KEYCLOAK_OTHER_CLIENT_ID=codehub-test-other
CODEHUB_TEST_KEYCLOAK_OTHER_CLIENT_SECRET=codehub-test-other-secret
CODEHUB_TEST_KEYCLOAK_SHORT_CLIENT_ID=codehub-test-short
CODEHUB_TEST_KEYCLOAK_SHORT_CLIENT_SECRET=codehub-test-short-secret
# RFC 8693: the upstream client yields a token for the exchange client, which
# exchanges it for one for codehub's audience.
CODEHUB_TEST_KEYCLOAK_UPSTREAM_CLIENT_ID=codehub-test-upstream
CODEHUB_TEST_KEYCLOAK_UPSTREAM_CLIENT_SECRET=codehub-test-upstream-secret
CODEHUB_TEST_KEYCLOAK_EXCHANGE_CLIENT_ID=codehub-exchange
CODEHUB_TEST_KEYCLOAK_EXCHANGE_CLIENT_SECRET=codehub-exchange-secret
CODEHUB_TEST_KEYCLOAK_PASSWORD_ALICE=alice-check-password
CODEHUB_TEST_KEYCLOAK_PASSWORD_BOB=bob-check-password
CODEHUB_TEST_KEYCLOAK_PASSWORD_CAROL=carol-check-password
