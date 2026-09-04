use hzr_core::{Config, ConfigPaths};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let paths = ConfigPaths::discover();
    let config = Config::load_or_default(&paths.config_file)?;
    let shutdown = hzr_daemon::shutdown_signal()?;
    hzr_daemon::serve(config, shutdown).await?;
    Ok(())
}
