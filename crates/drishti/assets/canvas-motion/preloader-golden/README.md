# preloader-golden conformance suite

Golden checks for `surfaces/landing/src/timelines/landing-preloader.timeline.json`.

## Coverage

| Phase | Time range | Assertions |
|-------|-----------|------------|
| Phase 1: Letter stagger | 0–500ms | 8 checks (node count, from/to values, start_ms for letter-0) |
| Phase 2: Logo reposition | 600–1400ms | 5 checks (existence, from/to, start_ms, end_ms) |
| Phase 3: Chrome reveal | 1200–2000ms | 6 checks (header opacity + clipPath from/to, start_ms) |
| Phase 4: Preloader dismiss | 2200ms | 4 checks (existence, from=1, to=0, start_ms) |
| Document fields | n/a | 4 checks (keys, id, version, labels) |

## Running

```bash
node conformance/runner.mjs preloader-golden
```
