<!--
Harness-descriptor contribution PR — data-only.
Use for: adding a new built-in harness descriptor (or extending one) that only
reuses AVAILABLE named capabilities. Summary/denial readers, slug capabilities, shadow preflights,
and guard support that require code are CODE contributions — one capability per PR,
separately from this one (see docs/progressive-enhancements.md "Guardrails").
Guide: docs/guides/byoh.md "Upstreaming your descriptor".
Title: feat(harness): add <label> descriptor
-->

## Harness

- **Label:** `<label>`
- **Harness CLI + version verified against:** <!-- e.g. `cool-cli 1.4.2` -->
- **What it is / docs link:**

## Scope check (data-only)

- [ ] This PR adds/changes descriptor **data only** — no summary/denial reader,
      slug, shadow, or guard code (those are separate one-capability-per-PR contributions)
- [ ] `[transcript]`/`[shadow]`/`[staging]` (if declared) reuse an existing named
      capability (`claude-stream-json` / `codex-items` / `opencode-events` / `opencode` /
      `claude-plugins` / `codex-skills` / `opencode-skills`)
      or ingest through `[transcript.extract]` blocks (declarative summary/session-surface data)
- [ ] No `[guard]` table and no `run.supports_guard = true` in this PR

## The diff (expected shape)

- [ ] `harnesses/<label>.toml` — the descriptor
- [ ] `src/adapters/descriptor.rs` — the `EMBEDDED_DESCRIPTORS` entry (mechanical
      registration only)
- [ ] `docs/<label>-notes.md` — verified implementation notes (promote your
      project's `.eval-magic/harnesses/<label>-notes.md` — `harness init`
      scaffolds it)
- [ ] `eval-magic harness list` and `harness show <label>` report only the
      enhancements declared by the descriptor

## Verification evidence

- [ ] Lint is clean — paste the output:

  ```
  $ eval-magic harness lint harnesses/<label>.toml
  ```

- [ ] A smoke eval ran end-to-end on this harness
      (`run --harness <label>` → dispatch → `ingest` → `finalize`); describe or
      link the result:
- [ ] Every declared enhancement was exercised by that run (e.g. staged skill
      discovered natively, events file read by the configured transcript
      capabilities, model flag appeared in the generated recipes)

## Don't-guess attestation

Every flag, filename, and event shape in the descriptor is traced to the
harness's own documentation or to output I observed — nothing is inferred by
analogy with another harness. `docs/<label>-notes.md` records, per value: the
value, its source (docs URL / `--help` output / observed events file), and the
harness version.

- [ ] The notes file's Verification record covers every non-comment field in
      the descriptor

## Related

Issues/PRs:
