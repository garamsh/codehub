#!/bin/sh
# Starts the check services (compose.yaml) and waits until they are healthy,
# then creates the S3 bucket if it is missing. Safe to run repeatedly.
# Running containers are kept; pick up a changed image or configuration with
# `make services-down` and then `make services`.
set -eu

cd "$(dirname "$0")/.."
set -a
. dev/check-env.sh
set +a

docker compose up --detach --wait --no-recreate

# The S3 image has no healthcheck of its own; wait for the store to be ready.
tries=0
until curl -sf -o /dev/null "${CODEHUB_TEST_S3_ENDPOINT}/minio/health/ready"; do
    tries=$((tries + 1))
    if [ "$tries" -gt 60 ]; then
        echo "services-up: s3 not ready at ${CODEHUB_TEST_S3_ENDPOINT}" >&2
        exit 1
    fi
    sleep 1
done

# The Keycloak image has no shell or client to probe with; wait on the discovery document.
tries=0
until curl -sf -o /dev/null "${CODEHUB_TEST_KEYCLOAK_ISSUER}/.well-known/openid-configuration"; do
    tries=$((tries + 1))
    if [ "$tries" -gt 120 ]; then
        echo "services-up: keycloak not ready at ${CODEHUB_TEST_KEYCLOAK_ISSUER}" >&2
        exit 1
    fi
    sleep 1
done

# The store has no bucket-creation setting; a signed PUT does it.
status=$(curl -s -o /dev/null -w '%{http_code}' -X PUT \
    --aws-sigv4 "aws:amz:${CODEHUB_TEST_S3_REGION}:s3" \
    --user "${CODEHUB_TEST_S3_ACCESS_KEY}:${CODEHUB_TEST_S3_SECRET_KEY}" \
    "${CODEHUB_TEST_S3_ENDPOINT}/${CODEHUB_TEST_S3_BUCKET}")
case "$status" in
    200 | 409) ;;
    *)
        echo "services-up: creating bucket ${CODEHUB_TEST_S3_BUCKET} returned HTTP $status" >&2
        exit 1
        ;;
esac
echo "services-up: postgres, s3, dex and keycloak healthy; bucket ${CODEHUB_TEST_S3_BUCKET} present"
