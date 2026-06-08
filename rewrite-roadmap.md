# eval-magic rewrite roadmap

The plan for porting [`@slowdini/eval-runner`](https://github.com/slowdini/eval-runner)
(TypeScript/npm) to `eval-magic` (Rust), from skeleton through to sunsetting the
TypeScript repo.

## Why

- **Performance** — the runner is invoked many times per eval run; keep the
  mechanical work fast and lean.
- **Portability** — ship a dependency-less prebuilt binary, runnable without npm.

The timing is right: the TS project is functional, has strong test coverage
(~271 cases across ~20 files), and just finished a refactor into seven clean
submodules. That gives us (a) a TDD harness — port the TS tests for a module,
watch them fail, make them green — and (b) a natural unit of work: one submodule
at a time. The rewrite is also where we pay down TS code-quality debt (notably
`run.ts` at ~1,593 LOC) instead of refactoring it in TS first.

## Approach

- **One module per phase**, ordered by dependency and risk: foundation first,
  the `run` orchestrator last.
- **Test-first**: each phase ports the corresponding TS tests before the
  implementation, so a red→green transition validates the port.
- **Fixture parity**: every pipeline subcommand is standalone JSON-in/JSON-out,
  so the Rust binary can be validated subcommand-by-subcommand against the same
  fixtures the TS suite uses.
- **Refactor while porting**: split over-long TS files (`run.ts`, `grade.ts`)
  into focused units as they land in Rust.

## Module map (eval-runner → eval-magic)

| TS submodule | ~LOC | Rust module | Responsibility |
|---|---|---|---|
| `core/` | 521 | `core` | Domain types, `RunContext`, runtime/path helpers |
| `validation/` | 144 | `validation` | JSON-Schema validation of evals.json (AJV → `jsonschema`) |
| `adapters/` | 784 | `adapters` | Harness session rendering + transcript parsing |
| `sandbox/` | 388 | `sandbox` | Write-guard install/teardown + write-boundary policy |
| `pipeline/` | 1,809 | `pipeline` | The six-stage post-dispatch processing chain |
| `workspace/` | 336 | `workspace` | Baseline promotion + workspace cleanup |
| `cli/` | 1,038 | `cli` | Subcommand dispatch + `run` orchestration |

## Phases

### Phase 0 — Foundation ✅ (this session)
- Single-crate skeleton (lib + `skill-eval` bin) with all seven modules stubbed.
- clap command tree mirroring eval-runner's subcommands; handlers report "not
  yet implemented" and a smoke test pins the surface.
- Dependencies chosen (see below). CI, git hooks, lint/format, and the
  version-bump release-PR workflow in place. Binary distribution deferred.

### Phase 1 — `core`
Domain types + `RunContext` + runtime/path helpers. Everything else depends on
it, so it goes first. Port `context.test.ts`, `runtime.test.ts`. Establishes the
serde-modeled types (`Eval`, `EvalsConfig`, `RunRecord`, `Assertion`,
`GradingResult`, `ToolInvocation`, …) that every later phase consumes.

### Phase 2 — `validation`
Smallest self-contained module (~144 LOC); proves the `jsonschema` +
bundled-`schema/*.json` approach end-to-end. Port `validate.test.ts`,
`validate-schema.test.ts`. Wires up the `validate` subcommand for real.

### Phase 3 — `adapters`
Pure transcript-parsing and session-rendering functions — well-tested, no
orchestration, low risk. Port the five adapter tests (Claude Code & Codex
session + transcript, plugin-shadow).

### Phase 4 — `sandbox`
Write-boundary classification (`sandbox-policy`) and guard install/teardown.
Port `policy.test.ts`, `install.test.ts`. Wires up `teardown-guard`. Note: the
TS `guard.ts` runs as a Node hook script invoked by path — decide during this
phase how the Rust binary exposes the equivalent guard entry point.

### Phase 5 — `pipeline`
The six stages, in chain order: `record-runs` → `fill-transcripts` →
`detect-stray-writes` → `grade` → `aggregate`. Largest body (~1,809 LOC) but
each stage is an independent JSON-in/JSON-out subcommand — port and validate one
stage at a time against shared fixtures. Decompose `grade.ts` (~616 LOC) into
focused units (transcript-check grading vs. judge-task emission vs. finalize)
while porting it.

### Phase 6 — `workspace`
Baseline promotion and workspace teardown. Port `promote-baseline.test.ts`,
`workspace-teardown.test.ts`. Wires up `snapshot`, `teardown`, `promote-baseline`.

### Phase 7 — `cli` / `run`
Subcommand dispatch is already scaffolded; this phase ports the `run`
orchestrator and the `ingest`/`finalize` run-modes. Decompose `run.ts`
(~1,593 LOC) into focused sub-orchestrators: skill staging, dispatch generation,
subagent coordination, guard arming, and the run-mode variants. This is the
highest-complexity phase and the main code-quality win.

### Phase 8 — Cutover & sunset
- Validate the Rust binary subcommand-by-subcommand against the TS fixtures
  until at parity.
- Set up **cargo-dist** (`dist init`) to build cross-platform binaries
  (macOS/Linux/Windows) and attach them + a shell/PowerShell installer to the
  GitHub Release, replacing the stubbed step in `.github/workflows/release.yml`.
  Optionally add `cargo install` / crates.io as a secondary channel.
- Switch the shipped artifact to the Rust binary, then deprecate and sunset the
  `@slowdini/eval-runner` npm package.

## Dependencies

Chosen in Phase 0, kept lean and justified:

| Crate | Role |
|---|---|
| `clap` (derive) | CLI parsing + generated help (replaces manual flags + `help.ts`) |
| `serde` + `serde_json` | (de)serialization; `preserve_order` keeps JSON key order stable/diffable vs. TS |
| `jsonschema` (no default features, `resolve-file` only) | Schema validation (replaces AJV); HTTP/TLS resolver stack dropped |
| `anyhow` | Error propagation in the binary / command handlers |
| `thiserror` | Typed error enums inside library modules |
| `walkdir` | Recursive discovery of `evals.json` files |
| `tempfile` (dev) | Fixture temp dirs (replaces TS `tmpdir()` pattern) |
| `assert_cmd` + `predicates` (dev) | Subprocess integration tests (replaces `Bun.spawnSync`) |
| `cargo-husky` (dev) | Installs git hooks on `cargo test` |

Subprocess/git spawning and path handling use `std::process::Command` /
`std::path` — no crate needed, matching the TS approach of shelling out.

### Deferred dependency decisions

Revisit each when the phase that forces it arrives:

- **LLM / HTTP client** — *likely none.* The `grade` stage only *emits*
  judge-task JSON; it does not call an API. Confirm in Phase 5; add a client only
  if a stage actually performs network I/O.
- **Snapshot testing (`insta`)** — decide in Phase 5 when porting the pipeline,
  where output artifacts are large and snapshot assertions may beat hand-written
  `serde_json` comparisons.
- **Colored terminal output** — `clap`/`anstream` already provide basic styling;
  add a dedicated crate only if richer output is needed (likely Phase 7).
