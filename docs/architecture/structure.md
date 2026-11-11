# Structure

The hexagonal structure of the system: what the unit is, what may reference what, and when a new port is earned.

## Current decisions

- **The system is a hexagon.** The inside is the application, the subject matter and the decisions made over it, and it names no technology. The outside is adapters, one per technology.
- **The implementation language is Rust** (ADR 0002). The stack convention spelling these rules in Rust lands separately; until it exists, the package and file names a unit takes are not fixed.

### Boundaries and dependency direction

- **The port is the unit.** One port per conversation with the outside, named for that conversation and never for the technology serving it.
- **Dependencies point inward.** An adapter references the application; the application references no adapter. A technology's name inside the application — a framework type, a driver, a wire format, a table — is a defect, even where the language permits it.
- **Every crossing goes through a port.** A driving adapter reaches the application only through a port; the application reaches the outside only through a port an adapter implements. Something outside reached from the inside without a port is a defect even where the call compiles.
- **A port is declared in the application's terms.** Its operations and the types crossing it are the application's, and converting them to and from the technology's is the adapter's work. A type belonging to the technology in a port's signature puts the outside inside.
- **An adapter's only counterpart is its port.** One adapter calling another carries application work outside, where no port names it and no substitute stands in for it.
- **A new port is earned** by a conversation whose technology can be replaced without the application changing, and which the application can name without naming that technology. Cockburn's statement expects two or more adapters on a port — the real technology, and one for a test harness (*Hexagonal architecture*, 2005) — so a port with a single adapter that nothing substitutes is indirection.
- **Whether the inside is itself partitioned is contested.** Cockburn's statement leaves it undivided. Later readings built on it — Jeffrey Palermo's Onion Architecture (2008), and Robert C. Martin's *Clean Architecture* (2017), which names hexagonal among its sources — divide it into concentric rings with the subject matter innermost and every reference pointing inward. A project takes one, records which, and under the second a reference pointing outward from an inner ring is a defect. This project has not taken a side; see open questions.

### Shape rules every project carries

- **The application has one composition site.** It is the one place that constructs concrete implementations, and it decides which implementation each declared abstraction receives.
- **Every other unit receives what it depends on and constructs none of it**, whatever the shape makes the unit — a domain, a layer, or an adapter.
- **The composition site stays thin.** It constructs and passes; it decides nothing else.

## Rationale

- Hexagonal structure was chosen in ADR 0001; the signs that it was wrong are recorded there.
- Rust as implementation language is recorded in ADR 0002.

## Open questions

- Whether the application is partitioned into rings or stays undivided. Under rings, a reference pointing outward from an inner ring is a defect; undivided, the rule is silent. It is settled in an ADR when the first domain is written (`docs/architecture/README.md` rule 7).
