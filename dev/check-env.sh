# Environment the checks read; the only place these values are written down.
# Sourced with allexport by the Makefile and by scripts/*.sh. Keep it in step
# with compose.yaml (ports, credentials), dev/dex/config.yaml (issuer, clients,
# passwords) and dev/seaweedfs/s3.json (S3 credentials).
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
