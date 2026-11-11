# codehub

A server-centred version control system for codebases worked on by people and by hundreds of concurrent agents. It replaces Git with content-addressed immutable objects and the changeset as the unit of work. History and blame are computed server-side as derived data, and commits and landing happen server-side.

- **Tenancy.** An organisation contains workspaces; one workspace is one codebase with one changeset log. An organisation holds a base layer shared by its workspaces.
- **Interfaces.** A minimal CLI named `ch` and an MCP server.
- **Execution.** Runners follow the GitHub Actions self-hosted runner model and run each job in one disposable environment.
- **Implementation.** Rust, structured as a hexagon (`docs/architecture/structure.md`).

The repository holds design documents only; no source code exists yet. Design decisions are recorded under `docs/architecture/`, and the rules contributors follow start at `AGENTS.md`.
