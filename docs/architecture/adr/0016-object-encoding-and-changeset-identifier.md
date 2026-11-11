# ADR 0016: Objects use a project-defined binary encoding, and a changeset is identified by its address

> Status: accepted
> Date: 2026-11-11

Append-only: once merged, the body below is not rewritten. A fact later found wrong is corrected in an appended note, not edited out; a revised decision is a new ADR that supersedes this one.

## Context

ADR 0011 made objects SHA-256 addressed and trees and file-version nodes derived, and left open the byte layout trees, changesets and file-version nodes are hashed and stored in. ADR 0012 made moving a ref a compare-and-swap on the ref's current changeset identifier, and did not fix what that identifier is. Both were open questions in `docs/architecture/source-control.md`.

On 2026-11-11 the user delegated every choice of milestone M1 to the PM ("그냥 너가 알아서 전부다 해줘"), recorded in issue #19, whose decisions 4 and 5 are recorded here.

Forces:

- An address is a hash of bytes, so every object must have exactly one encoding: two encoders that disagree on one byte give one value two addresses, and a tree shared between versions stops being shared.
- The domain ring computes trees at landing (ADR 0011) and is pure (ADR 0014), so the encoding is written and read there with no I/O.
- A changeset is a delta against its first parent and names that parent; the identifier the parent is named by is the one this ADR fixes.

Alternatives not taken:

- Deterministic CBOR (RFC 8949 §4.2). Its rules, such as map keys sorted by their encoded bytes, are the encoder's to enforce. The Rust crate `ciborium` 0.2.2 documents no mode that enforces them: its `ser` module exposes one function, `into_writer`, with no options, and its crate documentation, under "Design Decisions", represents a map as `Vec<(Value, Value)>` because "this type preserves the order of the pairs on the wire" (https://docs.rs/ciborium/0.2.2/ciborium/ser/index.html, https://docs.rs/ciborium/0.2.2/ciborium/). Other Rust CBOR crates were not examined. Determinism would therefore have been the project's to enforce and test on top of a dependency, which is the work a project-defined encoding asks for without the dependency.
- An identifier assigned by the log. The log would mint the identifier when the changeset is appended, so a changeset could not be named before it is in the log, and a parent would be named by something its bytes do not determine.

## Decision

- **Encoding.** Changesets, trees and file-version nodes use a project-defined binary encoding: a leading format-version byte, length-prefixed fields with fixed-width big-endian integers, and entries sorted by path-component bytes, so that every value has exactly one encoding. The specification is in `docs/architecture/source-control.md`.
- **Address.** An object's address is the SHA-256 of its encoded bytes, format-version byte included.
- **Changeset identifier.** A changeset is identified by the SHA-256 address of its encoding.

## Consequences

- Every object, the changeset included, is checked the same way: hash the bytes and compare with the address. An export bundle (ADR 0005) can be verified without the log.
- A changeset's identifier is known before it is appended, so a client or the server can compute it without a round trip to the log.
- A changeset names its parent, so re-parenting at landing (ADR 0012) produces a different changeset with a different identifier; a draft and the changeset it lands as do not share one.
- The identifier carries no order. The log's order is held beside it, not read from it.
- No serialisation crate is a dependency; the encoder and decoder are project code, and the specification is the only contract a second implementation has.
- A decoder rejects any input that is not the one encoding of its value — unknown version, unsorted or duplicate entries, trailing bytes — so a malformed object is refused rather than given a second address.
- A layout change is a new format-version byte. Objects already stored keep their bytes and addresses, so every reader decodes each version still referenced.
- What the first version cannot represent — symbolic links, any mode other than executable, chunked files until chunking is specified — needs a new format version to add.

## Signs the decision was wrong

- A second implementation written from the specification alone, such as a client in another language or an export reader, produces different bytes for the same object, showing the specification under-determines the encoding.
- A new format version is needed within the first few changes to the object model — for symbolic links, more file modes, or the chunked entry — and readers carry several versions where CBOR's self-describing maps would have taken an added field without one.
- Debugging and inspecting stored objects keeps requiring ad hoc decoders where generic CBOR tooling would have read them.
- Clients and agents need to follow one piece of work across its re-parenting at landing, and a second, stable identifier has to be added beside the address.
- Looking changesets up by a 32-byte hash in the log table measurably slows appends or history reads compared with a sequential key, and the log grows a second, assigned key for every query.
