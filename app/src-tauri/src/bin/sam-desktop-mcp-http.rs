#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    app_lib::run_http_server().await
}
