# ADR 0058: the chart routes through Gateway API as well as Ingress, and mounts the AuthZEN key

> Status: accepted; the path split of its routes is superseded by ADR 0060
> Date: 2026-11-21

Append-only: once merged, the body below is not rewritten. A fact later found wrong is corrected in an appended note, not edited out; a revised decision is a new ADR that supersedes this one.

## Context

ADR 0057's chart routes with one `Ingress` and has no way to give the server the private key of `private-key-jwt` (ADR 0042). Goal #316 runs codehub on a cluster whose entry is an Envoy Gateway, where the ingress controller is retired, and whose identity and authorisation come from garam, which authenticates codehub by that key. The names and the split of work are agreed in garamsh/discussions#6, discussioncomment-19779859.

Choices considered and not taken:

- **Replace the `Ingress` with an `HTTPRoute`.** Clusters with an ingress controller are still deployments of this chart, and the fault runs (ADR 0057) use one.
- **One `httpRoute` shape for the API and the registry.** The registry is optional and has a host of its own, so one list of hostnames could not say which host reaches which Service.
- **Rendering the gateway's policies (body limit, timeouts) from the chart.** They are the cluster owner's, belong to the Gateway or the route's `targetRef`, and differ by implementation.
- **Reading the key as a setting's value.** ADR 0042 reads the key from a file so that its content stays out of the configuration output; a Secret is mounted as that file.

## Decision

- **`httpRoute`** renders a Gateway API `HTTPRoute` for the API: `enabled` (off by default), `parentRefs` passed through verbatim, and `hostnames`. It splits the paths as the `Ingress` does: `/v1/runner/` to `runner-api`, every other path to `api`.
- **`registry.httpRoute`** (`enabled`, `hostnames`) renders a second `HTTPRoute`, for the registry's host, that sends `/v2/` to the registry Service. It uses `httpRoute.parentRefs`, since one Gateway serves both hosts, and renders only where the registry is enabled.
- **`ingress` and `httpRoute` are independent.** Each renders where enabled, and enabling both renders both. A route whose `parentRefs` or hostnames are empty fails the render.
- **The body limit of 64 MiB** (ADR 0055) stays the cluster's to set, on the ingress controller or on the gateway, and the chart renders no policy for it. `docs/guide/deployment.md` names both forms.
- **`authzen.keySecret`** (`name`, `key`) names the Secret holding the `private-key-jwt` key. Where it names one, the key is mounted read-only at `/etc/codehub/authzen/key.pem` in `api`, `runner-api` and `worker`, the roles that call the decision point or garam, and the shared ConfigMap sets `CODEHUB_AUTHZEN_KEY_FILE` to it. Where it names none, nothing is mounted and nothing is set.
- **A rotated key takes a rollout.** The server reads the file at start (`CODEHUB_AUTHZEN_KEY_FILE`, ADR 0042), so replacing the Secret's content does not reach a running pod. The chart cannot see the content and adds no checksum of it; the operator restarts the Deployments.
- **The Secret contract** is a table in `docs/guide/deployment.md`: each Secret the chart can read, by value path, default key, content, format, roles and whether it is required.

## Consequences

- aurora deploys the chart with its own values and no template of its own: a Gateway `parentRef`, two hosts and the Secrets named in the contract.
- A registry behind a Gateway needs `httpRoute.parentRefs` even where the API is routed some other way.
- The chart carries two routing objects that split the same paths; a change to the split is made in both templates, and the fixtures `ci/full.yaml` and `ci/garam.yaml` render one each.
- A key replaced in its Secret is used by pods started after the replacement, so a rotation that must be immediate restarts the Deployments.

## Signs the decision was wrong

- A third entry (a service mesh gateway, another route kind) is asked for, and the routing objects diverge from each other.
- An operator needs the chart to render a gateway policy so that the body limit is not forgotten, and the limit is missed in a deployment because the chart left it out.
