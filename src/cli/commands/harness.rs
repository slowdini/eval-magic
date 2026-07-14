//! The `harness` subcommands: inspect and validate the layered harness
//! descriptor registry (`list`, `show`, `lint`).

use std::fs;
use std::path::Path;

use anyhow::{Context, bail};

use crate::adapters::descriptor::layers::{
    Layer, check_user_layer_restrictions, default_config_root, discover_sources,
};
use crate::adapters::descriptor::{
    HarnessDescriptor, finalize_descriptor, merge_descriptor_value, parse_descriptor_value,
};
use crate::adapters::registry::{HarnessInfo, default_harness_name, harness_info};
use crate::core::Harness;

use crate::cli::args::{HarnessArgs, HarnessCommands};

pub(crate) fn run_harness(args: HarnessArgs) -> anyhow::Result<()> {
    match args.command {
        HarnessCommands::List => run_list(),
        HarnessCommands::Show { name } => run_show(&name),
        HarnessCommands::Lint { target } => run_lint(&target),
    }
}

/// One line per registered harness: label, contributing layers, declared
/// enhancements.
fn run_list() -> anyhow::Result<()> {
    let default_name = default_harness_name();
    let rows: Vec<(String, String, String)> = harness_info()
        .map(|info| {
            let name = if info.label == default_name {
                format!("{} (default)", info.label)
            } else {
                info.label.to_string()
            };
            (
                name,
                layer_chain(&info),
                declared_enhancements(info.descriptor),
            )
        })
        .collect();
    let name_width = rows.iter().map(|(n, _, _)| n.len()).max().unwrap_or(0);
    let layer_width = rows.iter().map(|(_, l, _)| l.len()).max().unwrap_or(0);
    for (name, layers, enhancements) in rows {
        println!("{name:<name_width$}  {layers:<layer_width$}  {enhancements}");
    }
    Ok(())
}

/// Print one harness's resolved (layer-merged) descriptor as authorable TOML,
/// headed by its provenance chain.
fn run_show(name: &str) -> anyhow::Result<()> {
    let Some(info) = harness_info().find(|info| info.label == name) else {
        return Err(Harness::resolve(name)
            .expect_err("name is absent from the registry")
            .into());
    };
    println!("# {name} — resolved descriptor (after layer merging)");
    println!("# sources:");
    for (layer, path) in info.sources {
        println!("#   {path} ({})", layer.display_name());
    }
    println!();
    print!("{}", toml::to_string(info.descriptor)?);
    Ok(())
}

/// Lint a descriptor file, or every discovered layer of a registered name.
fn run_lint(target: &str) -> anyhow::Result<()> {
    let looks_like_path = target.contains(std::path::MAIN_SEPARATOR)
        || target.ends_with(".toml")
        || Path::new(target).is_file();
    if looks_like_path {
        lint_file(Path::new(target))
    } else {
        lint_name(target)
    }
}

/// Run one descriptor file through the full load pipeline, reporting each
/// check as a ✓/✗ line (the `validate` idiom).
fn lint_file(path: &Path) -> anyhow::Result<()> {
    let display = path.display();
    let toml_src = fs::read_to_string(path).with_context(|| format!("cannot read {display}"))?;
    let mut failed = 0usize;

    let value = match parse_descriptor_value(&toml_src, &display.to_string()) {
        Ok(value) => {
            println!("✓ TOML syntax + schema");
            Some(value)
        }
        Err(e) => {
            eprintln!("✗ {e}");
            failed += 1;
            None
        }
    };

    if let Some(value) = &value {
        match check_user_layer_restrictions(value, &display.to_string()) {
            Ok(()) => println!("✓ user-layer restrictions ([guard] stays built-in-only)"),
            Err(e) => {
                eprintln!("✗ {e}");
                failed += 1;
            }
        }

        // Cross-field invariants — merged onto the registered harness with the
        // same label when one exists, so a partial override file is checked
        // against its real merge target. Merging is idempotent, so linting a
        // file the registry already discovered reports identically.
        let label = value.get("label").and_then(serde_json::Value::as_str);
        let target = label.and_then(|l| harness_info().find(|info| info.label == l));
        let (merged, provenance) = match &target {
            Some(info) => {
                let mut merged = info.value.clone();
                merge_descriptor_value(&mut merged, value.clone());
                (
                    merged,
                    format!("{} + {display} (lint)", layer_provenance(info)),
                )
            }
            None => (value.clone(), display.to_string()),
        };
        match finalize_descriptor(&merged, &provenance) {
            Ok(_) => match &target {
                Some(info) => println!("✓ cross-field invariants (merged onto {})", info.label),
                None => println!("✓ cross-field invariants"),
            },
            Err(e) => {
                eprintln!("✗ {e}");
                failed += 1;
            }
        }
    }

    if failed > 0 {
        bail!("descriptor lint failed for {display}: {failed} check(s) failed");
    }
    println!("Linted {display}: all checks passed.");
    Ok(())
}

