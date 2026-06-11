//! `init` -- scaffold a first `evals/evals.json` for a skill.

use std::fs;
use std::io::{self, Write};

use anyhow::{anyhow, bail};
use serde_json::{Value, json};

use crate::cli::args::InitArgs;
use crate::core::{DetectInput, detect_run_context};
use crate::validation::validate_evals_config;

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
    println!(
        "  eval-magic run --skill-dir {} --skill {} --mode new-skill --guard",
        ctx.skill_dir.display(),
        ctx.skill_name
    );
    println!(
        "  eval-magic ingest --skill-dir {} --skill {} --iteration 1 --subagents-dir <subagents-dir>",
        ctx.skill_dir.display(),
        ctx.skill_name
    );
    println!(
        "  eval-magic finalize --skill-dir {} --skill {} --iteration 1",
        ctx.skill_dir.display(),
        ctx.skill_name
    );
    println!(
        "  eval-magic promote-baseline --skill-dir {} --skill {} --iteration 1",
        ctx.skill_dir.display(),
        ctx.skill_name
    );

    Ok(())
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
        "evals": [eval],
    })
}

#[cfg(test)]
mod tests {
    use super::scaffold_json;
    use serde_json::json;

    #[test]
    fn scaffold_omits_default_skill_should_trigger() {
        let doc = scaffold_json("demo", "e1", "prompt", "output", Some(true));

        assert_eq!(
            doc,
            json!({
                "skill_name": "demo",
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
        let doc = scaffold_json("demo", "e1", "prompt", "output", Some(false));

        assert_eq!(doc["evals"][0]["skill_should_trigger"], false);
    }
}
