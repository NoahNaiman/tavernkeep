use std::net::SocketAddr;

#[tokio::main]
async fn main() -> std::io::Result<()> {
    let addr: SocketAddr = "127.0.0.1:8080".parse().expect("valid address");
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, tavernkeep::api::router()).await
}
