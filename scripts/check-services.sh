#!/bin/sh
# Smoke test of the check services: every variable is set and each service
# answers. Prints one line per service and fails on the first problem.
set -eu

cd "$(dirname "$0")/.."

fail() {
    echo "check-services: $*" >&2
    exit 1
}

# dev/check-env.sh is sourced only when the caller has not exported the
# variables itself; either way the checks below read what the process sees.
if [ -z "${CODEHUB_TEST_POSTGRES_URL:-}" ]; then
    set -a
    . dev/check-env.sh
    set +a
fi

for tool in curl jq docker openssl; do
    command -v "$tool" >/dev/null 2>&1 || fail "$tool is required"
done

for name in \
    CODEHUB_TEST_POSTGRES_URL \
    CODEHUB_TEST_S3_ENDPOINT CODEHUB_TEST_S3_REGION CODEHUB_TEST_S3_BUCKET \
    CODEHUB_TEST_S3_ACCESS_KEY CODEHUB_TEST_S3_SECRET_KEY \
    CODEHUB_TEST_OIDC_ISSUER CODEHUB_TEST_OIDC_CLIENT_ID \
    CODEHUB_TEST_OIDC_TEST_CLIENT_ID CODEHUB_TEST_OIDC_TEST_CLIENT_SECRET \
    CODEHUB_TEST_OIDC_PASSWORD_ALICE CODEHUB_TEST_OIDC_PASSWORD_BOB \
    CODEHUB_TEST_OIDC_PASSWORD_CAROL; do
    eval "value=\${$name:-}"
    [ -n "$value" ] || fail "$name is unset"
done

# PostgreSQL: select 1 over the superuser URL, from a client on the host network.
image=$(docker inspect --format '{{.Config.Image}}' "$(docker compose ps -q postgres)") ||
    fail "the postgres container is not running"
one=$(docker run --rm --network host --entrypoint psql "$image" \
    "$CODEHUB_TEST_POSTGRES_URL" --tuples-only --no-align --command 'select 1') ||
    fail "postgres did not answer at $CODEHUB_TEST_POSTGRES_URL"
[ "$one" = "1" ] || fail "postgres answered '$one' to select 1"
echo "check-services: postgres answered select 1"

# S3: list the bucket with the access key.
listing=$(curl -sf \
    --aws-sigv4 "aws:amz:${CODEHUB_TEST_S3_REGION}:s3" \
    --user "${CODEHUB_TEST_S3_ACCESS_KEY}:${CODEHUB_TEST_S3_SECRET_KEY}" \
    "${CODEHUB_TEST_S3_ENDPOINT}/${CODEHUB_TEST_S3_BUCKET}?list-type=2") ||
    fail "s3 did not list bucket $CODEHUB_TEST_S3_BUCKET at $CODEHUB_TEST_S3_ENDPOINT"
case "$listing" in
    *"<Name>${CODEHUB_TEST_S3_BUCKET}</Name>"*) ;;
    *) fail "s3 listing does not name bucket $CODEHUB_TEST_S3_BUCKET" ;;
esac
echo "check-services: s3 listed bucket $CODEHUB_TEST_S3_BUCKET"

# S3: a PUT whose x-amz-checksum-sha256 does not match the body is rejected
# and stores nothing; the same PUT with the right checksum is accepted (the
# control, so the refusal is the checksum's and not the credential's or the key's).
s3_request() {
    curl -s -o /dev/null -w '%{http_code}' \
        --aws-sigv4 "aws:amz:${CODEHUB_TEST_S3_REGION}:s3" \
        --user "${CODEHUB_TEST_S3_ACCESS_KEY}:${CODEHUB_TEST_S3_SECRET_KEY}" "$@"
}
right=$(printf 'checksum probe' | openssl dgst -sha256 -binary | openssl base64)
wrong=$(printf 'another body' | openssl dgst -sha256 -binary | openssl base64)
object="${CODEHUB_TEST_S3_ENDPOINT}/${CODEHUB_TEST_S3_BUCKET}"
ok=$(s3_request -X PUT -H "x-amz-checksum-sha256: $right" --data-binary 'checksum probe' "$object/checksum-ok")
[ "$ok" = "200" ] || fail "s3 answered HTTP $ok to a PUT with the right checksum"
s3_request -X DELETE "$object/checksum-bad" >/dev/null
bad=$(s3_request -X PUT -H "x-amz-checksum-sha256: $wrong" --data-binary 'checksum probe' "$object/checksum-bad")
[ "$bad" = "400" ] || fail "s3 answered HTTP $bad, not 400, to a PUT with a wrong x-amz-checksum-sha256"
missing=$(s3_request "$object/checksum-bad")
[ "$missing" = "404" ] || fail "s3 stored an object whose checksum did not match (GET answered HTTP $missing)"
echo "check-services: s3 rejected a PUT with a wrong x-amz-checksum-sha256 (400) and accepted the right one"

