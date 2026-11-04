# ADR 0010: the estate UUID replaces the `uuid` crate for stored identities

Status: **accepted**, 2026-11-04. Answers gap #851; refines INV-ID-2.

Stakeholders: the Director; logical-DB, whose ADR-0005 keeps `uuid` and `foundationdb-tuple`'s `uuid` feature until this lands; every estate crate that persists or keys by an identity
Concerns: an identity that cannot be serialized, constructed without `Result` plumbing, or packed into a store key byte-identically to the bytes already stored
Requirements: R1, R10, R12
Viewpoint: the capability viewpoint: one row per missing capability, its API, its bytes and its evidence
Elements and relations: `lgwks_std::id::Uuid`, `id::serde_impl`, `id::v7::{V7Generator, now_v7}`, `id::tuple`; `ci_local::scratch` names its volume with `Uuid::now_v7`; `lgwks id new|key|read` issues ids and packs and reads keys from the shell; logical-DB's model newtypes wrap `Uuid` and its FDB keys call `pack_tuple`

## Context

logical-DB tried to move its identities onto `lgwks_std::id::Uuid` (3.2.2) and reverted
(its ADR-0005). Three things were missing. There was no serde: JSON carries the hyphenated text
and CBOR carries 16 bytes through the `is_human_readable` switch. `new_v7` is fallible, which spread
`Result` through hundreds of constructors with no recovery path. And there was no FoundationDB tuple
element: keys pack identities as type code `0x30` through `foundationdb-tuple`. Data already stored
in those three shapes must read back unchanged.

## Decision

1. `Uuid` implements serde with `json` or `ron` on: text when the format is human-readable, the 16
   network-order bytes otherwise, the `uuid` crate's shape. It reads text, 16 bytes, or a sequence
   of exactly 16 bytes, and refuses everything else (INV-ID-3).
2. `Uuid::now_v7() -> Uuid` never refuses. It shares `new_v7`'s process-wide generator and order,
   and clamps where `new_v7` refuses (INV-ID-4):
   - A clock before 1970 is a clock that stepped backwards, so the last stamp is kept.
   - A refused entropy read draws nothing in its place. The counter steps and the random field is
     the last one the OS gave, so INV-RANDOM-1 still holds: nothing is substituted.
   - A stamp past 48 bits is pinned at the ceiling.

   Each clamp is logged under `trace`. `new_v7` stays for a caller that must see the refusal.
3. `Uuid::pack_tuple` appends `0x30` and the 16 bytes. `Uuid::unpack_tuple` reads one element and
   returns the rest of the key, refusing another code or a short key by name (INV-ID-5). This is
   the tuple layer's specified UUID encoding, so the bytes equal `foundationdb-tuple`'s.

## Alternatives rejected

- **Panicking on entropy failure, as `uuid::Uuid::now_v7` does.** A panic in a constructor takes
  down every tenant of the process. Cost: availability for a host fault that ordering does not need.
- **A userspace CSPRNG seeded once.** That is a second randomness source, which INV-RANDOM-1 refuses.
- **Depending on `foundationdb-tuple` from `lgwks_std`.** That adds an edge, plus the FDB client's
  tree, to the core surface for 17 bytes the specification fixes. Cost: INV-DEP-20.

## Consequences

The following stays open and is recorded on #851:

- **The full tuple layer.** There are no other type codes (integers, strings, nested tuples,
  versionstamps). A logical-DB key that mixes an identity with other elements still packs those
  through `foundationdb-tuple`, and its newtype implements `TuplePack` by calling `pack_tuple`.
- **The ECS identity surface.** `lgwks_bot`'s ECS keys effects by `EffectIdentity` digests, not
  by `Uuid`. A projection keyed by tenant and identity (#835) has no `Uuid` component or tuple
  key yet.
- **CBOR.** No CBOR codec exists in the estate, so the binary arm is proved through a
  non-human-readable test serializer.

## Correspondence

Refines INV-ID-2 (the v7 order, which `now_v7` shares). Related: ADR 0003 (on by default),
logical-DB ADR-0005 (the consumer's exit criteria), #835 (ECS projections), #851.

## Verification

- `id::sim_serde`: the RFC 9562 vectors, seeded round trips through JSON, RON, bytes and
  sequences, and refusals.
- `id::sim_v7_clamped`: every seeded clock and entropy pattern stays strictly ordered, the
  ceiling pins, the first id with no entropy, and 100 threads on the live generator never repeat.
- `id::sim_tuple`: the RFC vector against the specification's bytes, seeded round trips with the
  key's tail left to the caller, packed order equal to identifier order, and every truncation and
  every other code refused.
- `ci_local::scratch::tests`: the scratch name.
- `tests/it/sim_lgwks_id.rs`: the shipped `lgwks id` against the specification's bytes, with
  seeded round trips, issue order and every refusal kind.
