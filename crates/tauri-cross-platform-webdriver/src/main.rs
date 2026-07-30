use clap::Parser;
use tauri_cross_platform_webdriver::{
    config::{Cli, DriverConfig, LogFormat},
    serve,
};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    let filter = EnvFilter::try_from_default_env()
        .or_else(|_| EnvFilter::try_new(&cli.log))
        .unwrap_or_else(|_| EnvFilter::new("info"));
    let subscriber = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false);
    match cli.log_format {
        LogFormat::Pretty => subscriber.init(),
        LogFormat::Json => subscriber.json().flatten_event(true).init(),
    }

    if let Err(error) = serve(DriverConfig::from(&cli)).await {
        tracing::error!("{error}");
        std::process::exit(1);
    }
}
