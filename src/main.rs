use tavernkeep::clock::SystemClock;
use tavernkeep::config::Config;
use tavernkeep::{telemetry, warm};

const USAGE: &str = "\
usage:
  tavernkeep [serve]                       run the API (configured by TK_* env vars)
  tavernkeep warm --file <party.txt> [--url <http://host:port>]
                                           prefetch slugs into a running server
                                           (default URL: $TK_WARM_URL or http://127.0.0.1:8080)
";

enum Command {
    Serve,
    Warm { file: String, url: String },
    Help,
}

fn parse_args(args: &[String]) -> Result<Command, String> {
    match args.first().map(String::as_str) {
        None => Ok(Command::Serve),
        Some("serve") if args.len() == 1 => Ok(Command::Serve),
        Some("-h" | "--help" | "help") => Ok(Command::Help),
        Some("warm") => {
            let mut file = None;
            let mut url = std::env::var("TK_WARM_URL")
                .unwrap_or_else(|_| "http://127.0.0.1:8080".to_string());
            let mut it = args[1..].iter();
            while let Some(flag) = it.next() {
                let value = it.next().ok_or_else(|| format!("{flag} needs a value"))?;
                match flag.as_str() {
                    "--file" | "-f" => file = Some(value.clone()),
                    "--url" => url = value.clone(),
                    other => return Err(format!("unknown flag {other}")),
                }
            }
            let file = file.ok_or("warm needs --file <party.txt>")?;
            Ok(Command::Warm { file, url })
        }
        Some(other) => Err(format!("unknown command {other:?}")),
    }
}

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let command = match parse_args(&args) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("error: {e}\n\n{USAGE}");
            std::process::exit(2);
        }
    };
    match command {
        Command::Help => print!("{USAGE}"),
        Command::Warm { file, url } => std::process::exit(warm(&file, &url).await),
        Command::Serve => {
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
    }
}

async fn warm(file: &str, url: &str) -> i32 {
    let text = match std::fs::read_to_string(file) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("error: cannot read {file}: {e}");
            return 2;
        }
    };
    let entries = match warm::parse_party(&text) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("error: {file}: {e}");
            return 2;
        }
    };
    let report = warm::run(url, &entries).await;
    for e in &report.warmed {
        println!("ok        {} {}", e.kind.as_str(), e.slug);
    }
    for e in &report.not_found {
        println!("not found {} {}", e.kind.as_str(), e.slug);
    }
    for (e, why) in &report.failed {
        println!("failed    {} {} ({why})", e.kind.as_str(), e.slug);
    }
    println!(
        "warmed {}/{} entries via {url}",
        report.warmed.len(),
        entries.len()
    );
    if report.is_success() { 0 } else { 1 }
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
