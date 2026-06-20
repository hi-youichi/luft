//! `generate` subcommand: NL → Lua script generation without execution.

use crate::backend;
use crate::GenerateArgs;
use anyhow::Result;

pub async fn generate_script(args: GenerateArgs) -> Result<()> {
    let backend_id = match args.backend.as_deref() {
        Some(id) => id.to_string(),
        None => {
            let detected = backend::detect_backend();
            if detected == "mock" {
                anyhow::bail!(
                    "NL generation requires a real LLM backend. \
                     Install opencode (https://opencode.ai) or specify --backend <id>"
                );
            }
            eprintln!("ℹ  no --backend specified, auto-detected: {}", detected);
            detected.to_string()
        }
    };

    let backend = backend::create_backend(&backend_id, false)?;
    let cfg = maestro::planner::PlannerConfig::default();

    eprintln!("⚙  Generating Lua workflow script…");

    let planned = maestro::planner::plan_workflow(&args.nl, backend, &cfg).await?;

    match args.output {
        Some(path) => {
            std::fs::write(&path, &planned.script)?;
            eprintln!("✅  Written to {}", path.display());
        }
        None => {
            println!("{}", planned.script);
        }
    }

    Ok(())
}
