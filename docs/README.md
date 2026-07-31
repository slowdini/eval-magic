# Documentation map and placement policy

> **Audience:** contributors deciding where a piece of documentation belongs. For the docs
> themselves, see the README's Documentation table or run `eval-magic docs`.

Documentation is a first-class feature of eval-magic: an agent or human should be able to use
the installed tool with only what ships in the binary. These rules keep that true as docs grow.

## The three tiers

1. **Shipped CLI docs** — `eval-magic --help` and `eval-magic <cmd> --help`, derived from the
   doc comments in `src/cli/args.rs` plus the worked examples in `src/cli/help.rs`. The primary
   discovery surface; every command and flag is documented here.
2. **Shipped reference topics** — `eval-magic docs <topic>` prints a document embedded in the
   binary (version-matched to the install, readable offline). Topics are registered in
   `src/cli/commands/docs.rs`: `guide` embeds the repository README (the complete operating
   guide) and `byoh` embeds [byoh.md](byoh.md) (authoring a harness descriptor). Generated
   per-run docs (`RUNBOOK.md`, `dispatch-manifest.md`) are the runtime dispatch reference and
   behave like tier 1.
3. **Internal development docs** — everything else in this directory:
   [progressive-enhancements.md](progressive-enhancements.md) (the harness
   baseline/enhancement contract) and the per-harness implementation notes
   ([claude-notes.md](claude-notes.md), [codex-notes.md](codex-notes.md),
   [opencode-notes.md](opencode-notes.md)). Audience: people working on eval-magic itself. Each
   file declares its audience in a header blockquote.

## Placement rules

- **If a user of the installed binary could need it, it goes in tier 1 or 2.** Never answer a
  user-facing question only in a tier-3 file.
- **Shipped output references embedded topics, not repo paths.** Help text, console notes,
  generated artifacts, and scaffolded templates say `eval-magic docs <topic>`. A repo-relative
  `docs/...` path is only honest for tier-3 content, whose audience has the repository.
  `tests/cli/docs.rs` drift-guards that every `eval-magic docs <topic>` mention in help output
  names a real topic.
- **Assume tier-3 paths are unreachable from an install.** The `docs/` files ride along in the
  crate tarball, but installer-script users have only the binary.
- **Distill, don't move.** When a tier-3 file grows something a user would need (a dispatch
  quirk that changes how results read, an operational gotcha), distill it into the README or
  `--help` and leave a pointer in the tier-3 file. The dev doc keeps its implementation detail.
- **New tier-2 topics are a deliberate choice.** Embedding a doc makes it a shipped surface:
  register it in `src/cli/commands/docs.rs`, keep it audience-tagged, and expect the drift
  guard to start covering references to it.

## Hosted documentation

Deferred (see [issue #190](https://github.com/slowdini/eval-magic/issues/190)). GitHub already
renders the README and this directory for pre-install browsing, and the embedded topics cover
post-install use, so a docs site would today add a second published surface to keep in sync for
little gain. Revisit when any of these become true:

- more than ~4 user-facing reference topics — the bare-`docs` listing stops fitting on a
  screen;
- demand for docs versioned per release;
- significant human (non-agent) readership needing search and navigation beyond GitHub's
  rendering.

If that day comes, prefer something low-maintenance that reuses these Markdown files as its
source (e.g. GitHub Pages over this directory) rather than a parallel doc set.
