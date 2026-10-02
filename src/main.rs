#[tokio::main]
async fn main() {
    if let Err(error) = telegram_message_archive::bootstrap::run().await {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
