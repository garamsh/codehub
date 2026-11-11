#!/bin/sh
# Starts the check services (compose.yaml) and waits until they are healthy,
# then creates the S3 bucket if it is missing. Safe to run repeatedly.
set -eu

cd "$(dirname "$0")/.."
set -a
. dev/check-env.sh
set +a

docker compose up --detach --wait

# The S3 server has no bucket-creation setting; a signed PUT does it.
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
echo "services-up: postgres, s3 and dex healthy; bucket ${CODEHUB_TEST_S3_BUCKET} present"
