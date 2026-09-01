# Configuring guarded commands

> **Audience:** eval authors deciding which development commands an agent may run inside a task
> environment.

The write guard combines a fixed containment boundary with an eval-authored command policy. Use
the `guard` field in the `evals.json` file to grant the tools or command prefixes a task needs.
Without an explicit `guard` field, eval-magic detects packaged profiles from the staged task tree.

## Understand the two policy layers

Containment checks run before command allowances. A command policy cannot override these checks.
A command that violates both layers is denied once, with a verdict that names each blocking reason,
so the denial never points at a fix that cannot unblock the command:

- Direct write and patch tools must target the task environment.
- Shell redirects and `tee` targets must resolve inside the task environment.
- Remote Git mutations and repository-routing escapes are blocked.
- Recognized package, build, and edit destinations must stay inside the task environment. Global
  and user installation modes are blocked unless the classifier can prove an in-task destination.

After those checks, the command policy handles recognized development mutations. A recognized
command that has no matching allowance is blocked. A tool claimed by `allow_commands` also has its
other subcommands blocked. Commands that neither the containment classifier nor the command policy
recognizes remain best-effort allowed and are inspected by `detect-stray-writes` after the run.

## Choose the configuration scope

A config-level `guard` field is the default for every eval:

```json
{
  "skill_name": "rust-maintainer",
  "guard": {
    "allow_tools": ["cargo"]
  },
  "evals": [
    {
      "id": "repair-workspace",
      "prompt": "Repair the workspace and run its tests.",
      "expected_output": "The workspace tests pass."
    }
  ]
}
```

A per-eval `guard` field completely replaces the config-level field. It does not merge with it.
Even an empty object disables the config default and automatic profile detection for that eval:

```json
{
  "skill_name": "web-maintainer",
  "guard": {
    "profiles": ["language/javascript"]
  },
  "evals": [
    {
      "id": "serve-next-app",
      "prompt": "Start the development server.",
      "expected_output": "The server starts.",
      "guard": {
        "allow_commands": ["npm run dev"]
      }
    }
  ]
}
```

Any explicit `guard` field disables automatic detection. When it names `profiles`, only those
profiles are expanded.

## Choose allowance granularity

The `guard` object accepts three arrays:

- `allow_tools` contains executable basenames. An entry such as `cargo` permits all Cargo
  subcommands after containment checks.
- `allow_commands` contains one literal shell command per entry. Each entry is a token prefix, so
  `cargo test` permits `cargo test --workspace` but not `cargo build`.
- `profiles` contains packaged profile IDs. Profile commands are added to `allow_commands`.

Command rules cannot contain pipes, separators, redirects, variable expansions, or command
substitutions. Eval validation rejects those shapes. Matching normalizes an executable path to its
basename, skips leading environment assignments, and understands the `env`, `command`, `exec`,
`nice`, and `timeout` wrappers. A literal command passed through `sh -c`, `bash -c`, or `zsh -c` is
matched recursively. Every segment of a compound command must be allowed independently.

For example, this policy permits the listed Next.js lifecycle scripts but claims no other npm
subcommands:

```json
{
  "guard": {
    "allow_commands": [
      "npm run dev",
      "npm run build",
      "npm run start"
    ]
  }
}
```

With that policy, `npm run dev -- --hostname 127.0.0.1` is allowed and `npm install` is blocked.
Use `allow_tools` only when every subcommand of the tool is appropriate for the eval.

## Use packaged profiles

Packaged profiles provide lightweight defaults. Detection is recursive, so a frontend and backend
in the same task environment can activate multiple profiles. The detector skips `.git`,
`.eval-magic-outputs`, harness configuration and staged-skill directories, `target`,
`node_modules`, and `.venv`.

The packaged profiles are:

- `language/rust` is detected from `Cargo.toml`. It allows `cargo build`, `check`, `test`, `run`,
  `fmt`, and `clippy`.
- `language/javascript` is detected from `package.json`. It allows npm install, CI, test, build,
  lint, and typecheck commands, plus corresponding pnpm, Yarn, and Bun install, add, test, build,
  lint, and typecheck commands.
- `framework/nextjs` is detected when `package.json` declares a `next` dependency. It allows npm,
  pnpm, Yarn, and Bun dev, build, and start scripts, plus direct `next` invocations through `npx`,
  `pnpm exec`, `yarn`, and `bunx`.
- `language/python` is detected from `pyproject.toml`, `setup.py`, or a
  `requirements*.txt` file. It allows pip installs, Python module invocations for pip, build,
  pytest, and unittest, and direct `pytest`.

Name profiles explicitly when the files in the task tree are not the policy you want:

```json
{
  "guard": {
    "profiles": ["language/javascript", "framework/nextjs"],
    "allow_commands": ["npm run integration"]
  }
}
```

Explicit commands and expanded profile commands are deduplicated in the effective policy.

## Audit the effective policy

Each task in `dispatch.json` records its fully expanded `guard_policy`. The armed marker records the
same policy as `guardPolicy`, so the live hook and the campaign plan cannot resolve defaults
differently. `detect-stray-writes` reads the frozen task policy from `dispatch.json` and applies the
same classifier after the run. A legacy dispatch without `guard_policy` uses an empty command
policy rather than guessing which defaults applied.

The command policy is not a complete shell sandbox. Keep task environments isolated, inspect guard
denials and stray-write findings during ingest, and use narrow `allow_commands` entries when the
eval does not need every operation a tool exposes.
