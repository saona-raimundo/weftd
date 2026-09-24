// src/cli.rs

use clap::Parser;

#[derive(Parser)]
#[command(name = "weftd")]
pub struct Args {
    /// Path to a TOML config/session file
    pub path: Option<String>,

    /// Timeout in seconds for model loading (default: 120)
    #[arg(short, long, default_value = "120")]
    pub timeout: u64,

    /// Path to server log file
    #[arg(long, default_value = "server.log")]
    pub serve_log: String,

    /// Don't auto-open browser at startup
    #[arg(long)]
    pub no_open: bool,

    /// Serve static files from disk for hot-reload during frontend development
    #[arg(long)]
    pub dev: bool,
}

pub fn parse() -> Args {
    Args::parse()
}
