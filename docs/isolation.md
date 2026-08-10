# Isolating dispatches from live skill sources

> **Audience:** anyone whose run reported a live copy of a staged eval skill — the *skill-shadow*
> banner on stderr, a finding in `plugin-shadow.json`, or a `benchmark.json` validity warning. This
> is the per-harness how-to for making each dispatch stop discovering that copy, and for proving it
> worked. The comparison this protects is described in `eval-magic docs guide`; the descriptor field
> that records the result is in `eval-magic docs byoh`. Printed from the binary via
> `eval-magic docs isolation`, this guide's relative links resolve in the
> [eval-magic repository](https://github.com/slowdini/eval-magic).

## Why a live copy breaks the comparison

An eval measures a delta: the same task run with the skill and without it. That only means something
if the control arm really is skill-free.

Staging gives each dispatch its own `.claude/skills/` (or harness equivalent), and the staging slug
keeps the staged copy from colliding with anything on disk. But a slug prevents an *on-disk*
collision, not *runtime discovery*. An installed plugin or a global skill directory that ships a
same-named skill is discoverable from **both** arms — so `without_skill` isn't skill-absent, and
the delta measures nothing.

How eval-magic grades that risk:

| Finding | Severity | Why |
| --- | --- | --- |
| the skill under test has a live copy | comparison invalid | both arms can resolve it; the delta is meaningless |
| a sibling skill has a live copy, in both arms of every group | warning | symmetric, so the delta stays comparable — but both arms differ from a clean environment |
| a sibling skill has a live copy in only one arm of some group | comparison invalid | asymmetric contamination is indistinguishable from the effect you're measuring |

> Not to be confused with the `could not verify task Git isolation` warning, which is about a task
> environment's Git state, not about skill discovery. Different problem, different fix.

## What eval-magic detects, and what it cannot verify

| Question | Answered by | When |
| --- | --- | --- |
| *Could* a live copy be discovered from this environment? | the skill-shadow banner + `plugin-shadow.json` | before dispatch |
| *Did this dispatch actually load one?* | `resolved_severity` in `plugin-shadow.json`, from each dispatch's own transcript | after `ingest` |
| Does the remedy I applied actually work? | nothing — eval-magic never reads your command templates or environment | never |

That last row is the one that surprises people. The preflight scans your operator config; it does
**not** parse the shell templates in your harness descriptor. If you added `--setting-sources
project,local` by hand, eval-magic cannot see it, so it keeps reporting the finding. That is
deliberate: a `sh -c` wrapper, a flag built from an environment variable, or a shim would defeat any
such parse, and a wrong suppression would hide real contamination — a much worse failure than a
finding you can dismiss.

So the loop is: eval-magic tells you a live copy *exists*, you isolate it, and `ingest` confirms from
each dispatch's transcript whether it actually loaded — on harnesses whose transcripts report that.
Where they don't (Codex and OpenCode today), the middle row stays unanswerable and the finding
remains unverified.

## Recipes by harness

One rule spans every remedy below: **it has to be on every eval-agent dispatch, including every
resumed turn.** An eval with scripted `turns[]` re-enters the harness once per turn through a
separate template. Isolating the first dispatch and not the resumed ones silently re-contaminates
the control arm from turn 2 onward. Judge dispatches are exempt — a judge gets a rubric and a
transcript, and having the skill available cannot tell it the answer.

### Claude Code

| Remedy | Hides | Does *not* hide | Caveat |
| --- | --- | --- | --- |
| `--setting-sources project,local` | everything user-scope: `enabledPlugins`, and skills under `~/.claude/skills` | project/local settings, which is the point — staged skills and the write-guard hook keep loading | auth is unaffected |
| `"enabledPlugins": { "<plugin>@<marketplace>": false }` in a settings source the dispatch loads | that one named plugin | every other plugin, and all global skills | must be in a scope the dispatch actually loads — check it against your `--setting-sources` |
| `CLAUDE_CONFIG_DIR="$(mktemp -d)"` | everything: installed plugins and global skills | nothing | OAuth credentials live in `~/.claude.json`, which a relocated config dir may not carry — set `ANTHROPIC_API_KEY` or authenticate once in the fresh dir |

`--setting-sources project,local` is usually the right first move: it is one flag, it needs no
change to your global config, and it leaves other sessions alone.

Whichever you pick, **your staged skills still load.** They are project-local, independent of
installed plugins, and the `__skill_invoked` meta-check still resolves the staged slug under all
three remedies. Isolating the dispatch does not disarm the eval.

### Codex

| Remedy | Hides | Does *not* hide | Caveat |
| --- | --- | --- | --- |
| `--disable plugins` | skills from enabled installed plugins | skills in repository, user, or admin directories | it is a **global** option — place it before `exec`, e.g. `codex --disable plugins --ask-for-approval never exec …` |
| move or rename the conflicting skill directory | that one source | anything else | the only remedy for a repository or admin skill |
| a clean `HOME` | `$HOME/.agents/skills` | plugins stored under `CODEX_HOME`, and repository/admin skills | preserve `CODEX_HOME` if the dispatch still needs your existing Codex configuration |

**Known limit:** Codex also ships bundled system skills but exposes no stable way to enumerate them,
so the preflight cannot detect a collision with one. If your skill name might collide with a bundled
system skill, check that by hand.

### OpenCode

A skill installed for Claude Code or Codex is visible to OpenCode by default — `~/.claude/skills`
and `~/.agents/skills` are among the roots OpenCode loads. This cross-harness vector is the one most
people don't expect.

| Remedy | Hides | Does *not* hide | Caveat |
| --- | --- | --- | --- |
| `OPENCODE_DISABLE_CLAUDE_CODE_SKILLS=1` | the `.claude` roots, global and project | `.agents` and `.opencode` roots | — |
| `OPENCODE_DISABLE_EXTERNAL_SKILLS=1` | both `.claude` and `.agents` roots | `.opencode` roots | — |
| move or rename the conflicting skill directory | that one source | anything else | the only remedy for an `.opencode` root |

The generated dispatch recipes never set these switches for you. That is deliberate: a recipe should
match what a real user session does, and silently changing the environment would make the eval
measure something other than reality.

**Known limits:** OpenCode also loads skills from config-declared `skills.paths`, from remote
`skills.urls`, and from a singular `.opencode/skill/` directory. The preflight does not scan those.
Check them by hand when relevant.

## Declaring `[shadow] isolates_live_sources` honestly

Once you have isolated every reported source, record it in your harness descriptor. This is the
sanctioned way to tell eval-magic the findings are provenance rather than defects:

```toml
label = "claude-code"

[shadow]
isolates_live_sources = true
```

A layer that only sets this field inherits its `preflight` from the descriptor it merges onto, so an
overlay stays this short. What changes: detection still runs and still writes every source to
`plugin-shadow.json`, along with the assertion itself, so the run stays auditable; `run` prints an
informational notice instead of a warning; and `aggregate` omits the shadow validity warnings.

eval-magic never inspects your command templates, so it cannot tell in advance that the assertion
holds. Where transcripts report a roster it does the next best thing: `ingest` checks the assertion
against what the dispatches actually loaded, and a contradiction is reported rather than trusted —
declaring it falsely gets you a louder warning, not a quieter one. Where they don't, the assertion is
taken on trust. Either way, keeping it honest is yours. **Do not declare it when:**

- any reported source is still discoverable — partial isolation does not qualify;
- you have not applied the remedy to the resumed turns of a scripted eval;
- on Codex, you used `--disable plugins` but the report also names a direct skill source, which that
  flag does not cover;
- on OpenCode, you used either `OPENCODE_DISABLE_*` switch but the report names an `.opencode`
  source, which neither switch hides;
- you have not actually verified it. Which brings us to:

## Verifying isolation

### Read eval-magic's own verdict first

Claude Code's `--output-format stream-json` opens each dispatch with a
`{"type":"system","subtype":"init"}` event listing what the session really loaded. `ingest` reads it
for every dispatch and every resumed turn, records the result in `session-surface.json`, and resolves
each finding in `plugin-shadow.json`:

```bash
jq -r '.findings[] | "\(.skill_name): \(.resolved_severity // "not verified")"' \
  <iteration_dir>/plugin-shadow.json
```

- **`isolated`** — every expected cell reported, and none saw the source. The finding drops out of
  `benchmark.json`'s `validity_warnings`; it stays in `plugin-shadow.json` as provenance.
- **`comparison invalid` / `warning`** — either a dispatch actually loaded it, or it could not be
  settled. `.findings[].sources[].verification` carries the per-cell detail, including an
  `inconclusive_reason` when presence and absence were indistinguishable.
- **absent** — never verified: this harness's transcripts carry no roster, or `ingest` has not run.

Refuting requires *every* expected cell to have reported and none to have seen the source.
Confirming needs only one dispatch that did. A missing transcript never refutes — it leaves the
finding unverified, so a reporting gap can't be mistaken for isolation.

### Read the raw init event yourself

The same ground truth, if you want to check it directly:

```bash
jq 'select(.subtype == "init") | {plugins, skills}' <outputs_dir>/claude-events.jsonl
```

`"plugins": []` means no plugin loaded. To check one specific source, match its runtime id — a
plugin skill is advertised as `<plugin-name>:<skill>`, which is exactly the `runtime_id` recorded
for it in `plugin-shadow.json`:

```bash
jq -r 'select(.subtype == "init") | .skills[]' <outputs_dir>/claude-events.jsonl \
  | grep -cx 'slow-powers:hardening-plans'
```

`0` means that skill was not discoverable from that dispatch. Two caveats:

- **List length proves nothing.** `skills` also contains your staged copies (under their staging
  slugs) and Claude Code's own bundled skills. Only the absence of the specific runtime id is
  evidence.
- **Check a resumed turn too.** Each `turn-N/` events file carries its own `init` event, so a
  scripted eval gives you one verdict per turn — use it, since resumed turns are exactly where an
  under-applied remedy hides.

Codex and OpenCode transcripts carry no equivalent roster, so there is nothing to read and no verdict
to check — their findings stay unverified, and `[shadow] isolates_live_sources` is how you record
that you isolated them. For those, verify by confirming the flag or environment variable appears in
every rendered eval-agent command in `RUNBOOK.md` and `dispatch-manifest.md`.

### `claude plugin list` does not answer this

It reports what is *installed*, not what a given dispatch *loaded*. It takes no
`--setting-sources`, and it will happily show a plugin as resolving even from a fresh temp directory
outside your repository. A plugin can be listed there and absent from a dispatch, or the reverse.
Use the dispatch's own `init` event.

## When you can't isolate

Sometimes the source is out of reach: a Codex bundled system skill, an OpenCode `skills.urls` entry,
or a recipe you don't control. In that case **do not declare the assertion.** Keep the validity
warning — it is the honest record that the run has a known threat to its validity.

For a *sibling* collision that is symmetric across both arms, the delta is still comparable; read it
knowing both arms differ from a clean environment. For a *subject* collision, the iteration is
comparison-invalid and no amount of interpretation fixes it — isolate the source and re-run.
