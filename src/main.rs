use tavernkeep::clock::SystemClock;
use tavernkeep::config::Config;
use tavernkeep::telemetry;

#[tokio::main]
async fn main() {
    telemetry::init_logging();
    let config = match Config::from_env() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(2);
        }
    };
    if let Err(e) = serve(config).await {
        tracing::error!(error = %e, "server failed");
        std::process::exit(1);
    }
}

async fn serve(config: Config) -> std::io::Result<()> {
    let bind = config.bind;
    let upstream = config.upstream_url.clone();
    let (router, state) = tavernkeep::app(config, SystemClock::shared());
    // The Prometheus recorder needs periodic upkeep to drain histogram
    // samples when nothing scrapes /metrics.
    let metrics = state.metrics.clone();
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(std::time::Duration::from_secs(5));
        loop {
            tick.tick().await;
            metrics.run_upkeep();
        }
    });
    let listener = tokio::net::TcpListener::bind(bind).await?;
    tracing::info!(%bind, %upstream, "tavernkeep listening");
    axum::serve(listener, router)
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    tracing::info!("tavernkeep stopped");
    Ok(())
}

async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let term = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut s) => {
                s.recv().await;
            }
            Err(e) => {
                tracing::warn!(error = %e, "cannot listen for SIGTERM");
                std::future::pending::<()>().await;
            }
        }
    };
    #[cfg(not(unix))]
    let term = std::future::pending::<()>();
    tokio::select! {
        () = ctrl_c => {},
        () = term => {},
    }
    tracing::info!("shutdown signal received, draining connections");
}
