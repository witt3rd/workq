//! animus CLI — operator interface to the animus appliance.

use animus_rs::config::Config;
use animus_rs::db::Db;
use animus_rs::engine::{ControlConfig, ControlPlane};
use animus_rs::faculty::FacultyRegistry;
use animus_rs::model::work::{NewWorkItem, State};
use animus_rs::telemetry::{TelemetryConfig, init_telemetry};
use clap::{Parser, Subcommand};
use secrecy::ExposeSecret;
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Parser)]
#[command(name = "animus", about = "Substrate for relational beings")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Run the control plane daemon
    Serve {
        /// Directory containing faculty TOML configs
        #[arg(long, default_value = "faculties")]
        faculties: PathBuf,
        /// Global maximum concurrent foci
        #[arg(long, default_value_t = 4)]
        max_concurrent: usize,
    },
    /// Work item operations
    Work {
        #[command(subcommand)]
        action: WorkAction,
    },
}

#[derive(Subcommand)]
enum WorkAction {
    /// Submit a new work item
    Submit {
        /// Work type (determines faculty routing)
        work_type: String,
        /// Provenance source
        source: String,
        /// Structural dedup key
        #[arg(long)]
        dedup_key: Option<String>,
        /// Provenance trigger info
        #[arg(long)]
        trigger: Option<String>,
        /// JSON parameters
        #[arg(long)]
        params: Option<String>,
        /// Priority (higher = more urgent)
        #[arg(long, default_value_t = 0)]
        priority: i32,
    },
    /// List work items
    List {
        /// Filter by state
        #[arg(long)]
        state: Option<String>,
        /// Filter by work type
        #[arg(long, name = "type")]
        work_type: Option<String>,
        /// Maximum items to show
        #[arg(long, default_value_t = 20)]
        limit: i64,
    },
    /// Show a work item
    Show {
        /// Work item ID (full UUID or prefix)
        id: String,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();
    let cli = Cli::parse();

    match cli.command {
        Command::Serve {
            faculties,
            max_concurrent,
        } => cmd_serve(faculties, max_concurrent).await,
        Command::Work { action } => {
            let config = Config::from_env()?;
            let db = Db::connect(config.database_url.expose_secret()).await?;
            db.migrate().await?;
            db.create_queue("work").await?;

            match action {
                WorkAction::Submit {
                    work_type,
                    source,
                    dedup_key,
                    trigger,
                    params,
                    priority,
                } => {
                    cmd_work_submit(&db, work_type, source, dedup_key, trigger, params, priority)
                        .await
                }
                WorkAction::List {
                    state,
                    work_type,
                    limit,
                } => cmd_work_list(&db, state, work_type, limit).await,
                WorkAction::Show { id } => cmd_work_show(&db, id).await,
            }
        }
    }
}

async fn cmd_serve(faculties: PathBuf, max_concurrent: usize) -> anyhow::Result<()> {
    let config = Config::from_env()?;

    let _guard = init_telemetry(TelemetryConfig {
        endpoint: config.otel_endpoint.clone(),
        service_name: "animus".to_string(),
    })?;

    let db = Db::connect(config.database_url.expose_secret()).await?;
    db.migrate().await?;
    db.create_queue("work").await?;

    let registry = FacultyRegistry::load_from_dir(&faculties)?;

    let control = ControlPlane::new(
        Arc::new(db),
        Arc::new(registry),
        ControlConfig::default(),
        max_concurrent,
    );

    let ctrl = control.clone();
    tokio::spawn(async move {
        tokio::signal::ctrl_c().await.ok();
        ctrl.shutdown();
    });

    control.run().await?;
    Ok(())
}

async fn cmd_work_submit(
    db: &Db,
    work_type: String,
    source: String,
    dedup_key: Option<String>,
    trigger: Option<String>,
    params: Option<String>,
    priority: i32,
) -> anyhow::Result<()> {
    let params: serde_json::Value = match params {
        Some(json) => serde_json::from_str(&json)?,
        None => serde_json::json!({}),
    };

    let mut new = NewWorkItem::new(&work_type, &source)
        .params(params)
        .priority(priority);

    if let Some(ref key) = dedup_key {
        new = new.dedup_key(key);
    }
    if let Some(ref trig) = trigger {
        new = new.trigger(trig);
    }

    let result = db.submit_work(new).await?;

    match result {
        animus_rs::db::work::SubmitResult::Created(item) => {
            println!("Created: {} (state: {})", item.id, item.state);
        }
        animus_rs::db::work::SubmitResult::Merged {
            new_id,
            canonical_id,
        } => {
            println!("Merged: {new_id} → canonical {canonical_id}");
        }
    }

    Ok(())
}

async fn cmd_work_list(
    db: &Db,
    state: Option<String>,
    work_type: Option<String>,
    limit: i64,
) -> anyhow::Result<()> {
    let state_filter: Option<State> = match state {
        Some(s) => Some(
            s.parse()
                .map_err(|_| anyhow::anyhow!("invalid state: {s}"))?,
        ),
        None => None,
    };

    let items = db
        .list_work_items(state_filter, work_type.as_deref(), limit)
        .await?;

    if items.is_empty() {
        println!("No work items found.");
        return Ok(());
    }

    // Header
    println!(
        "{:<8}  {:<12}  {:<10}  {:<4}  {:<30}  CREATED",
        "ID", "TYPE", "STATE", "PRI", "DEDUP_KEY"
    );
    println!("{}", "-".repeat(100));

    for item in &items {
        let short_id = &item.id.to_string()[..8];
        let dedup = item.dedup_key.as_deref().unwrap_or("-");
        let dedup_display = if dedup.len() > 30 {
            &dedup[..30]
        } else {
            dedup
        };
        println!(
            "{:<8}  {:<12}  {:<10}  {:<4}  {:<30}  {}",
            short_id,
            item.work_type,
            item.state,
            item.priority,
            dedup_display,
            item.created_at.format("%Y-%m-%d %H:%M")
        );
    }

    println!("\n{} item(s)", items.len());
    Ok(())
}

async fn cmd_work_show(db: &Db, id_str: String) -> anyhow::Result<()> {
    // Support prefix matching — find the work item whose ID starts with the given string
    let id = if id_str.len() < 36 {
        // Prefix search
        let items = db.list_work_items(None, None, 100).await?;
        let matches: Vec<_> = items
            .iter()
            .filter(|item| item.id.to_string().starts_with(&id_str))
            .collect();
        match matches.len() {
            0 => anyhow::bail!("no work item matching prefix '{id_str}'"),
            1 => matches[0].id,
            n => anyhow::bail!("{n} work items match prefix '{id_str}' — be more specific"),
        }
    } else {
        let uuid = uuid::Uuid::parse_str(&id_str)?;
        animus_rs::model::work::WorkId(uuid)
    };

    let item = db.get_work_item(id).await?;

    println!("ID:         {}", item.id);
    println!("Type:       {}", item.work_type);
    println!("State:      {}", item.state);
    println!("Priority:   {}", item.priority);
    println!("Dedup Key:  {}", item.dedup_key.as_deref().unwrap_or("-"));
    println!("Source:     {}", item.provenance.source);
    println!(
        "Trigger:    {}",
        item.provenance.trigger.as_deref().unwrap_or("-")
    );
    println!(
        "Params:     {}",
        serde_json::to_string_pretty(&item.params)?
    );
    println!("Attempts:   {}", item.attempts);
    println!(
        "Max Tries:  {}",
        item.max_attempts
            .map(|n| n.to_string())
            .unwrap_or("-".to_string())
    );
    println!("Created:    {}", item.created_at);
    println!("Updated:    {}", item.updated_at);
    if let Some(resolved) = item.resolved_at {
        println!("Resolved:   {resolved}");
    }
    if let Some(parent) = item.parent_id {
        println!("Parent:     {parent}");
    }
    if let Some(merged) = item.merged_into {
        println!("Merged Into: {merged}");
    }
    if let Some(ref outcome) = item.outcome {
        println!("---");
        println!(
            "Outcome:    {}",
            if outcome.success {
                "success"
            } else {
                "failure"
            }
        );
        if let Some(ref data) = outcome.data {
            println!("Data:       {}", serde_json::to_string_pretty(data)?);
        }
        if let Some(ref err) = outcome.error {
            println!("Error:      {err}");
        }
        println!("Duration:   {}ms", outcome.duration_ms);
    }

    Ok(())
}
