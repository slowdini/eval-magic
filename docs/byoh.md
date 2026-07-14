# Bring your own harness

> **Audience:** anyone (human or agent) pointing eval-magic at a harness it has never seen. No Rust
> required — a harness declares its capabilities in a TOML descriptor file, and every capability it
> *doesn't* declare degrades to a documented fallback with a warning, never a rejection. The
> baseline-vs-enhancement contract behind this is
> [progressive-enhancements.md](progressive-enhancements.md).

## The five-minute version

From inside a project that dispatches through `cool-custom-harness`:

```sh
mkdir -p .eval-magic/harnesses
cat > .eval-magic/harnesses/cool-custom-harness.toml <<'EOF'
label = "cool-custom-harness"

[dispatch]
exec_template = '''
cool-cli run --cd <eval-root>{model_arg} \
  "Read the file at <dispatch_prompt_path> and follow its instructions exactly." \
  > <outputs_dir>/final-message.md'''
EOF

eval-magic harness lint .eval-magic/harnesses/cool-custom-harness.toml
eval-magic harness list
eval-magic run --harness cool-custom-harness
```

That run is complete: `run` warns about each enhancement the descriptor doesn't declare (naming the
fallback that carries it), builds `dispatch.json` / `RUNBOOK.md` / `dispatch-manifest.md` with your
exec recipe, and after you dispatch the tasks, `ingest` assembles records from each task's
`outputs/final-message.md`, runs the `detect-stray-writes` audit, and hands grading to `llm_judge`.

For a one-off descriptor that shouldn't live in the project, pass it directly — its label becomes
the invocation's default harness, so `--harness` can be omitted:

```sh
eval-magic run --harness-file ./cool-custom-harness.toml
```

## The minimum viable descriptor

`label` alone loads:

```toml
label = "cool-custom-harness"
```

That is a **baseline** harness: staging is unavailable (runs fall back to `--no-stage`, inlining
each `SKILL.md` into its dispatch prompt), and dispatch guidance is generic. Add an
`[dispatch] exec_template` and the generated `RUNBOOK.md` / `dispatch-manifest.md` carry a
copy-pasteable per-task recipe instead.

### The exec-template contract

`exec_template` is the one-shot command that dispatches a single task. eval-magic substitutes
`{model_arg}` (the harness's model flag + requested model, empty unless `[model].flag` is declared)
and `{guard_args}` (guard-only arguments, built-ins only); the `<eval-root>`,
`<dispatch_prompt_path>`, and `<outputs_dir>` placeholders are filled per task by whoever dispatches
(the runbook explains this). Two hard requirements:

1. **Run from the task's `eval_root`** — each task's per-`(group, condition)` env dir is the
   subprocess cwd.
2. **Recover the final message** — the agent's final reply must land in
   `outputs/final-message.md`. If your CLI can't write it directly (like the example above
   redirecting stdout), capture it yourself before running `ingest`.

## Layering and field-level merge

Descriptor files stack in precedence order; **later layers override individual fields** of an
earlier descriptor with the same `label` (tables merge key-by-key; scalars and arrays replace
wholesale — never whole-file shadowing). A new `label` defines a new harness.

1. **Embedded built-ins** — `claude-code`, `codex`, `opencode` (bundled in the binary).
2. **User-global** — `<config-root>/harnesses/*.toml`, where the config root is
   `$EVAL_MAGIC_CONFIG_DIR` (empty value disables the layer), else `$XDG_CONFIG_HOME/eval-magic`,
   else `~/.config/eval-magic`.
3. **Project-local** — `<cwd>/.eval-magic/harnesses/*.toml`.
4. **`--harness-file <path>`** — a one-off top layer; when `--harness` is omitted its label is the
   invocation's default harness.

Files within one directory apply in filename order; one file per label per layer (a duplicate is
skipped with a warning). So a project can retune a single built-in field without restating the
descriptor:

```toml
# .eval-magic/harnesses/claude-code.toml — override one field of a built-in
label = "claude-code"

[model]
flag = "--model-x"
```

