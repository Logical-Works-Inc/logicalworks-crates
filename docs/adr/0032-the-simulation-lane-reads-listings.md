# ADR 0032: the simulation lane reads listings, not archives

Status: **accepted**, 2026-11-12. The Director's orders were "WHY THE FUCK R UR RUNS TAKING SO
LONG" and "sub 5 minute CI".

Stakeholders: the Director; a contributor whose pull request runs the Linux bot-it shards.
Concerns: a shard cancelled at its 5-minute cap with every test passing, because an upload ran first.
Requirements: R13
Viewpoint: the gate viewpoint, one row per CI step and the artifact it uploads.
Elements and relations: the `tests_linux` listing and upload steps, the `workspace_build` waiter and
download, the `simulation-listing` and `simulation-listings-ready` lanes of `scripts/gate-lanes.toml`,
and the `simulation-evidence` lane in `ci_local/runners.rs`.

## Context

The `simulation-evidence` lane proves at least half the tests are deterministic simulations. It
counts tests from nextest listings. To avoid compiling the workspace again, the first shard of each
suite ran `cargo nextest archive` and uploaded the compiled suite. The lane then extracted each
archive only to run `cargo nextest list` on it.

In run 43228739672 (#931) every bot-it shard uploaded its archive before its tests: 2, 2, 30, 40,
45, 96, 105 and 177 s. bot-it-g5 spent 177 s uploading, then was cancelled at 300 s on test 303 of
325 with every test so far passing.

## Decision

The first shard of each suite writes `cargo nextest list --message-format json` of the build it
already made to `/tmp/nextest-listing/listing.json` and uploads only that, as
`nextest-listing-<archive>-Linux`, kept one day. `SIMULATION_LISTINGS` names the downloaded files
and the lane reads them as JSON. With nothing named it lists the workspace from source, as before.

## Alternatives rejected

- **The archive at compression level 0.** It skips the zip pass but still moves every compiled
  test binary through the artifact service once per suite.
- **The simulation job building the workspace itself.** Cold, that alone was cancelled at 293 s
  (run 41627777207).

## Consequences

- The lane counts exactly what it counted before, from the same listing JSON, and extracts nothing.
- A shard's upload is one JSON file, so its tests start once its build ends: 1 s to list and
  1 s to upload on run 43234727204.
- The listing is written outside the checkout. In the checkout, the authority-claims guard read
  its test names as revocation claims and failed bot-it-g3 (run 43234727204).
- An unreadable listing is the lane's I/O error naming the file; a listing that is not JSON is a
  refusal naming it.

## Correspondence

Refines the simulation lane of ADR 0028's CI layout and the `simulation-evidence` lane
`docs/workflow/ci-layout.md` describes.

## Verification

- `sim_lane_split::the_simulation_lane_reads_listings_not_archives` refuses a workflow or lane
  table that runs an archive again. It checks that every suite the waiter names is one listing
  the lane reads, and that the upload keeps one-day retention.
- `runners::tests::named_listings_are_read_as_files_and_only_none_lists_the_workspace` holds the
  fallback.
- The `gate-parity` lane holds the `simulation-listing` and `simulation-listings-ready` lanes to
  `ci.yml`.
