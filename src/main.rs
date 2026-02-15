mod cli;
mod dsp;
#[allow(unused)]
mod output;
pub mod p25;
mod sdr;
pub mod util;

use anyhow::Result;
use clap::Parser;
use tracing_subscriber::EnvFilter;

fn main() -> Result<()> {
    let cli = cli::Cli::parse();

    match cli.command {
        cli::Command::Cc(args) => {
            // Initialize tracing based on verbosity
            let filter = if args.verbose {
                EnvFilter::new("debug")
            } else {
                EnvFilter::from_default_env()
            };
            tracing_subscriber::fmt().with_env_filter(filter).init();

            eprintln!("p25 cc: control channel decoder");

            if let Some(ref file) = args.file {
                eprintln!("  input: {}", file.display());
            }
            if let Some(freq) = args.frequency {
                eprintln!("  frequency: {:.0} Hz", freq);
            }
            if let Some(ref device) = args.device {
                eprintln!("  device: {device}");
            }

            eprintln!();
            eprintln!("SDR and DSP pipeline not yet implemented.");
            eprintln!("TSBK parser and JSON output are available as library modules.");
            eprintln!("Run `cargo test` to verify the protocol decoder.");
        }
    }

    Ok(())
}
