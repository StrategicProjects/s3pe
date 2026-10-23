//! Errors of the client. Each variant is a situation the caller can act on.

use std::fmt;

/// What can go wrong when talking to the store.
#[derive(Debug)]
pub enum Error {
    /// Missing credentials, missing bucket, bad endpoint.
    Config(String),
    /// Could not reach the server, or the connection broke (retried first).
    Transport(String),
    /// The object or bucket does not exist (HTTP 404).
    NotFound {
        /// Bucket asked for.
        bucket: String,
        /// Key asked for (empty for bucket-level operations).
        key: String,
    },
    /// The credentials do not allow this (HTTP 403).
    AccessDenied {
        /// Bucket asked for.
        bucket: String,
        /// Key asked for.
        key: String,
        /// The server's message, when it sent one.
        message: String,
    },
    /// Any other HTTP error, with the S3 error code and message when given.
    Http {
        /// HTTP status.
        status: u16,
        /// S3 error code (`NoSuchBucket`, …), empty if none.
        code: String,
        /// S3 error message, empty if none.
        message: String,
        /// Bucket asked for.
        bucket: String,
        /// Key asked for.
        key: String,
    },
    /// The bytes that arrived are not the bytes that were sent.
    Integrity(String),
    /// A local file could not be read or written.
    Io(std::io::Error),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Config(m) => write!(f, "configuration: {m}"),
            Error::Transport(m) => write!(f, "transport: {m}"),
            Error::NotFound { bucket, key } if key.is_empty() => {
                write!(f, "not found: bucket {bucket}")
            }
            Error::NotFound { bucket, key } => write!(f, "not found: s3://{bucket}/{key}"),
            Error::AccessDenied {
                bucket,
                key,
                message,
            } => {
                write!(f, "access denied: s3://{bucket}/{key}")?;
                if !message.is_empty() {
                    write!(f, " ({message})")?;
                }
                Ok(())
            }
            Error::Http {
                status,
                code,
                message,
                bucket,
                key,
            } => {
                write!(f, "HTTP {status}")?;
                if !code.is_empty() {
                    write!(f, " {code}")?;
                }
                if !message.is_empty() {
                    write!(f, ": {message}")?;
                }
                write!(f, " (s3://{bucket}/{key})")
            }
            Error::Integrity(m) => write!(f, "integrity: {m}"),
            Error::Io(e) => write!(f, "io: {e}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Io(e) => Some(e),
            _ => None,
        }
    }
}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::Io(e)
    }
}

/// `Result` with this crate's [`Error`].
pub type Result<T> = std::result::Result<T, Error>;
