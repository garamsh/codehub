# The addresses of the garam stack (dev/garam/compose.yaml) that `make e2e-garam` and
# e2e/tests/garam.rs read; the only place they are written down. Sourced with allexport by
# dev/garam/run.sh. The ports sit below the Linux ephemeral range (32768-60999) and away from the ports of
# compose.yaml (15432, 19000, 18080-18082).
CODEHUB_GARAM_E2E_GATEWAY_PORT=28081
CODEHUB_GARAM_E2E_MACHINE_PORT=28082
CODEHUB_GARAM_E2E_POLICY_PORT=28083
# garam requires its issuer identifier to be an https URL with the path /api.
CODEHUB_GARAM_E2E_ISSUER=https://localhost:28081/api
CODEHUB_GARAM_E2E_TOKEN_ENDPOINT=https://localhost:28082/oauth/token
CODEHUB_GARAM_E2E_POLICY_URL=https://localhost:28083
