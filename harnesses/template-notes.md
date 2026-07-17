# {label} — harness implementation notes

> **Audience:** the verification record behind `.eval-magic/harnesses/{label}.toml`. While the
> descriptor is project-local this file lives beside it; when the descriptor is upstreamed this
> file is promoted to `docs/{label}-notes.md` in the eval-magic repo (the harness-descriptor PR
> template requires it). The enhancement model is docs/progressive-enhancements.md.

## Verified against

- Harness CLI + version: <!-- e.g. `{label} 1.4.2` (`{label} --version`) -->
- Date verified: <!-- YYYY-MM-DD -->
- Documentation consulted: <!-- URLs, `{label} --help` sections -->

## Verification record

Every non-comment field in the descriptor gets a row — verify, don't guess: values come from the
harness's own documentation or from output you observed, never by analogy with another harness.

| Descriptor field | Value | Source (docs URL / --help output / observed file) |
|------------------|-------|---------------------------------------------------|
| `label` | `{label}` | chosen name |

## Dispatch quirks

<!-- Observed CLI behavior that shaped [dispatch]: flag interactions, stdin handling, cwd
behavior, how the final message is recovered. -->

## What's wired

<!-- Which enhancements the descriptor declares, and which ride their fallbacks — see
`eval-magic harness list` and the run preflight warnings. -->

## Wiring the next enhancements

<!-- Candidates and open questions, per enhancement — see docs/progressive-enhancements.md. -->
