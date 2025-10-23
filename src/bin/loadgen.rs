//! Minimal closed-loop HTTP load generator, used by `make load` when `oha`
//! is not installed.
//!
//! usage: loadgen --url <url> [--url <url> ...] [--duration <secs>] [--concurrency <n>]

use std::time::{Duration, Instant};

#[derive(Debug)]
struct Args {
    urls: Vec<String>,
    duration: Duration,
    concurrency: usize,
}

fn parse_args() -> Result<Args, String> {
    let mut urls = Vec::new();
    let mut duration = Duration::from_secs(15);
    let mut concurrency = 64;
    let mut it = std::env::args().skip(1);
    while let Some(flag) = it.next() {
        let value = it.next().ok_or_else(|| format!("{flag} needs a value"))?;
        match flag.as_str() {
            "--url" => urls.push(value),
            "--duration" => {
                duration = Duration::from_secs(value.parse().map_err(|_| "bad --duration")?)
            }
            "--concurrency" => concurrency = value.parse().map_err(|_| "bad --concurrency")?,
            other => return Err(format!("unknown flag {other}")),
        }
    }
    if urls.is_empty() {
        return Err("at least one --url is required".into());
    }
    Ok(Args {
        urls,
        duration,
        concurrency: concurrency.max(1),
    })
}

fn percentile(sorted: &[Duration], p: f64) -> Duration {
    if sorted.is_empty() {
        return Duration::ZERO;
    }
    let rank = ((p / 100.0) * sorted.len() as f64).ceil() as usize;
    sorted[rank.clamp(1, sorted.len()) - 1]
}

#[tokio::main]
async fn main() {
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(2);
        }
    };
    let client = reqwest::Client::builder()
        .pool_max_idle_per_host(args.concurrency)
        .build()
        .expect("client builds");
    let started = Instant::now();
    let deadline = started + args.duration;
    let mut workers = Vec::new();
    for w in 0..args.concurrency {
        let client = client.clone();
        let urls = args.urls.clone();
        workers.push(tokio::spawn(async move {
            let mut latencies = Vec::new();
            let mut errors = 0u64;
            let mut i = w;
            while Instant::now() < deadline {
                let url = &urls[i % urls.len()];
                i += 1;
                let t = Instant::now();
                match client.get(url).send().await {
                    Ok(r) if r.status().is_success() => {
                        let _ = r.bytes().await;
                        latencies.push(t.elapsed());
                    }
                    _ => errors += 1,
                }
            }
            (latencies, errors)
        }));
    }
    let mut all = Vec::new();
    let mut errors = 0;
    for w in workers {
        let (l, e) = w.await.expect("worker panicked");
        all.extend(l);
        errors += e;
    }
    let elapsed = started.elapsed().as_secs_f64();
    all.sort_unstable();
    let ms = |d: Duration| d.as_secs_f64() * 1000.0;
    println!("requests:     {}", all.len());
    println!("errors:       {errors}");
    println!("duration_s:   {elapsed:.2}");
    println!("rps:          {:.0}", all.len() as f64 / elapsed);
    println!("p50_ms:       {:.3}", ms(percentile(&all, 50.0)));
    println!("p95_ms:       {:.3}", ms(percentile(&all, 95.0)));
    println!("p99_ms:       {:.3}", ms(percentile(&all, 99.0)));
}
