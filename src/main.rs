#[tokio::main]
async fn main() {
    if let Err(error) = tgarchive::bootstrap::run().await {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