/// Strictly re-lint every discovered layer file, reporting the ones registry
/// initialization skipped with a warning, and re-validate the named harness's
/// merged chain.
fn lint_name(name: &str) -> anyhow::Result<()> {
    let project_root = std::env::current_dir().unwrap_or_default();
    let (sources, io_warnings) =
        discover_sources(default_config_root().as_deref(), &project_root, None)
            .map_err(anyhow::Error::from)?;
    let mut failed = io_warnings.len();
    for warning in io_warnings {
        eprintln!("✗ {warning}");
    }

    let mut chain: Option<(serde_json::Value, Vec<String>)> = None;
    for source in sources {
        let value = match parse_descriptor_value(&source.toml_src, &source.path) {
            Ok(value) => value,
            Err(e) => {
                // The label is unknowable, so every broken discovered file is
                // reported — these are exactly the files init skipped.
                if source.layer != Layer::Embedded {
                    eprintln!("✗ {e}");
                    failed += 1;
                }
                continue;
            }
        };
        if source.layer != Layer::Embedded
            && let Err(e) = check_user_layer_restrictions(&value, &source.path)
        {
            eprintln!("✗ {e}");
            failed += 1;
            continue;
        }
        if value.get("label").and_then(serde_json::Value::as_str) != Some(name) {
            continue;
        }
        let step = format!("{} ({})", source.path, source.layer.display_name());
        println!("✓ {step}: schema + user-layer checks");
        match &mut chain {
            None => chain = Some((value, vec![step])),
            Some((base, steps)) => {
                let mut merged = base.clone();
                merge_descriptor_value(&mut merged, value);
                steps.push(step);
                let provenance = steps.join(" + ");
                match finalize_descriptor(&merged, &provenance) {
                    Ok(_) => *base = merged,
                    Err(e) => {
                        eprintln!("✗ {e}");
                        failed += 1;
                        steps.pop();
                    }
                }
            }
        }
    }

    match &chain {
        Some((value, steps)) => {
            let provenance = steps.join(" + ");
            match finalize_descriptor(value, &provenance) {
                Ok(_) => println!("✓ resolved descriptor: {provenance}"),
                Err(e) => {
                    eprintln!("✗ {e}");
                    failed += 1;
                }
            }
        }
        // Discovery missed it, but a `--harness-file` may still have
        // registered it for this invocation.
        None => match harness_info().find(|info| info.label == name) {
            Some(info) => println!("✓ registered: {}", layer_provenance(&info)),
            None if failed == 0 => {
                return Err(Harness::resolve(name)
                    .expect_err("name is absent from every layer")
                    .into());
            }
            None => {}
        },
    }

    if failed > 0 {
        bail!("descriptor lint failed for {name}: {failed} check(s) failed");
    }
    println!("Linted {name}: all checks passed.");
    Ok(())
}

/// `built-in + project` — the layer chain for `harness list`.
fn layer_chain(info: &HarnessInfo) -> String {
    info.sources
        .iter()
        .map(|(layer, _)| layer.display_name())
        .collect::<Vec<_>>()
        .join(" + ")
}

/// `harnesses/claude-code.toml (built-in) + …` — the full provenance chain.
fn layer_provenance(info: &HarnessInfo) -> String {
    info.sources
        .iter()
        .map(|(layer, path)| format!("{path} ({})", layer.display_name()))
        .collect::<Vec<_>>()
        .join(" + ")
}

/// The enhancements a resolved descriptor declares, for `harness list`.
fn declared_enhancements(descriptor: &HarnessDescriptor) -> String {
    let mut list: Vec<&str> = Vec::new();
    if descriptor.skills_dir.is_some() {
        list.push("staging");
    }
    if descriptor.skills_block.is_some() {
        list.push("skills-block");
    }
    if descriptor.transcript.is_some() {
        list.push("transcript");
    }
    if descriptor.model.is_some() {
        list.push("model-flag");
    }
    if descriptor.guard.is_some() {
        list.push("guard");
    }
    if descriptor.shadow.is_some() {
        list.push("shadow-preflight");
    }
    if !descriptor.dispatch.is_empty() {
        list.push("dispatch-recipes");
    }
    if list.is_empty() {
        "baseline".to_string()
    } else {
        list.join(", ")
    }
}