# OIDC: discovery document, then the device grant end to end as alice.
issuer=$CODEHUB_TEST_OIDC_ISSUER
discovery=$(curl -sf "$issuer/.well-known/openid-configuration") ||
    fail "dex did not serve the discovery document at $issuer"
[ "$(echo "$discovery" | jq -r .issuer)" = "$issuer" ] ||
    fail "discovery issuer differs from CODEHUB_TEST_OIDC_ISSUER"
jwks_uri=$(echo "$discovery" | jq -r .jwks_uri)
device_endpoint=$(echo "$discovery" | jq -r .device_authorization_endpoint)
token_endpoint=$(echo "$discovery" | jq -r .token_endpoint)
curl -sf "$jwks_uri" | jq -e '.keys | length > 0' >/dev/null || fail "no signing key at $jwks_uri"

jar=$(mktemp)
trap 'rm -f "$jar"' EXIT
scope="openid email audience:server:client_id:codehub"

device=$(curl -sf "$device_endpoint" -d "client_id=$CODEHUB_TEST_OIDC_CLIENT_ID" \
    --data-urlencode "scope=$scope") || fail "device authorization request failed"
device_code=$(echo "$device" | jq -r .device_code)
user_code=$(echo "$device" | jq -r .user_code)

# Approve by plain HTTP: submit the user code, then the login form it leads to.
login_url=$(curl -sf -c "$jar" -b "$jar" -L -o /dev/null -w '%{url_effective}' \
    -d "user_code=$user_code" "$issuer/device/auth/verify_code") ||
    fail "dex rejected the user code"
curl -sf -c "$jar" -b "$jar" -L -o /dev/null \
    -d "login=alice@example.test" --data-urlencode "password=$CODEHUB_TEST_OIDC_PASSWORD_ALICE" \
    "$login_url" || fail "dex rejected alice's login"

token=$(curl -s "$token_endpoint" -d "client_id=$CODEHUB_TEST_OIDC_CLIENT_ID" \
    -d grant_type=urn:ietf:params:oauth:grant-type:device_code -d "device_code=$device_code")
access=$(echo "$token" | jq -r '.access_token // empty')
[ -n "$access" ] || fail "device grant yielded no access token: $token"
claims=$(echo "$access" | jq -R 'split(".")[1] | gsub("-";"+") | gsub("_";"/") | @base64d | fromjson') ||
    fail "the access token is not a JWT"
echo "$claims" | jq -e '.aud | index("codehub")' >/dev/null || fail "access token aud lacks codehub: $claims"
echo "check-services: dex discovery served; alice's device code approved, access token aud $(echo "$claims" | jq -c .aud)"

# The confidential client obtains a token directly.
direct=$(curl -sf -u "$CODEHUB_TEST_OIDC_TEST_CLIENT_ID:$CODEHUB_TEST_OIDC_TEST_CLIENT_SECRET" "$token_endpoint" \
    -d grant_type=password -d "username=bob@example.test" \
    --data-urlencode "password=$CODEHUB_TEST_OIDC_PASSWORD_BOB" --data-urlencode "scope=$scope") ||
    fail "the codehub-test client could not obtain a token for bob"
echo "$direct" | jq -e '.access_token' >/dev/null || fail "no access token for bob"
echo "check-services: codehub-test client obtained a token for bob"
