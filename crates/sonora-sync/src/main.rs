use sonora_sync::{Config, router};
use tokio::net::TcpListener;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    tracing_subscriber::fmt().with_target(false).try_init()?;
    let config = Config::from_env()?;
    let app = router(config.navidrome_url, &config.db_path)?;
    let listener = TcpListener::bind(config.bind_addr).await?;
    tracing::info!(addr = %listener.local_addr()?, "Sonora sync relay listening");
    axum::serve(listener, app).await?;
    Ok(())
}
