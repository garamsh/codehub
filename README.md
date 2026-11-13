# codehub

A server-centred version control system for codebases worked on by people and by hundreds of concurrent agents. It replaces Git with content-addressed immutable objects and the changeset as the unit of work. History and blame are computed server-side as derived data, and commits and landing happen server-side.

- **Tenancy.** An organisation contains repositories; one repository is one codebase with one changeset log (`docs/architecture/tenancy.md`).
- **Interfaces.** A minimal CLI named `ch` and the HTTP API described by OpenAPI (`docs/architecture/interfaces.md`). codehub builds no MCP server.
- **Execution.** A runner, `codehub-runner`, polls the server for the checks of each landing and reports their results (`docs/architecture/runner.md`).
- **Implementation.** Rust, structured as a hexagon (`docs/architecture/structure.md`).

To try it, follow `docs/quickstart.md`. To deploy the published images, read `docs/deployment.md`. Design decisions are recorded under `docs/architecture/`, and the rules contributors follow start at `AGENTS.md`.
