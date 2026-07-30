use std::{net::IpAddr, time::Duration};

use clap::{Parser, ValueEnum};

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum LogFormat {
    Pretty,
    Json,
}

#[derive(Debug, Clone, Parser)]
#[command(
    name = "tauri-wd",
    version,
    about = "Cross-platform W3C WebDriver for Tauri apps"
)]
pub struct Cli {
    /// Port exposed to WebDriver clients.
    #[arg(long, default_value_t = 4444)]
    pub port: u16,

    /// Maximum simultaneous app sessions.
    #[arg(long, default_value_t = 4, value_parser = parse_max_sessions)]
    pub max_sessions: usize,

    /// Maximum time for an app and its webview bridge to become responsive.
    #[arg(long, default_value_t = 30, value_parser = parse_positive_u64)]
    pub startup_timeout: u64,

    /// Hard upper bound for one proxied WebDriver command.
    #[arg(long, default_value_t = 310, value_parser = parse_positive_u64)]
    pub command_timeout: u64,

    /// Timeout used by the startup JavaScript health probe.
    #[arg(long, default_value_t = 2_000, value_parser = parse_positive_u64)]
    pub probe_timeout_ms: u64,

    /// Maximum accepted request or response body in MiB.
    #[arg(long, default_value_t = 64, value_parser = parse_body_limit)]
    pub max_body_mib: usize,

    /// Log filter, such as info or tauri_wd=debug.
    #[arg(long, default_value = "info")]
    pub log: String,

    /// Human-readable or structured logs.
    #[arg(long, value_enum, default_value_t = LogFormat::Pretty)]
    pub log_format: LogFormat,
}

fn parse_max_sessions(value: &str) -> Result<usize, String> {
    let value = value
        .parse::<usize>()
        .map_err(|_| "must be an integer from 1 to 64".to_owned())?;
    (1..=64)
        .contains(&value)
        .then_some(value)
        .ok_or_else(|| "must be from 1 to 64".to_owned())
}

fn parse_positive_u64(value: &str) -> Result<u64, String> {
    value
        .parse::<u64>()
        .ok()
        .filter(|value| *value > 0)
        .ok_or_else(|| "must be a positive integer".to_owned())
}

fn parse_body_limit(value: &str) -> Result<usize, String> {
    let value = value
        .parse::<usize>()
        .map_err(|_| "must be an integer from 1 to 1024".to_owned())?;
    (1..=1024)
        .contains(&value)
        .then_some(value)
        .ok_or_else(|| "must be from 1 to 1024 MiB".to_owned())
}

#[derive(Debug, Clone)]
pub struct DriverConfig {
    pub host: IpAddr,
    pub port: u16,
    pub max_sessions: usize,
    pub startup_timeout: Duration,
    pub command_timeout: Duration,
    pub probe_timeout: Duration,
    pub max_body_bytes: usize,
}

impl From<&Cli> for DriverConfig {
    fn from(cli: &Cli) -> Self {
        Self {
            host: IpAddr::V4(std::net::Ipv4Addr::LOCALHOST),
            port: cli.port,
            max_sessions: cli.max_sessions,
            startup_timeout: Duration::from_secs(cli.startup_timeout),
            command_timeout: Duration::from_secs(cli.command_timeout),
            probe_timeout: Duration::from_millis(cli.probe_timeout_ms),
            max_body_bytes: cli.max_body_mib.saturating_mul(1024 * 1024),
        }
    }
}

#[cfg(test)]
impl Default for DriverConfig {
    fn default() -> Self {
        Self {
            host: IpAddr::V4(std::net::Ipv4Addr::LOCALHOST),
            port: 4444,
            max_sessions: 4,
            startup_timeout: Duration::from_secs(2),
            command_timeout: Duration::from_secs(2),
            probe_timeout: Duration::from_millis(250),
            max_body_bytes: 1024 * 1024,
        }
    }
}
