use serde::Serialize;

/// Every failure the engine can report. `message` is safe to show the user and
/// never contains a password.
#[derive(Debug, thiserror::Error)]
pub enum CoreError {
    #[error("The mail server rejected this email address or password.")]
    Auth,
    #[error("Could not reach the mail server: {0}")]
    Network(String),
    #[error("The mail server reported a problem: {0}")]
    Imap(String),
    #[error("{0}")]
    Invalid(String),
    #[error("Not found: {0}")]
    NotFound(String),
    #[error("Local mail database problem: {0}")]
    Db(#[from] rusqlite::Error),
    #[error("Could not use the macOS Keychain: {0}")]
    Secret(String),
    #[error("This message has not been downloaded yet, and the mail server cannot be reached.")]
    Offline,
    /// The message could not be sent, in words safe to show the user.
    #[error("{message}")]
    Send { code: String, message: String },
    /// A refusal from the account server, in words safe to show the user.
    #[error("{message}")]
    Cloud { code: String, message: String },
    /// Their browser sign-in has run out, and only signing in again fixes it.
    #[error("{0} needs you to sign in again.")]
    NeedsSignIn(String),
    /// A problem using the person's AI key, in words safe to show them.
    #[error("{message}")]
    Ai { code: String, message: String },
}

impl CoreError {
    pub fn code(&self) -> &str {
        match self {
            CoreError::Auth => "auth",
            CoreError::Network(_) => "network",
            CoreError::Imap(_) => "imap",
            CoreError::Invalid(_) => "invalid",
            CoreError::NotFound(_) => "not_found",
            CoreError::Db(_) => "db",
            CoreError::Secret(_) => "secret",
            CoreError::Offline => "offline",
            CoreError::Send { code, .. } => code,
            CoreError::Cloud { code, .. } => code,
            CoreError::Ai { code, .. } => code,
            CoreError::NeedsSignIn(_) => "needs_signin",
        }
    }
}

impl Serialize for CoreError {
    fn serialize<S: serde::Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let mut st = s.serialize_struct("CoreError", 2)?;
        st.serialize_field("code", self.code())?;
        st.serialize_field("message", &self.to_string())?;
        st.end()
    }
}

pub type Result<T> = std::result::Result<T, CoreError>;
