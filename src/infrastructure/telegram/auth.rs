use grammers_client::client::{LoginToken, PasswordToken, SignInError};

use super::session::TelegramAdapter;

/// Opaque one-time challenge returned after asking Telegram to send a login code.
pub struct LoginChallenge(LoginToken);

/// Opaque proof challenge required when the account has two-factor authentication enabled.
pub struct PasswordChallenge {
    token: PasswordToken,
    pub hint: Option<String>,
}

pub enum LoginProgress {
    Authenticated,
    PasswordRequired(Box<PasswordChallenge>),
}

#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    #[error("Telegram rejected the login code")]
    InvalidCode,
    #[error("Telegram rejected the two-factor password")]
    InvalidPassword,
    #[error("this account must be registered using an official Telegram client first")]
    SignUpRequired,
    #[error("Telegram rate limited authentication for {retry_after_seconds} seconds")]
    FloodWait { retry_after_seconds: u64 },
    #[error("Telegram authentication failed: {0}")]
    Telegram(String),
}

impl TelegramAdapter {
    /// Requests a one-time login code. Neither the phone nor the app hash is retained or logged.
    pub async fn request_login_code(
        &self,
        phone: &str,
        api_hash: &str,
    ) -> Result<LoginChallenge, AuthError> {
        self.client
            .request_login_code(phone, api_hash)
            .await
            .map(LoginChallenge)
            .map_err(map_error)
    }

    /// Completes code-based login or returns an opaque 2FA challenge.
    pub async fn sign_in(
        &self,
        challenge: LoginChallenge,
        code: &str,
    ) -> Result<LoginProgress, AuthError> {
        match self.client.sign_in(&challenge.0, code).await {
            Ok(_) => Ok(LoginProgress::Authenticated),
            Err(SignInError::PasswordRequired(token)) => {
                let hint = token.hint().map(str::to_owned);
                Ok(LoginProgress::PasswordRequired(Box::new(
                    PasswordChallenge { token, hint },
                )))
            }
            Err(error) => Err(map_sign_in_error(error)),
        }
    }

    /// Completes two-factor login. The password is passed directly to Grammers and is not stored.
    pub async fn check_password(
        &self,
        challenge: PasswordChallenge,
        password: impl AsRef<[u8]>,
    ) -> Result<(), AuthError> {
        match self.client.check_password(challenge.token, password).await {
            Ok(_) => Ok(()),
            Err(error) => Err(map_sign_in_error(error)),
        }
    }
}

fn map_sign_in_error(error: SignInError) -> AuthError {
    match error {
        SignInError::SignUpRequired => AuthError::SignUpRequired,
        SignInError::InvalidCode => AuthError::InvalidCode,
        SignInError::PasswordRequired(_) => AuthError::Telegram(
            "Telegram returned another password challenge unexpectedly".to_owned(),
        ),
        SignInError::InvalidPassword(_) => AuthError::InvalidPassword,
        SignInError::Other(error) => map_invocation(error),
    }
}

fn map_error(error: grammers_client::InvocationError) -> AuthError {
    map_invocation(error)
}

fn map_invocation(error: grammers_client::InvocationError) -> AuthError {
    use grammers_client::InvocationError;

    match error {
        InvocationError::Rpc(rpc) if rpc.name == "FLOOD_WAIT" => rpc
            .value
            .map(|seconds| AuthError::FloodWait {
                retry_after_seconds: u64::from(seconds),
            })
            .unwrap_or_else(|| {
                AuthError::Telegram("Telegram returned FLOOD_WAIT without a duration".into())
            }),
        error => AuthError::Telegram(error.to_string()),
    }
}
