# `lgwks_std` invariants

The `lgwks_std` part of the invariant register, one short line per rule: rule · why · enforced by. The rest is in
[`INVARIANTS.md`](../../INVARIANTS.md).

- **INV-STD-SIM-1** `Geometry::score` accepts both `[f64; 4]` and `BoundingBox`. · why: #160 · enforced by: `tests/it/similarity_public_api.rs`
- **INV-STD-SIM-2** Similarity keeps raw cosine in `[-1, 1]`; a refused component withdraws the verdict and deficits are never renormalized. · why: #160 · enforced by: `tests/it/similarity_evidence_contract.rs`, `tests/it/sim_similarity_sweep.rs`
- **INV-STD-RETRY** `RetryPolicy` is pure and allocation-free: exact capped exponential delay, one attempt at least, and deadline equality refuses. · why: #164 · enforced by: `lgwks_std::retry::tests`
- **INV-STD-SEEDED-1** A seed names one `seeded::Seeded` stream on every target, pinned by reference vectors; `below` is unbiased; never a key source. · why: #344 · enforced by: `seeded::tests`, `tests/it/sim_seeded.rs`
- **INV-HEX-1** `hex::decode_into` needs the exact destination length and validates all input first, so a refusal writes nothing. · enforced by: `tests/it/sim_hex.rs`
- **INV-ENCODING-1** Percent-escape errors use input byte offsets; UTF-8 errors use decoded offsets and never pose as source coordinates. · enforced by: `tests/it/sim_encoding.rs`
- **INV-RANDOM-1** `lgwks_std::random` supports exactly its `getrandom` backend's targets, substitutes nothing and types the cause. · why: #276 · enforced by: `tests/it/sim_random_error.rs`, the `target-matrix` lane
- **INV-ID-1** UUID v4 masks apply only to generated ids; parsing preserves any value, and malformed hex names group and offset. · enforced by: `id::tests`, `tests/it/sim_id.rs`
- **INV-ID-2** Each `Uuid::new_v7` is strictly greater than the last in value and text, across threads and a backwards clock. · why: #715 · enforced by: `id::sim_v7`, `tests/it/sim_id_v7.rs`
- **INV-ID-4** `Uuid::now_v7` never refuses and keeps `new_v7`'s order: a bad clock keeps the last stamp, and past 48 bits it pins. · why: #851, ADR 0010 · enforced by: `id::sim_v7_clamped`
- **INV-ID-5** `Uuid::pack_tuple` appends the FoundationDB UUID element (code `0x30`) in identifier order, and `Element` encodes all twelve codes. · why: #851 · enforced by: `id::sim_tuple`, `tests/it/sim_lgwks_id.rs`
- **INV-STD-BUILD-1** A build digest depends only on tag, root-relative names, bytes, variables and features; a path outside the root is refused. · why: #733 · enforced by: `tests/it/sim_build.rs`
- **INV-LEB128-1** LEB128 decoders accept only minimal encodings and return the consumed length, leaving trailing input to the caller. · enforced by: `tests/it/sim_leb128.rs`
- **INV-STD-HASH-1** A digest is a function of the bytes alone: incremental equals one-shot at every chunking. · enforced by: `tests/it/sim_hash.rs`
- **INV-TIME-1** RFC 3339 parsing bounds offsets and refuses leap seconds; Unix conversion reports range failures, never the epoch. · why: #153 · enforced by: `tests/it/sim_time_profile.rs`
- **INV-STD-LOCALE-1** Human-text readers answer the same under every locale and zone, and `run_ps` pins `TZ=UTC0` and `LC_ALL=C`. · why: #278 · enforced by: `tests/it/sim_locale.rs`
- **INV-STD-UNSAFE-1** `unsafe` lives only in the Darwin and Windows process modules, each call behind a seam Miri drives. · why: #263, #345 · enforced by: `tests/it/unsafe_boundary.rs`, the `miri-unsafe` lane
- **INV-STD-HTTP-1** HTTP failures keep a typed class and stage, redirects are bounded and strip headers across origins, and one deadline bounds a call. · why: #163, #190 · enforced by: `http::tests::redirect_loop_refuses_at_the_configured_limit`
- **INV-STD-HTTP-2** The body ceiling holds one fixed read window per call, so peak heap is independent of the body size. · why: #163 · enforced by: `tests/it/sim_http.rs`, `tests/http_alloc.rs`
- **INV-STD-HTTP-3** An event stream decodes the same however the network splits it. · why: #913 · enforced by: `http::stream::sse_decode::sim_sse`
- **INV-STD-HTTP-4** A per-call user agent is sent on every hop at request level, so it reaches the wire and keeps the pooled connection. · why: #913 · enforced by: `http::tests::pool::a_per_call_user_agent_reaches_the_wire_on_the_pooled_connection`
- **INV-STD-HTTP-5** A `Debug` of `Options` or `Client` names each extra header and never renders its value. · why: #913 · enforced by: `tests/it/sim_http.rs`
- **INV-STD-ONLINE-1** Reachability candidates share one monotonic budget over at most 64 addresses; the result is a TCP heuristic. · why: #163 · enforced by: `online::tests::a_whole_probe_fits_one_budget`
- **INV-GLOB-1** Glob matching is anchored, Unicode-scalar and case-sensitive with segment-bound `*`, and a compiled pattern is shareable. · enforced by: `tests/it/glob_public.rs`
- **INV-STD-SHARED-POLICY** One matcher, evidence policy and retry policy serve 10,000 callers bit-identical to the serial run, in flat memory. · why: #154 · enforced by: `tests/it/sim_shared_policy_tiers.rs`
- **INV-CODEC-1** JSON and RON decoders borrow where they can and never report escaped text as borrowed. · why: #162 · enforced by: `tests/it/sim_codec.rs`
- **INV-STD-JSON-1** `json::Value` keeps strings of up to 22 bytes inline and reads, writes and converts exactly as serde_json does. · why: #831 · enforced by: `tests/it/sim_json.rs`, the `latency-tails` lane
- **INV-WIRE-1** `lgwks_std::wire` is an rkyv facade with an observable format descriptor, not a canonical encoding. · why: #167 · enforced by: `tests/it/sim_wire.rs`
- **INV-PATTERN-SAFE** Checked patterns enforce their ceilings before allocation and never return a partial replacement. · why: #168 · enforced by: `tests/it/sim_pattern.rs`
- **INV-FS-2** A strict walk that succeeds has no known omissions, a tolerant walk returns them, and an unresolved root is refused; a directory cut by `max_depth` through a deep link is walked again through a shallower path. · why: #143, #913 · enforced by: `tests/it/sim_fs_walk.rs`
- **INV-FS-3** Handle-relative access resolves one component under one admitted descriptor and never follows a symlink implicitly. · why: #174 · enforced by: `lgwks_std::fs::capability::tests`
- **INV-FS-4** Created names are owner-only, descriptors close on exec, an inconsistent open request is refused, and `open_entry` opens regular files only without ever blocking on a FIFO or device. · why: #185, #913 · enforced by: `lgwks_std::fs::capability::tests`
- **INV-FS-5** Walk output is rooted at the canonical root; budgets charge before retention and mark prefixes incomplete. · why: #166 · enforced by: `tests/it/sim_fs_walk.rs`
- **INV-FS-6** A capability listing is bounded by `ListLimits`, returns UTF-8 names only, says when it is incomplete, and reads through its own descriptor, so listings at once through clones each see every name. · why: #192, #913 · enforced by: `lgwks_std::fs::capability::tests`
- **INV-FS-7** A predicate walk offers each entry once with its `lstat`; `Skip` withholds an uncharged subtree and `Stop` ends the walk. · why: #343 · enforced by: `tests/it/sim_fs_walk_prune.rs`
- **INV-FS-8** The streaming walk lists every link as `Symlink`, keeps a followed link's name and still lists a directory a link entered first. · why: #945 · enforced by: `lgwks_std::fs::stream::sim_links`
- **INV-TASK-POOL-1** The blocking pool's ceiling is fixed once, shutdown closes admission and joins every thread, and finished threads are reaped. · why: #264, #289 · enforced by: `tests/sim_task_pool_public.rs`, `task::tests`
- **INV-STD-PROCESS-1** A non-reaping waitid judges by `si_code`, so a stopped child never reads as exited. · why: #691 · enforced by: `process::tests`, `a_gated_spawn_holds_its_payload_until_release`
- **INV-STD-PROCESS-2** A tree stop signals every target whatever earlier ones answered, and reports the first refusal last. · why: #913 · enforced by: `process::sim_signal_every`
- **INV-TLS-2** An RSA modulus of up to 8,192 bits is admitted in a certificate and a key through one reader. · why: #913 · enforced by: `http::tls::der::sim_rsa_bounds`
- **INV-LEGACY-1** Each `lgwks_std::legacy` codec round-trips bytes, refuses past its `Limits` before allocating and names the offset of a failure. · why: #792 · enforced by: `tests/it/sim_legacy_cfb.rs`, `tests/it/sim_legacy_fax.rs`
- **INV-LEGACY-2** The legacy codecs are reachable in one line from `script!`. · why: #792 · enforced by: `a_script_flow_calls_the_ebcdic_codec_in_one_line`
