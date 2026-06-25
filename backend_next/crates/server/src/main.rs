use server::config::AppConfig;
use server::routes::app_with_state;
use server::state::AppState;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            std::env::var("RUST_LOG").unwrap_or_else(|_| "server=info,gateway=info".to_owned()),
        )
        .init();

    let config = AppConfig::load()?;
    let addr = config.server_bind_addr();
    tracing::info!(%addr, "starting backend_next server");
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    axum::serve(listener, app_with_state(AppState::production().await?)).await?;
    Ok(())
}
