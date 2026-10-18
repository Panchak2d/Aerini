use std::path::PathBuf;
use std::time::Duration;

use clap::Parser;

pub const DEFAULT_TIMEOUT_SECS: u64 = 30;
pub const DEFAULT_MAX_OUTPUT_BYTES: u64 = 32 * 1024;

#[derive(Debug, Parser)]
#[command(
    name = "aerini-mcp",
    version,
    about = "Model Context Protocol server for Aerini, over stdio.",
    long_about = "Model Context Protocol server for Aerini, over stdio.\n\n\
Online mode: set AERINI_URL (or --url) and AERINI_TOKEN to use an `aerini-server api` instance.\n\
Offline mode: leave AERINI_URL unset (needs a build with the `offline` feature) to read the node \
catalog, validate workflows, and write `.aerini` files for the desktop app. Offline mode never runs anything.\n\n\
The token is read only from the AERINI_TOKEN environment variable so it never shows in a process list."
)]
pub struct Cli {
    /// Base URL of an `aerini-server api` instance, e.g. http://127.0.0.1:3000.
    #[arg(long, env = "AERINI_URL")]
    pub url: Option<String>,

    /// Allow tools that change the server: save_workflow, delete_workflow, start_schedule, stop_schedule.
    #[arg(long)]
    pub allow_write: bool,

    /// Per-request timeout for server calls other than run_workflow, in seconds.
    #[arg(long, default_value_t = DEFAULT_TIMEOUT_SECS, value_parser = clap::value_parser!(u64).range(1..=3600))]
    pub timeout_secs: u64,

    /// Largest run output returned to the model, in bytes; the rest is cut and marked.
    #[arg(long, default_value_t = DEFAULT_MAX_OUTPUT_BYTES, value_parser = clap::value_parser!(u64).range(1024..=4_194_304))]
    pub max_output_bytes: u64,

    /// Offline mode: folder where save_workflow writes `.aerini` files. Defaults to ~/aerini-workflows.
    #[arg(long, env = "AERINI_OUT_DIR")]
    pub out_dir: Option<PathBuf>,
}

#[derive(Clone)]
pub struct Config {
    pub url: Option<String>,
    pub token: Option<String>,
    pub allow_write: bool,
    pub timeout: Duration,
    pub max_output_bytes: usize,
    pub out_dir: Option<PathBuf>,
}

impl std::fmt::Debug for Config {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Config")
            .field("url", &self.url)
            .field("token", &self.token.as_ref().map(|_| "<redacted>"))
            .field("allow_write", &self.allow_write)
            .field("timeout", &self.timeout)
            .field("max_output_bytes", &self.max_output_bytes)
            .field("out_dir", &self.out_dir)
            .finish()
    }
}

impl Config {
    pub fn from_cli(cli: Cli, token: Option<String>) -> Result<Self, String> {
        let url = non_empty(cli.url);
        let token = non_empty(token);
        if url.is_some() && token.is_none() {
            return Err("AERINI_URL is set but AERINI_TOKEN is not; the server rejects calls without a token".into());
        }
        Ok(Self {
            url,
            token,
            allow_write: cli.allow_write,
            timeout: Duration::from_secs(cli.timeout_secs),
            max_output_bytes: usize::try_from(cli.max_output_bytes).unwrap_or(usize::MAX),
            out_dir: cli.out_dir,
        })
    }

    pub fn from_env_and_args() -> Result<Self, String> {
        Self::from_cli(Cli::parse(), std::env::var("AERINI_TOKEN").ok())
    }
}

fn non_empty(v: Option<String>) -> Option<String> {
    v.map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cli(args: &[&str]) -> Cli {
        let mut full = vec!["aerini-mcp"];
        full.extend_from_slice(args);
        Cli::try_parse_from(full).expect("valid args")
    }

    #[test]
    fn url_without_token_is_rejected_and_blank_values_count_as_unset() {
        let err = Config::from_cli(cli(&["--url", "http://127.0.0.1:3000"]), Some("  ".into())).unwrap_err();
        assert!(err.contains("AERINI_TOKEN"));

        let cfg = Config::from_cli(cli(&["--url", "  "]), None).unwrap();
        assert!(cfg.url.is_none());
    }

    #[test]
    fn defaults_are_read_only_with_thirty_second_timeout() {
        let cfg = Config::from_cli(cli(&[]), None).unwrap();
        assert!(!cfg.allow_write);
        assert_eq!(cfg.timeout, Duration::from_secs(DEFAULT_TIMEOUT_SECS));
        assert_eq!(cfg.max_output_bytes as u64, DEFAULT_MAX_OUTPUT_BYTES);
    }

    #[test]
    fn out_of_range_limits_are_refused_by_the_parser() {
        assert!(Cli::try_parse_from(["aerini-mcp", "--timeout-secs", "0"]).is_err());
        assert!(Cli::try_parse_from(["aerini-mcp", "--max-output-bytes", "10"]).is_err());
    }
}
