use thiserror::Error;

/// Application-wide result type alias.
pub type Result<T> = std::result::Result<T, AppError>;

/// Application error types.
#[derive(Debug, Error)]
pub enum AppError {
    /// I/O errors from filesystem operations.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// Terminal initialization or rendering errors.
    #[error("Terminal error: {0}")]
    Terminal(String),

    /// Invalid path provided by the user.
    #[error("Invalid path: {0}")]
    InvalidPath(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn io_error_conversion() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "file not found");
        let app_err: AppError = io_err.into();
        assert!(matches!(app_err, AppError::Io(_)));
        assert!(app_err.to_string().contains("file not found"));
    }

    #[test]
    fn terminal_error_display() {
        let err = AppError::Terminal("failed to enter raw mode".into());
        assert_eq!(err.to_string(), "Terminal error: failed to enter raw mode");
    }

    #[test]
    fn invalid_path_error_display() {
        let err = AppError::InvalidPath("/nonexistent".into());
        assert_eq!(err.to_string(), "Invalid path: /nonexistent");
    }
}
