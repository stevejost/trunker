//! CLI argument definitions using `clap` derive macros.
//!
//! The primary command is `p25 cc` for control channel decoding.

use clap::{Parser, Subcommand};
use std::path::PathBuf;

/// Unix-philosophy P25 trunked radio tools.
#[derive(Parser, Debug)]
#[command(name = "p25", version, about, long_about = None)]
pub struct Cli {
    /// Subcommand to execute.
    #[command(subcommand)]
    pub command: Command,
}

/// Available subcommands.
#[derive(Subcommand, Debug)]
pub enum Command {
    /// Decode a P25 control channel.
    ///
    /// Monitors a P25 control channel frequency, demodulates C4FM, decodes
    /// Trunking Signaling Blocks (TSBKs), and emits structured JSON to stdout.
    Cc(CcArgs),
}

/// Arguments for the `cc` (control channel) subcommand.
#[derive(clap::Args, Debug)]
pub struct CcArgs {
    /// Center frequency in Hertz (e.g., 853.450e6).
    #[arg(short = 'f', long)]
    pub frequency: Option<f64>,

    /// SoapySDR device string (e.g., "driver=rtlsdr").
    #[arg(long)]
    pub device: Option<String>,

    /// Decode from an IQ file instead of live SDR.
    #[arg(long)]
    pub file: Option<PathBuf>,

    /// Sample rate in samples per second (required with --file).
    #[arg(long)]
    pub sample_rate: Option<f64>,

    /// Enable verbose debug output.
    #[arg(short, long)]
    pub verbose: bool,
}
