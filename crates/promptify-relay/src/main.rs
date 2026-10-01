//! `promptify-relay [--listen ADDR]` — run behind a TLS-terminating proxy (Caddy, nginx, Cloudflare).

use promptify_relay::{Limits, Relay, router};

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().collect();
    let listen = args
        .iter()
        .position(|a| a == "--listen")
        .and_then(|i| args.get(i + 1).cloned())
        .or_else(|| std::env::var("PROMPTIFY_RELAY_LISTEN").ok())
        .unwrap_or_else(|| "127.0.0.1:8787".into());
    let listener = match tokio::net::TcpListener::bind(&listen).await {
        Ok(listener) => listener,
        Err(e) => {
            eprintln!("cannot listen on {listen}: {e}");
            std::process::exit(1);
        }
    };
    eprintln!("promptify-relay listening on {listen}");
    let app = router(Relay::new(Limits::default()));
    let shutdown = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    if let Err(e) = axum::serve(listener, app).with_graceful_shutdown(shutdown).await {
        eprintln!("relay stopped: {e}");
        std::process::exit(1);
    }
}
