use hzr_core::{Config, ConfigPaths};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let paths = ConfigPaths::discover();
    let config = Config::load_or_default(&paths.config_file)?;
    hzr_daemon::serve(config, async {
        let _ = tokio::signal::ctrl_c().await;
    })
    .await?;
    Ok(())
}
