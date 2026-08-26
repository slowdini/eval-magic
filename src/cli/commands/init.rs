//! `init` -- scaffold a first `evals/evals.json` for a skill.

use std::fs;
use std::io::{self, Write};
use std::path::{Component, Path, PathBuf};

use anyhow::{anyhow, bail};
use serde_json::{Value, json};

use crate::cli::args::InitArgs;
use crate::cli::command_target_args;
use crate::core::{DetectInput, detect_run_context};
use crate::validation::validate_evals_config;

const DEFAULT_CODEBASE_URL: &str = "https://github.com/slowdini/eval-magic-fixture";
const DEFAULT_CODEBASE_REF: &str = "b6d269c1cdedf7cadb53bacc41acaf5f2cdbe03f";

/// Create `<skill>/evals/evals.json` with one seed eval and print next steps.
pub(crate) fn run_init(args: InitArgs) -> anyhow::Result<()> {
    let ctx = detect_run_context(DetectInput {
        skill_dir: args.skill_dir.clone(),
        skill: args.skill.clone(),
        ..Default::default()
    })?;

    let evals_path = ctx.skill_subdir.join("evals").join("evals.json");

    if evals_path.exists() && !args.force {
        bail!(
            "evals.json already exists: {}\n  Pass --force to overwrite it.",
            evals_path.display()
        );
    }

    let codebase = resolve_codebase(&args, &ctx.stage_root, &evals_path)?;

    let id = value_or_prompt(args.id, "--id", "Eval id")?;
    let prompt = value_or_prompt(args.prompt, "--prompt", "Prompt")?;
    let expected_output =
        value_or_prompt(args.expected_output, "--expected-output", "Expected output")?;

    let document = scaffold_json(
        &ctx.skill_name,
        &id,
        &prompt,
        &expected_output,
        args.skill_should_trigger,
        codebase,
    );
    validate_evals_config(&document, &evals_path.to_string_lossy())?;

    if let Some(parent) = evals_path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(
        &evals_path,
        format!("{}\n", serde_json::to_string_pretty(&document)?),
    )?;

    println!(
        "Initialized evals for {} -> {}",
        ctx.skill_name,
        evals_path.display()
    );
    println!();
    println!("Next:");
    let target_args = command_target_args(&ctx);
    println!("  eval-magic run{}", target_args);
    println!("Then follow the generated RUNBOOK.md.");

    Ok(())
}

fn resolve_codebase(
    args: &InitArgs,
    invocation_cwd: &Path,
    evals_path: &Path,
) -> anyhow::Result<Value> {
    if let (Some(url), Some(reference)) = (&args.codebase_url, &args.codebase_ref) {
        return Ok(json!({ "url": url, "ref": reference }));
    }

    let (candidate, preserve_absolute, option_name) = if args.codebase_cwd {
        (invocation_cwd.to_path_buf(), false, "--codebase-cwd")
    } else if let Some(raw) = &args.codebase_path {
        let path = Path::new(raw);
        (
            if path.is_absolute() {
                path.to_path_buf()
            } else {
                invocation_cwd.join(path)
            },
            path.is_absolute(),
            "--codebase-path",
        )
    } else {
        return Ok(json!({
            "url": DEFAULT_CODEBASE_URL,
            "ref": DEFAULT_CODEBASE_REF,
        }));
    };

    let resolved = crate::core::fs::real_path(&candidate)?;
    if !resolved.is_dir() {
        bail!("{option_name} is not a directory: {}", candidate.display());
    }

    let rendered = if preserve_absolute {
        crate::core::fs::artifact_path(&resolved)
    } else {
        let evals_dir = evals_path
            .parent()
            .ok_or_else(|| anyhow!("generated eval path has no parent"))?;
        let evals_dir = crate::core::fs::real_path(evals_dir)?;
        crate::core::fs::artifact_path(&relative_path(&evals_dir, &resolved)?)
    };

    Ok(json!({ "path": rendered }))
}

fn relative_path(from: &Path, to: &Path) -> anyhow::Result<PathBuf> {
    let from_components: Vec<_> = from.components().collect();
    let to_components: Vec<_> = to.components().collect();
    let common = from_components
        .iter()
        .zip(&to_components)
        .take_while(|(left, right)| left == right)
        .count();

    if common == 0 {
        bail!(
            "cannot render codebase path {} relative to {}",
            to.display(),
            from.display()
        );
    }

    let mut relative = PathBuf::new();
    for component in &from_components[common..] {
        match component {
            Component::Normal(_) => relative.push(".."),
            _ => bail!("cannot render codebase path relative to generated evals directory"),
        }
    }
    for component in &to_components[common..] {
        match component {
            Component::Normal(value) => relative.push(value),
            _ => bail!("cannot render codebase path relative to generated evals directory"),
        }
    }
    if relative.as_os_str().is_empty() {
        relative.push(".");
    }
    Ok(relative)
}

fn value_or_prompt(value: Option<String>, flag: &str, label: &str) -> anyhow::Result<String> {
    match value {
        Some(value) => Ok(value),
        None => prompt_for(flag, label),
    }
}

fn prompt_for(flag: &str, label: &str) -> anyhow::Result<String> {
    print!("{label}: ");
    io::stdout().flush()?;

    let mut line = String::new();
    let bytes = io::stdin().read_line(&mut line)?;
    let value = line.trim().to_string();
    if bytes == 0 || value.is_empty() {
        return Err(anyhow!("missing required init field {flag}"));
    }
    Ok(value)
}

fn scaffold_json(
    skill_name: &str,
    id: &str,
    prompt: &str,
    expected_output: &str,
    skill_should_trigger: Option<bool>,
    codebase: Value,
) -> Value {
    let mut eval = json!({
        "id": id,
        "prompt": prompt,
        "expected_output": expected_output,
    });
    if skill_should_trigger == Some(false) {
        eval["skill_should_trigger"] = json!(false);
    }

    json!({
        "skill_name": skill_name,
        "codebase": codebase,
        "evals": [eval],
    })
}

#[cfg(test)]
mod tests {
    use super::scaffold_json;
    use serde_json::json;

    #[test]
    fn scaffold_omits_default_skill_should_trigger() {
        let doc = scaffold_json(
            "demo",
            "e1",
            "prompt",
            "output",
            Some(true),
            json!({ "path": "." }),
        );

        assert_eq!(
            doc,
            json!({
                "skill_name": "demo",
                "codebase": { "path": "." },
                "evals": [
                    {
                        "id": "e1",
                        "prompt": "prompt",
                        "expected_output": "output"
                    }
                ]
            })
        );
    }

    #[test]
    fn scaffold_writes_false_skill_should_trigger() {
        let doc = scaffold_json(
            "demo",
            "e1",
            "prompt",
            "output",
            Some(false),
            json!({ "path": "." }),
        );

        assert_eq!(doc["evals"][0]["skill_should_trigger"], false);
    }
}
