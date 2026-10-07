use super::database::db_error;
use crate::{
    application::{
        ArchiveWriter,
        services::{Application, ComponentStatus},
    },
    config::{Config, TelegramConfig},
    domain::Chat,
    infrastructure::{
        persistence::sqlite::SqliteStore,
        telegram::{AuthError, LoginProgress, TelegramAdapter},
    },
    interface::cli::CliError,
};
use std::sync::Arc;

pub(super) async fn auth_login(phone: Option<String>) -> Result<(), CliError> {
    let telegram = Config::telegram().map_err(CliError::InvalidInput)?;
    telegram.validate_paths().map_err(CliError::InvalidInput)?;
    let adapter = TelegramAdapter::open(telegram.api_id, &telegram.session_file)
        .await
        .map_err(|error| CliError::Telegram(error.to_string()))?;
    let result = login_with_adapter(&telegram, &adapter, phone).await;
    let shutdown = adapter
        .shutdown()
        .await
        .map_err(|_| CliError::Telegram("Telegram connection did not shut down cleanly".into()));
    result?;
    shutdown?;
    Ok(())
}

async fn login_with_adapter(
    telegram: &TelegramConfig,
    adapter: &TelegramAdapter,
    phone: Option<String>,
) -> Result<(), CliError> {
    let authorized = adapter
        .is_authorized()
        .await
        .map_err(|_| CliError::Telegram("could not check Telegram authorization".into()))?;
    use std::io::IsTerminal;
    if login_gate(authorized, std::io::stdin().is_terminal())? == LoginGate::AlreadyAuthorized {
        println!("Telegram session is already authorized.");
        return Ok(());
    }

    let phone = match phone {
        Some(phone) if !phone.trim().is_empty() => phone,
        Some(_) => {
            return Err(CliError::InvalidInput(
                "phone number cannot be empty".into(),
            ));
        }
        None => prompt_phone()?,
    };
    let challenge = adapter
        .request_login_code(&phone, &telegram.api_hash)
        .await
        .map_err(auth_error)?;
    let code = prompt_secret("Telegram login code: ")?;
    match adapter
        .sign_in(challenge, &code)
        .await
        .map_err(auth_error)?
    {
        LoginProgress::Authenticated => println!("Telegram session authorized."),
        LoginProgress::PasswordRequired(challenge) => {
            if let Some(hint) = challenge.hint.as_deref().filter(|hint| !hint.is_empty()) {
                eprintln!("Telegram requires two-factor authentication (hint: {hint}).");
            }
            let password = prompt_secret("Telegram two-factor password: ")?;
            adapter
                .check_password(*challenge, password)
                .await
                .map_err(auth_error)?;
            println!("Telegram session authorized.");
        }
    }
    Ok(())
}

#[derive(Debug, PartialEq, Eq)]
enum LoginGate {
    AlreadyAuthorized,
    PromptForCredentials,
}

/// A terminal is only required when credentials must actually be entered.
fn login_gate(authorized: bool, interactive: bool) -> Result<LoginGate, CliError> {
    if authorized {
        Ok(LoginGate::AlreadyAuthorized)
    } else if interactive {
        Ok(LoginGate::PromptForCredentials)
    } else {
        Err(CliError::Telegram(
            "auth login requires an interactive terminal for hidden credential input".into(),
        ))
    }
}

fn prompt_phone() -> Result<String, CliError> {
    use std::io::Write;
    print!("Phone number: ");
    std::io::stdout()
        .flush()
        .map_err(|_| CliError::Telegram("could not prompt for phone number".into()))?;
    let mut phone = String::new();
    std::io::stdin()
        .read_line(&mut phone)
        .map_err(|_| CliError::Telegram("could not read phone number".into()))?;
    let phone = phone.trim().to_owned();
    if phone.is_empty() {
        return Err(CliError::InvalidInput(
            "phone number is required; interactive input was empty".into(),
        ));
    }
    Ok(phone)
}

fn prompt_secret(prompt: &str) -> Result<String, CliError> {
    rpassword::prompt_password(prompt)
        .map_err(|_| CliError::Telegram("hidden input requires an interactive terminal".into()))
}

fn auth_error(error: AuthError) -> CliError {
    let message = match error {
        AuthError::InvalidCode => "Telegram rejected the login code".to_owned(),
        AuthError::InvalidPassword => "Telegram rejected the two-factor password".to_owned(),
        AuthError::SignUpRequired => {
            "this account must first be registered with an official Telegram client".to_owned()
        }
        AuthError::FloodWait {
            retry_after_seconds,
        } => format!("Telegram rate limited login; retry after {retry_after_seconds} seconds"),
        AuthError::Telegram(_) => {
            "Telegram login failed; check connectivity and API credentials".to_owned()
        }
    };
    CliError::Telegram(message)
}

pub(super) async fn open_authorized_telegram(
    config: &TelegramConfig,
) -> Result<Arc<TelegramAdapter>, CliError> {
    let adapter = Arc::new(
        TelegramAdapter::open(config.api_id, &config.session_file)
            .await
            .map_err(|error| CliError::Telegram(error.to_string()))?,
    );
    match adapter.is_authorized().await {
        Ok(true) => Ok(adapter),
        Ok(false) => {
            let _ = adapter.shutdown().await;
            Err(CliError::Telegram(
                "Telegram session is not authorized; run `tgarchive auth login`".into(),
            ))
        }
        Err(_) => {
            let _ = adapter.shutdown().await;
            Err(CliError::Telegram(
                "could not check Telegram authorization; verify connectivity and credentials"
                    .into(),
            ))
        }
    }
}

/// Binds the archive to the authenticated account and stores its profile as a sender, so the
/// account's own messages show by name.
pub(super) async fn bind_account(
    store: &SqliteStore,
    adapter: &TelegramAdapter,
) -> Result<(), CliError> {
    let account = adapter
        .authenticated_account()
        .await
        .map_err(|_| CliError::Telegram("could not resolve Telegram account identity".into()))?;
    store
        .bind_telegram_account(account.id)
        .await
        .map_err(db_error)?;
    store
        .write_batch(crate::application::IngestBatch {
            senders: vec![account],
            ..Default::default()
        })
        .await
        .map_err(db_error)
}

pub(super) async fn refresh_with_adapter(
    database_url: &str,
    adapter: Arc<TelegramAdapter>,
) -> Result<Vec<Chat>, CliError> {
    let store = Arc::new(
        SqliteStore::connect(database_url)
            .await
            .map_err(|error| CliError::Database(error.to_string()))?,
    );
    let result = async {
        bind_account(&store, &adapter).await?;
        let application = Application::new(
            store.clone(),
            store.clone(),
            store.clone(),
            store.clone(),
            Some(adapter),
            ComponentStatus::disabled(),
        );
        application.refresh_chats().await.map_err(CliError::from)
    }
    .await;
    store.close().await;
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn authorized_session_needs_no_terminal() {
        assert_eq!(
            login_gate(true, false).unwrap(),
            LoginGate::AlreadyAuthorized
        );
        assert_eq!(
            login_gate(true, true).unwrap(),
            LoginGate::AlreadyAuthorized
        );
    }

    #[test]
    fn unauthorized_session_requires_a_terminal() {
        assert_eq!(
            login_gate(false, true).unwrap(),
            LoginGate::PromptForCredentials
        );
        assert!(login_gate(false, false).is_err());
    }
}