`eval-magic harness show claude-code` prints the resolved post-merge descriptor (as authorable
TOML, headed by the contributing files) — the fastest way to see what a layer actually changed.
Two caveats: the merge can't *delete* (an override can replace a field's value but cannot remove
an embedded table), and a guarded built-in's output isn't copy-paste-safe as a user layer until
you drop its `[guard]` table and `run.supports_guard` (see the guard restriction below).

**Broken discovered files never brick the CLI** — they are skipped with a warning pointing at
`eval-magic harness lint <file>`. A broken `--harness-file` is a hard error (you named it
explicitly).

## Field reference

The authoritative field-by-field reference is the bundled schema,
[`schema/harness-descriptor.schema.json`](../schema/harness-descriptor.schema.json); the semantics
and fallbacks per enhancement are in
[progressive-enhancements.md](progressive-enhancements.md). The short map:

| Table | Declares | Fallback when absent |
|-------|----------|----------------------|
| (top level) | `label` (required), `skills_dir`, `config_dirs` | no `skills_dir` ⇒ forced `--no-stage`, SKILL.md inlined |
| `[dispatch]` | exec/parallel/judge/next-steps/manifest templates | generic handoff text; with only `exec_template`, generic recipes are built around it |
| `[transcript]` | `events_filename` + `parser` (a named capability) | `transcript_check` grades unverifiable, `llm_judge` carries grading, tokens/duration unrecorded |
| `[model]` | `flag` | `--agent-model`/`--judge-model` recorded as provenance only |
| `[staging]` + `[skills_block]` | slug/naming rules, skills-block format | `--no-stage` inlining |
| `[tools]` | tool-name vocabulary by role | required alongside `[transcript]` (the stray-writes audit classifies by it) |
| `[shadow]` | `preflight` (named capability) | no shadow report — correct for harnesses that load nothing global |
| `[guard]` | **built-ins only** — see below | `detect-stray-writes` audits after the fact |

### Named capabilities: real code for free

Everything that is genuinely code — transcript stitching, guard hooks, slug sanitization, shadow
scanning — is a **named capability** a descriptor references. If your harness emits a compatible
stream, you get the full feature from configuration alone:

- `transcript.parser = "claude-stream-json"` — Claude Code `-p --output-format stream-json` events.
- `transcript.parser = "codex-items"` — Codex `item.started`/`item.completed` JSONL.
- `staging.slug_capability = "opencode"` — OpenCode's sanitizing slug rules.
- `shadow.preflight = "claude-plugins"` — the Claude plugin/global-skills shadow scan.

For example, a harness that logs Codex-compatible item JSONL gets full transcript ingest — parsed
tool invocations, `transcript_check` grading, the works — with:

```toml
[tools]
write = ["file_change"]
shell = ["command_execution"]

[transcript]
events_filename = "cool-events.jsonl"
parser = "codex-items"
```

An unknown capability name fails the schema gate listing the allowed values.

### The guard restriction

User-supplied descriptors may **not** declare `[guard]` or set `run.supports_guard = true`: the
write guard installs native hook config into dispatch environments and stays restricted to
built-in descriptors until the guard engine is opened up (fail-open safety). Unguarded runs fall
back to the `detect-stray-writes` audit. A project-local overlay of a guarded built-in is fine —
the restriction applies to the user file's own content, and the embedded guard merges through
underneath it.

## The workflow

```sh
eval-magic harness lint <file>   # full per-file report: TOML + schema, user-layer
                                 # restrictions, cross-field invariants (merged onto the
                                 # registered harness with the same label, if any)
eval-magic harness lint <name>   # strictly re-lint every discovered layer of a name —
                                 # surfaces files that registry init skipped with a warning
eval-magic harness list          # every registered harness: layers + declared enhancements
eval-magic harness show <name>   # the resolved post-merge descriptor as authorable TOML
```

Then `run --harness <name>` and read the warnings: each one names the fallback carrying that part
of the run, which doubles as your wiring roadmap — declare the enhancement and the warning
disappears. (A `harness lint --probe` live dispatch check — rendering the exec template with a
trivial prompt and verifying final-message recovery — is planned; see the tracking issue.)
