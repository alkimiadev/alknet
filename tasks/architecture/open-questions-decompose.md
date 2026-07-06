---
id: architecture/open-questions-decompose
name: Decompose open-questions.md into per-OQ files under questions/
status: completed
depends_on: []
scope: moderate
risk: low
impact: project
level: implementation
---

## Description

The single `docs/architecture/open-questions.md` file had grown to 1310 lines
across 47 OQs — large enough to be unmanageable, with high size variance
(OQ-42 at 220 lines next to OQ-06 at 8 lines). Decomposed into one file per
OQ under `docs/architecture/questions/` (`NNN-slug.md`, mirroring the ADR
convention), with `open-questions.md` retained as the index (theme-grouped
tables + a cross-theme "Deferred / Blocked" section for safe-exit visibility).

## What changed

- Created `docs/architecture/questions/` with 47 files (`001-...md` through
  `047-...md`), content moved verbatim from the old monolithic file (heading
  rewritten from `### OQ-NN` to `# OQ-NN` for standalone files).
- Rewrote `docs/architecture/open-questions.md` as the index:
  - Status-values / door-type definitions preserved
  - 15 theme sections, each with a table: `| OQ | Title | Status | Door | Pri |`
  - A "Deferred / Blocked" cross-theme section surfacing the 6 deferred OQs
    with their `Blocked on:` conditions inline (the safe-exit visibility
    surface)
- Updated `docs/architecture/README.md`: dropped the 65-line curated OQ summary
  section (replaced by the index tables), kept the doc-table row pointing at
  `open-questions.md`.
- Original monolithic file preserved at `/tmp/opencode/open-questions.old.md`
  for verification.

## Verification

- `python3 /tmp/opencode/verify_oqs.py` confirms all 47 per-OQ files match the
  original content byte-for-byte (modulo the `### OQ` → `# OQ` heading rewrite).
- All 62 inbound links to `open-questions.md` across ~30 files remain valid
  (none used `#oq-NN` anchors — they all point at the bare filename, which is
  unchanged).

## Out of scope

- Re-resolving or editing any OQ content — verbatim move only.
- Fixing OQ-09/OQ-10's missing `Blocked on:` field — tracked as
  `architecture/oq-09-10-blocking-conditions`.
- Establishing the `tasks/architecture/` blocker-task convention — tracked as
  `architecture/safe-exit-blocker-task-mechanism`.
- Touching reviews/research docs that cite `open-questions.md` by line number
  (those citations are already stale; reviews are historical artifacts).