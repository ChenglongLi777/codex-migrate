use crate::model::ImportOptions;
use crate::operations;
use crate::path_mapper::{normalize, parse_mapping};
use anyhow::Result;
use clap::{Args, Parser, Subcommand};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

#[derive(Debug, Parser)]
#[command(name = "codex-migrate", version, about)]
pub struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Subcommand)]
enum Commands {
    Doctor(HomeArgs),
    Export(ExportArgs),
    Scan(ScanArgs),
    Import(ImportArgs),
    Rebind(RebindArgs),
    ExportHtml(ExportHtmlArgs),
    Verify(HomeArgs),
    Rollback(RollbackArgs),
}

#[derive(Debug, Args)]
struct HomeArgs {
    #[arg(long)]
    codex_home: Option<PathBuf>,
}

#[derive(Debug, Args)]
struct ExportArgs {
    source: PathBuf,
    #[arg(long)]
    output_parent: PathBuf,
}

#[derive(Debug, Args)]
struct ScanArgs {
    source: PathBuf,
    #[arg(long)]
    codex_home: Option<PathBuf>,
}

#[derive(Debug, Args)]
struct ImportArgs {
    source: PathBuf,
    #[arg(long)]
    codex_home: Option<PathBuf>,
    #[arg(long)]
    dry_run: bool,
    #[arg(long = "thread")]
    threads: Vec<String>,
    #[arg(long = "exclude-thread")]
    excluded_threads: Vec<String>,
    #[arg(long = "map", value_name = "OLD=NEW")]
    mappings: Vec<String>,
    #[arg(long = "history-only", value_name = "OLD")]
    history_only: Vec<String>,
}

#[derive(Debug, Args)]
struct RollbackArgs {
    transaction_id: String,
    #[arg(long)]
    codex_home: Option<PathBuf>,
}

#[derive(Debug, Args)]
struct RebindArgs {
    #[arg(long)]
    codex_home: Option<PathBuf>,
    #[arg(long = "thread")]
    threads: Vec<String>,
    #[arg(long = "map", value_name = "OLD=NEW")]
    mappings: Vec<String>,
    #[arg(long)]
    dry_run: bool,
}

#[derive(Debug, Args)]
struct ExportHtmlArgs {
    #[arg(long)]
    codex_home: Option<PathBuf>,
    #[arg(long = "thread")]
    threads: Vec<String>,
    #[arg(long)]
    output: Option<PathBuf>,
}

pub fn run(cli: Cli) -> Result<()> {
    match cli.command {
        Commands::Doctor(args) => {
            let report = operations::diagnose(args.codex_home.as_deref())?;
            println!("{}", serde_json::to_string_pretty(&report)?);
            if !report.issues.is_empty() {
                anyhow::bail!("doctor found {} issue(s)", report.issues.len());
            }
        }
        Commands::Export(args) => {
            let summary = operations::export_directory(&args.source, &args.output_parent, |_| {})?;
            println!(
                "backed up the complete Codex directory with {} session file(s) to {}",
                summary.thread_count, summary.output
            );
        }
        Commands::Scan(args) => {
            let catalog = operations::scan_source(&args.source)?;
            let mut result = serde_json::to_value(catalog)?;
            if let Some(target) = args.codex_home {
                result["target_codex_home"] =
                    serde_json::Value::String(target.to_string_lossy().into_owned());
            }
            println!("{}", serde_json::to_string_pretty(&result)?);
        }
        Commands::Import(args) => {
            let catalog = operations::scan_source(&args.source)?;
            let options = build_options(&catalog, &args)?;
            let plan = operations::plan_directory_import(
                &args.source,
                args.codex_home.as_deref(),
                &options,
            )?;
            println!("{}", serde_json::to_string_pretty(&plan)?);
            if !args.dry_run {
                let summary = operations::import_directory(
                    &args.source,
                    args.codex_home.as_deref(),
                    &options,
                    |_| {},
                )?;
                println!(
                    "import completed; transaction id: {}",
                    summary.transaction_id
                );
            }
        }
        Commands::Rebind(args) => {
            let catalog = operations::scan_local(args.codex_home.as_deref())?;
            let all_ids = catalog
                .projects
                .iter()
                .flat_map(|project| project.sessions.iter())
                .map(|session| session.thread.id.clone())
                .collect::<BTreeSet<_>>();
            let options = ImportOptions {
                selected_thread_ids: if args.threads.is_empty() {
                    all_ids
                } else {
                    args.threads.into_iter().collect()
                },
                mappings: args
                    .mappings
                    .iter()
                    .map(|value| parse_mapping(value))
                    .collect::<Result<Vec<_>>>()?
                    .into_iter()
                    .map(|mapping| (mapping.source, mapping.target))
                    .collect(),
                history_only_projects: BTreeSet::new(),
            };
            let environment = crate::discovery::discover(args.codex_home.as_deref())?;
            let plan = operations::plan_directory_import(
                &environment.codex_home,
                Some(&environment.codex_home),
                &options,
            )?;
            println!("{}", serde_json::to_string_pretty(&plan)?);
            if !args.dry_run {
                let summary =
                    operations::rebind_existing(args.codex_home.as_deref(), &options, |_| {})?;
                println!("{}", serde_json::to_string_pretty(&summary)?);
            }
        }
        Commands::ExportHtml(args) => {
            let catalog = operations::scan_local(args.codex_home.as_deref())?;
            let selected = if args.threads.is_empty() {
                catalog
                    .projects
                    .iter()
                    .flat_map(|project| project.sessions.iter())
                    .map(|session| session.thread.id.clone())
                    .collect()
            } else {
                args.threads.into_iter().collect()
            };
            let summary = operations::export_html(
                args.codex_home.as_deref(),
                &selected,
                args.output.as_deref(),
            )?;
            println!("{}", serde_json::to_string_pretty(&summary)?);
        }
        Commands::Verify(args) => {
            let report = operations::verify(args.codex_home.as_deref())?;
            println!("{}", serde_json::to_string_pretty(&report)?);
        }
        Commands::Rollback(args) => {
            operations::rollback(args.codex_home.as_deref(), &args.transaction_id)?;
            println!("rolled back transaction {}", args.transaction_id);
        }
    }
    Ok(())
}

fn build_options(
    catalog: &crate::model::SourceCatalog,
    args: &ImportArgs,
) -> Result<ImportOptions> {
    let all_ids = catalog
        .projects
        .iter()
        .flat_map(|project| project.sessions.iter())
        .map(|session| session.thread.id.clone())
        .collect::<BTreeSet<_>>();
    let mut selected = if args.threads.is_empty() {
        all_ids
    } else {
        args.threads.iter().cloned().collect()
    };
    for excluded in &args.excluded_threads {
        selected.remove(excluded);
    }
    let mappings = args
        .mappings
        .iter()
        .map(|value| parse_mapping(value))
        .collect::<Result<Vec<_>>>()?
        .into_iter()
        .map(|mapping| (mapping.source, mapping.target))
        .collect::<BTreeMap<_, _>>();
    let history_only_projects = args
        .history_only
        .iter()
        .map(|path| normalize(path))
        .collect();
    Ok(ImportOptions {
        selected_thread_ids: selected,
        mappings,
        history_only_projects,
    })
}
