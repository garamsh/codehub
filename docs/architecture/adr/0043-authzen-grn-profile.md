# ADR 0043: The AuthZEN adapter has a GRN profile, in which a repository is named by its id and a path is context

> Status: accepted
> Date: 2026-11-13

Append-only: once merged, the body below is not rewritten. A fact later found wrong is corrected in an appended note, not edited out; a revised decision is a new ADR that supersedes this one.

## Context

The AuthZEN adapter (ADR 0030) sends neutral names, read in `crates/adapter-authzen/src/authorisation_decision.rs` at `171fb60967154bd45d285cb3f79e5985e2f7449b`:
- the subject is `{type: "principal", id: <sub>, properties: {issuer}}`;
- the actions are `create_organisation`, `create_repository`, `administer_members`, `administer_runners`, `read` and `write`;
- an organisation is `{type: "organisation", id: <org>}`;
- a repository is `{type: "repository", id: "<org>/<name>"}`, with the organisation, the repository and any path as resource properties.

Cerbos, the development decision point, is written against those names (`dev/cerbos/policies/`).

ADR 0028 has codehub speak standards only and name no external product. It names an organisation and its repositories the same on both sides, and sends a path to the decision point as a resource attribute. ADR 0030 maps the path as a resource property.

garam is the decision point codehub joins under garamsh/aurora#749. garam's ADR-0149, merged on garam's `dev` branch as `adf2e1c8855dd75b463675f2035c661cd8aad957`, states what garam accepts from codehub. It was decided with codehub's answers on garamsh/garam#1760 (comment 6830559368):
- the subject and resource `type` is `grn`. garam resolves the entity type from the id, so codehub never tells a person from an agent (ADR 0027);
- the subject id is the token's `sub`, unchanged, and no subject properties are sent. A garam `sub` is a GRN naming its organisation and kind, unique across garam, and codehub verifies tokens of one configured issuer only (ADR 0029). Dropping the issuer therefore names no principal ambiguously;
- a repository is `grn:<org>/codehub/repo/<id>`, where `<id>` is the decimal form of the repository's identity column. An organisation is `grn:root/iam/org/<org>`. No resource properties are sent; garam's contract refuses them;
- the actions are `codehub:org.use` (codehub's `CreateOrganisation`), `codehub:repo.create`, `codehub:repo.read`, `codehub:repo.write` and `codehub:runner.administer`. `AdministerMembers` has no action, because external mode refuses the member endpoints (ADR 0030);
- a path is `context.path`: its components joined by `/`, exact UTF-8, sent only with `repo.read` and `repo.write`;
- garam's token endpoint answers `unauthorized_client` (RFC 6749 §5.2) for an organisation garam does not hold, once the assertion's signature has verified, and `invalid_client` for every failure to authenticate.

garam's ADR-0149 is "accepted; not built". Its HTTP binding is garamsh/garam#1809, the `unauthorized_client` answer garamsh/garam#1811, and the manifest garamsh/garam#1815.

A repository's identity column is `bigint generated always as identity` and is never reused (`crates/adapter-postgres/migrations/0001_initial.sql`). Nothing renames or deletes a repository. A name, by contrast, could be reused once deletion exists. A grant written for the first repository of a name would then hold for the next one.

A path component is bytes (`crates/app/src/domain/path.rs`, `Component(Vec<u8>)`), so a path need not be UTF-8.

Alternatives not taken:

- **Replace the neutral names with garam's.** Cerbos, its policies and the external-mode tests are written against the neutral names, and another AuthZEN decision point need not speak GRNs.
- **A mapping table in configuration**, from each action and type to a configured name. It cannot express a resource id built from the repository's id, and ADR 0028 keeps a mapping table out of the core. Two fixed profiles are what is needed.
- **Name the repository in the GRN by its name.** That GRN is reused when a name is deleted and created again, as above.
- **Send a non-UTF-8 path in a lossy rendering.** Two paths could then be one string, and a rule on one would decide the other.
- **Read `unauthorized_client` as a server error**, as ADR 0042 reads every refused token request. It is a definite refusal of one organisation by garam, after codehub's key was verified. A server error would report an outage where there is none.

## Decision

- The AuthZEN adapter has two profiles, selected by the setting `CODEHUB_AUTHZEN_PROFILE`: `plain`, which is today's names and the default, and `grn`. The profile changes what is sent, not when the adapter asks (ADR 0038, ADR 0041). The `grn` profile requires `private_key_jwt` client authentication (ADR 0042), and the server refuses to start with it otherwise.
- In the GRN profile the adapter sends what garam's ADR-0149 states, above:
  - the type `grn` for subject and resource;
  - the subject id as the token's `sub`, with no subject properties;
  - repositories as `grn:<org>/codehub/repo/<id>` by the repository's id, and organisations as `grn:root/iam/org/<org>`;
  - the `codehub:` action names, and no resource properties;
  - the path as `context.path`, with `repo.read` and `repo.write` only.
- A path that is not valid UTF-8 is denied without asking, in both profiles. The plain profile renders the path with `String::from_utf8_lossy` today (`crates/app/src/domain/path.rs`, `impl Display for Path`), which has the same defect.
- `AdministerMembers` is never asked in external mode (ADR 0030). In the GRN profile the adapter treats it as a defect: it fails the request as a server error and allows nothing.
- With `private_key_jwt` client authentication (ADR 0042), a token request answered `unauthorized_client` denies every decision asked for that organisation; the request answers as any denial does (ADR 0033). Every other refused or failed token request stays a server error that allows nothing (ADR 0033). Like a server error, a denial is not cached. This supersedes, for `unauthorized_client`, ADR 0042's bullet that a refused token fails the request as a server error.
- The repository's id is carried to the adapter in the authorisation decision port's resource, as it already is (`Resource::Repository` and `Resource::Path` hold `repository`). The port does not change, and the core still hands the path to the port as a property of the resource (ADR 0021); only the adapter's rendering differs.
- The HTTP API's repository representation carries the repository's id, so a policy author can learn which repository a GRN names.
- In the `grn` profile this supersedes three decisions of ADR 0028: "codehub speaks standards only", for garam's GRN names and its entity feed (ADR 0044); "an organisation and its repositories are named the same on both sides", for repositories, which are named by id; and "a path is sent to the decision point as a resource attribute". It also supersedes ADR 0030's "The path is a resource property" in that profile. In the `plain` profile all four stand.

## Consequences

- One garam organisation is served by one codehub deployment, because a repository's id is unique only within a deployment. This is garam's ADR-0149 consequence too.
- A garam policy on a repository names a number, which a reader resolves through codehub.
- Cerbos and the plain profile are unchanged except for the UTF-8 denial.
- Nothing of the `grn` profile can be shown against garam until garamsh/garam#1809, #1811 and #1815 are served. Until then it is tested against a stand-in that speaks ADR-0149's contract.
- Moving between the two profiles is a code change in codehub where ADR 0028 expected configuration alone, since a third layer with its own names needs its own profile.
- `CODEHUB_AUTHZEN_EVALUATIONS` stays the operator's decision (ADR 0041). In the GRN profile each path sits in its own evaluation's context, not in a shared resource id. Whether garam answers each evaluation separately rests on garamsh/garam#1809.
- An organisation that garam does not hold is refused as a denial, so external mode's lazy creation (ADR 0030) creates nothing for it.

## Signs the decision was wrong

- A third decision point needs a third profile, which would show that the profile should be data rather than code.
- A garam organisation needs two codehub deployments.
- garam's contract changes in a way that requires codehub to parse a GRN.
