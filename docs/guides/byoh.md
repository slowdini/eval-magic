# Bring your own harness

> **Audience:** operators adapting eval-magic to an agent CLI that has no built-in descriptor.
> You do not need to write Rust for a baseline harness.

Use this guide when `claude-code`, `cline`, `codex`, and `opencode` are not the CLI you need, or when a
project must override part of a built-in harness. Normal eval authoring and run options belong in
`eval-magic init --help` and `eval-magic run --help`.

## Scaffold and run a harness

From the project that will use the harness:

```sh
eval-magic harness init cool-custom-harness
eval-magic harness lint .eval-magic/harnesses/cool-custom-harness.toml
eval-magic harness list
eval-magic run --harness cool-custom-harness
```

`harness init` writes two files:

- `.eval-magic/harnesses/cool-custom-harness.toml` is a commented descriptor with only `label`
  enabled.
- `.eval-magic/harnesses/cool-custom-harness-notes.md` records the source and harness version for
  every value you enable.

The label-only descriptor is usable. It falls back to `--no-stage`, inlines each `SKILL.md`, uses
`llm_judge` and runner-owned assertions for grading, and audits writes after dispatch. Read the
warnings from `run`; each warning names the lower-fidelity fallback carrying an undeclared
capability.

For a descriptor that should not live in the project, pass it directly:

```sh
eval-magic run --harness-file ./cool-custom-harness.toml
```

Its `label` becomes the default harness for that invocation.

## Add a dispatch command first

The highest-leverage field is `[dispatch].exec_template`. It is the command `eval-magic dispatch`
spawns for every task, so without it there is nothing for the runner to run.

```toml
label = "cool-custom-harness"

[dispatch]
exec_template = '''
cool-cli run --cd <eval-root>{model_arg} \
  "Read the file at <dispatch_prompt_path> and follow its instructions exactly." \
  > <outputs_dir>/final-message.md'''
```

The command has two requirements:

1. Run the agent from the supplied `<eval-root>`. Each condition and repetition owns a private task
   repository there.
2. Recover the final reply at `<outputs_dir>/final-message.md`. Redirect stdout or copy the native
   output there when the CLI cannot write the file itself.

Prove both requirements before a real eval:

```sh
eval-magic harness lint .eval-magic/harnesses/cool-custom-harness.toml --probe
```

The probe renders the real command, asks for confirmation, invokes the harness CLI in a temporary
directory, and checks that the final-message file is nonempty. It can spend tokens and use network
services. Static lint runs first, and non-interactive use defaults to no; `--yes` explicitly accepts
the dispatch and `--probe-timeout SECONDS` bounds it.

## Use the generated field reference

The scaffold is the installed field reference:

```sh
eval-magic harness init example --stdout
```

It explains dispatch templates, environment defaults, transcript ingest, conversations, staging,
model selection, tool vocabularies, and shadow preflight beside the fields themselves. Use these
commands while filling it in:

- `eval-magic harness show claude-code` shows a descriptor backed by a named transcript parser.
- `eval-magic harness show codex` shows declarative extraction for a flat JSONL stream.
- `eval-magic harness show <name>` shows the result after descriptor layers merge.
- `eval-magic harness lint <name-or-file>` validates schema rules and cross-field contracts.
- `eval-magic harness list` shows the source layers and enhancements for every resolved harness.

The scaffold and resolved descriptor output are the installed references. Repository contributors
can trace the underlying schema and adapter contracts from `docs/developer_overview.md` in a source
checkout.

## Layer descriptors by field

Descriptors load in this order:

1. Embedded built-ins.
2. User-global files under `$EVAL_MAGIC_CONFIG_DIR/harnesses/`, otherwise
   `$XDG_CONFIG_HOME/eval-magic/harnesses/` or `~/.config/eval-magic/harnesses/`.
3. Project files under `<cwd>/.eval-magic/harnesses/`.
4. The file named by `--harness-file`.

A later descriptor with the same `label` overrides individual fields; tables merge by key, while
scalars and arrays replace the earlier value. A different label registers another harness. For
example, a project can override one built-in model flag without copying the rest of its descriptor:

```toml
label = "claude-code"

[model]
flag = "--model-x"
```

`harness show claude-code` prints the resolved descriptor and its contributing layers. A layer can
replace a value but cannot delete an inherited table. When copying a guarded built-in into a user
layer, remove `[guard]` and `run.supports_guard`; user-supplied descriptors cannot declare guard
support because a malformed guard can fail open.

Discovered files that fail to load are skipped with a warning so they do not brick unrelated CLI
commands. An invalid `--harness-file` is fatal because the invocation selected it explicitly.

## Verify instead of guessing

Every enabled flag, filename, event shape, and environment behavior must come from the harness's
documentation or output you observed. Record the value, evidence, and harness version in the notes
file. Do not copy a capability from another harness because its events look similar.

Use this sequence:

1. Run `harness lint` after each descriptor edit.
2. Run `harness lint --probe` after adding or changing the dispatch template.
3. Run a small eval through `run`, dispatch, `ingest`, and `finalize`.
4. Confirm that every declared enhancement was exercised by the smoke run.

Multi-turn evals — scripted `turns` and `responder` alike — require
`[conversation].resume_exec_template` plus transcript extraction of ordered assistant messages and
the native session ID. There is no fresh-session fallback: `run` rejects the case when the harness
cannot preserve the conversation. The responder itself needs nothing further from a descriptor; it
reads the agent's message out of the transcript and consults its own model through the dispatch
template you already declared, so it works on any harness that can resume. See
`eval-magic docs conversations`.

When a shadow preflight reports a live copy, isolate every initial and resumed eval-agent dispatch
before setting `isolates_live_sources = true`. The per-harness remedies and verification procedure
are in `eval-magic docs isolation`.

## Upstreaming your descriptor

A proven local descriptor can become a built-in. Descriptor data may declare command templates,
environment defaults, model flags, staging rules, tools, declarative transcript extraction, and
references to named capabilities that already exist. A new transcript reader, denial reader, slug
algorithm, shadow scan, or guard mechanism is a separate code contribution.

Open a data-only contribution with the
[harness descriptor PR template](https://github.com/slowdini/eval-magic/blob/dev/.github/PULL_REQUEST_TEMPLATE/harness-descriptor.md).
It contains:

1. `harnesses/<label>.toml` with the proven descriptor.
2. The mechanical registration in `src/adapters/descriptor.rs`.
3. `docs/<label>-notes.md` with evidence for every enabled value.

Attach clean lint output, the harness version, and an end-to-end smoke-eval result. The descriptor
registry and `eval-magic harness list` are the capability source of truth.
