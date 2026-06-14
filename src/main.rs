mod config;
mod constants;
mod model;
mod protocol;
mod server;
mod state;
mod wal;

#[tokio::main(flavor = "multi_thread")]
async fn main() {
    server::run().await;
}
