use anyhow::Result;
use clap::{Parser, Subcommand};
use serde_json::json;
use timeline::{config, history::History};

#[derive(Parser)]
#[command(
    version,
    about = "Local desktop activity recording and evidence inspection"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Offline conversion of legacy images into verified packed regions.
    MigrateImages,
    /// List committed screenshot evidence and missing samples.
    Screenshots {
        #[arg(long, default_value_t = 0)]
        after: i64,
        #[arg(long, default_value_t = 100)]
        limit: usize,
    },
    /// Stream one retained WebP to stdout; unavailable evidence is an error.
    Image { id: i64 },
    /// Run the desktop recorder until interrupted.
    Run,
    /// Inspect recorder state (requires the recorder to be running).
    Status,
    /// Durably pause recording.
    Pause,
    /// Resume recording with fresh evidence.
    Resume,
    /// Inspect committed raw evidence as a bounded JSON page.
    Observations {
        #[arg(long, default_value_t = 0)]
        after: i64,
        #[arg(long, default_value_t = 100)]
        limit: usize,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let directory = config::history_directory()?;
    match cli.command {
        Command::MigrateImages => {
            println!(
                "{}",
                serde_json::to_string(&timeline::migration::migrate(&directory)?)?
            );
        }
        Command::Observations { after, limit } => {
            let history = History::read(&directory)?;
            let observations = history.observations(after, limit)?;
            let next_after = observations.last().map(|o| o.id).unwrap_or(after);
            let has_more = !history.observations(next_after, 1)?.is_empty();
            println!(
                "{}",
                serde_json::to_string_pretty(
                    &json!({"observations": observations, "next_after": next_after, "has_more": has_more})
                )?
            );
        }
        Command::Screenshots { after, limit } => {
            let history = History::read(&directory)?;
            let screenshots = history.screenshots(after, limit)?;
            let next_after = screenshots.last().map(|s| s.id).unwrap_or(after);
            let has_more = !history.screenshots(next_after, 1)?.is_empty();
            println!(
                "{}",
                serde_json::to_string_pretty(
                    &json!({"screenshots":screenshots,"next_after":next_after,"has_more":has_more})
                )?
            );
        }
        Command::Image { id } => {
            let history = History::read(&directory)?;
            let mut image = history
                .image(id)?
                .ok_or_else(|| anyhow::anyhow!("screenshot {id} unavailable or expired"))?;
            std::io::copy(&mut image, &mut std::io::stdout().lock())?;
        }
        Command::Run => timeline::daemon::run(&directory)?,
        command => {
            let command = match command {
                Command::Pause => "pause",
                Command::Resume => "resume",
                _ => "status",
            };
            println!("{}", timeline::daemon::request(&directory, command)?);
        }
    }
    Ok(())
}
