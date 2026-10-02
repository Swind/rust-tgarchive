fn main() {
    if let Err(error) = telegram_message_archive::bootstrap::run() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
