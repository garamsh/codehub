# The pins and addresses of the gaon stack that `make e2e-gaon` builds and
# crates/codehub-runner/tests/gaon_live.rs reads; the only place they are written down. Sourced
# with allexport by dev/gaon/run.sh.
# A pin moves by editing it here and running `make e2e-gaon`; no bot proposes the bump.
# The commit of garamsh/gaon that gaon-apiserver is built from: the merge of garamsh/gaon#244.
CODEHUB_GAON_E2E_SHA=ad26e9b7ffc9ab2446cb8c3ac4489aefec7b0438
CODEHUB_GAON_E2E_REPOSITORY=https://github.com/garamsh/gaon
# golang:1.26.11-bookworm; apiserver/go.mod at that commit needs Go 1.26.
CODEHUB_GAON_E2E_GO_IMAGE=golang@sha256:dc40a13b61304647ed7e5d581d246b86413dfc29d63c6d018310feb93cd98fc0
# quay.io/coreos/etcd:v3.6.16, the etcd release gaon builds against.
CODEHUB_GAON_E2E_ETCD_IMAGE=quay.io/coreos/etcd@sha256:a9c7cc12156f4b26ca41f0b57ba5b76c45037ff6c1e47ea3d68cb7908d48a965
# The ports sit below the Linux ephemeral range (32768-60999) and away from dev/garam (28081-28083)
# and compose.yaml (15432, 19000, 18080-18082).
CODEHUB_GAON_E2E_API_PORT=28091
CODEHUB_GAON_E2E_ETCD_PORT=28092
# gaon requires its service-account issuer to be an https URL with no path; nothing dials it.
CODEHUB_GAON_E2E_ISSUER=https://localhost:28091
CODEHUB_GAON_E2E_ROOT=https://localhost:28091/apis/compute.gaon.garam.sh/v1alpha1
