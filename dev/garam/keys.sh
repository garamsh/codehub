#!/bin/sh
# Writes the throwaway key material of the garam stack into the directory $1, every file readable
# by the owner alone: garam's token signing key, the key codehub signs its client assertions with,
# a second key no service principal holds, the service keys file that registers codehub's public
# key with garam, the key-encryption key of the machine listener, and a certificate authority with
# the certificate both TLS listeners serve. Nothing here is committed or printed.
set -eu

dir=${1:?the directory to write the keys into}
umask 077

# Every openssl call is bounded, so a stalled one fails the run.
openssl() { timeout 30 openssl "$@"; }

b64u() { basenc --base64url -w0 | tr -d =; }

# The RSA modulus of the key $1 as base64url, and the RFC 7638 thumbprint garam reads as the kid.
modulus() { openssl rsa -in "$1" -noout -modulus | cut -d= -f2 | basenc --base16 -d | b64u; }
thumbprint() {
    printf '{"e":"AQAB","kty":"RSA","n":"%s"}' "$(modulus "$1")" | openssl dgst -sha256 -binary | b64u
}

for name in signing codehub other; do
    openssl genpkey -algorithm RSA -pkeyopt rsa_keygen_bits:2048 -out "$dir/$name.pem" 2>/dev/null
done

kid=$(thumbprint "$dir/codehub.pem")
printf '%s' "$kid" >"$dir/codehub.kid"
printf '{"codehub":{"keys":[{"kty":"RSA","use":"sig","alg":"RS256","kid":"%s","n":"%s","e":"AQAB"}]}}\n' \
    "$kid" "$(modulus "$dir/codehub.pem")" >"$dir/service-keys.json"
openssl rand -hex 32 >"$dir/kek.hex"

openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:P-256 -nodes -days 2 \
    -subj "/CN=codehub garam e2e" -addext "basicConstraints=critical,CA:TRUE" \
    -addext "keyUsage=critical,keyCertSign" \
    -keyout "$dir/ca.key" -out "$dir/ca.crt" 2>/dev/null
openssl req -newkey ec -pkeyopt ec_paramgen_curve:P-256 -nodes \
    -subj "/CN=garam" -keyout "$dir/tls.key" -out "$dir/tls.csr" 2>/dev/null
printf 'basicConstraints=critical,CA:FALSE\nsubjectAltName=DNS:machine,DNS:policy,DNS:localhost,IP:127.0.0.1\n' \
    >"$dir/tls.ext"
openssl x509 -req -in "$dir/tls.csr" -CA "$dir/ca.crt" -CAkey "$dir/ca.key" -CAcreateserial \
    -days 2 -extfile "$dir/tls.ext" -out "$dir/tls.crt" 2>/dev/null
