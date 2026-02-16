use anyhow::Result;
use clap::{Parser, Subcommand};

/// P25 trunked radio decoder — RF in, JSON out.
#[derive(Parser)]
#[command(name = "p25", version, about)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

/// Available subcommands.
#[derive(Subcommand)]
enum Command {
    /// Decode a P25 control channel from IQ samples.
    Cc {
        /// Path to an IQ sample file (CF32 format).
        #[arg(short, long)]
        input: String,

        /// Sample rate in Hz.
        #[arg(short, long, default_value_t = 2_400_000)]
        sample_rate: u32,

        /// Center frequency in Hz.
        #[arg(short, long)]
        frequency: u64,
    },
}

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_writer(std::io::stderr)
        .init();

    let cli = Cli::parse();

    match cli.command {
        Command::Cc {
            input,
            sample_rate,
            frequency,
        } => {
            tracing::info!(
                input = %input,
                sample_rate,
                frequency,
                "starting control channel decoder"
            );
            // TODO: wire up SDR source -> DSP pipeline -> protocol decoder -> JSON output
            anyhow::bail!("control channel decoder not yet implemented");
        }
    }
}
