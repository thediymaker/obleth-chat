use anyhow::Result;
use obleth_chat::{
    auth::{Auth, AuthConfig},
    config::{Config, ConnectionsMode},
    gateway::Gateway,
    router, AppState, MIGRATOR,
};
use sqlx::postgres::PgPoolOptions;
use std::time::Duration;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "obleth_chat=info".into()),
        )
        .init();
    let config = Config::from_env()?;
    let auth_config = AuthConfig::from_env()?;
    let pool = PgPoolOptions::new()
        .max_connections(config.db_connections)
        .acquire_timeout(Duration::from_secs(5))
        .connect(&config.database_url)
        .await?;
    MIGRATOR.run(&pool).await?;
    let auth = Auth::new(pool.clone(), auth_config).await?;
    let gateway = match (&config.gateway_url, &config.signing_key_file) {
        (Some(url), Some(path)) => Some(
            Gateway::from_file(
                url.clone(),
                path,
                config.gateway_issuer.clone(),
                config.gateway_audience.clone(),
            )
            .await?,
        ),
        _ => None,
    };
    let mut state = AppState::new(auth, pool.clone(), gateway, config.max_streams);
    state.admin_gateway_url = config.admin_gateway_url;
    // One bucket holds Knowledge documents and, when set, every other file's bytes.
    let objects = obleth_chat::knowledge::ObjectStore::from_env()?;
    if objects.is_some() {
        let files = obleth_chat::files::Files::new(pool.clone(), objects.clone());
        state.store = state.store.clone().with_files(files.clone());
        tokio::spawn(files.worker(state.shutdown.clone()));
        tracing::info!("files kept in object storage");
    }
    if let Some(sandbox_config) = obleth_chat::sandbox::SandboxConfig::from_env()? {
        // The browser runs in the workspace namespace, under the same access rule.
        if let Some(browser_config) = obleth_chat::browser::BrowserConfig::from_env()? {
            let browsers = obleth_chat::browser::Browsers::new(
                browser_config,
                sandbox_config.clone(),
                pool.clone(),
            )?;
            tokio::spawn(browsers.clone().worker(state.shutdown.clone()));
            tracing::info!("browser enabled");
            state.browsers = Some(browsers);
        }
        let sandboxes = obleth_chat::sandbox::Sandboxes::new(sandbox_config, pool.clone())?;
        tokio::spawn(sandboxes.clone().worker(state.shutdown.clone()));
        tokio::spawn(obleth_chat::research::worker(
            pool.clone(),
            sandboxes.clone(),
            state.shutdown.clone(),
        ));
        tracing::info!("workspaces enabled");
        state.sandboxes = Some(sandboxes);
    }
    // Connections need the gateway; their token key is derived from its signing key.
    if let (Some(path), true) = (
        &config.signing_key_file,
        config.connections != ConnectionsMode::Off,
    ) {
        let material = tokio::fs::read(path).await?;
        state.connections = Some(
            obleth_chat::connections::Connections::new(
                pool.clone(),
                &material,
                config.connections_allow_private,
            )?
            .clusters(config.connections == ConnectionsMode::All)
            .auto_mode(config.auto_mode),
        );
        tracing::info!(
            clusters = config.connections == ConnectionsMode::All,
            "connections enabled"
        );
    }
    if let Some(knowledge) = obleth_chat::knowledge::Knowledge::from_env(pool.clone(), objects)? {
        if let Some(gateway) = state.gateway.clone() {
            tokio::spawn(knowledge.clone().worker(gateway, state.shutdown.clone()));
        }
        tracing::info!("knowledge enabled");
        state.knowledge = Some(knowledge);
    }
    if let Some(dir) = config.backup_status_dir {
        state.metrics.register_backup_status(dir)?;
    }
    // Auto mode's watchdog nudges unattended turns that stopped without finishing.
    tokio::spawn(obleth_chat::autopilot::worker(
        state.clone(),
        state.shutdown.clone(),
    ));
    let mut maintenance = tokio::spawn(obleth_chat::retention::worker(
        pool.clone(),
        state.shutdown.clone(),
    ));
    let shutdown = state.shutdown.clone();
    let listener = tokio::net::TcpListener::bind(config.listen).await?;
    tracing::info!(address=%config.listen,"obleth-chat listening; local accounts enabled");
    let app = router(state.clone(), config.static_dir)
        .into_make_service_with_connect_info::<std::net::SocketAddr>();
    axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            #[cfg(unix)]
            {
                let mut term =
                    tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                        .expect("signal handler");
                tokio::select! {_=tokio::signal::ctrl_c()=>{},_=term.recv()=>{}}
            }
            #[cfg(not(unix))]
            {
                let _ = tokio::signal::ctrl_c().await;
            }
            shutdown.cancel();
        })
        .await?;
    let _ = tokio::time::timeout(Duration::from_secs(10), async {
        while state.metrics.streams.get() > 0 {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await;
    if tokio::time::timeout(Duration::from_secs(10), &mut maintenance)
        .await
        .is_err()
    {
        maintenance.abort();
        let _ = maintenance.await;
    }
    pool.close().await;
    Ok(())
}
