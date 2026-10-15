// Copyright (C) 2026 whispr.dev
// SPDX-License-Identifier: GPL-3.0-only

//! The one error type the notebook returns. Its text is what `osintropy` prints after
//! "Error: ", so every message is written for the person at the keyboard.

use std::fmt;
use std::io;

use crate::models::Invalid;

#[derive(Debug)]
pub enum Error {
    /// Input that breaks a rule (4.1.1 raised `ValueError`).
    Invalid(String),
    /// A run or observation that isn't in the case file (4.1.1 raised `KeyError`).
    NotFound(String),
    /// A file could not be read or written.
    Io(io::Error),
    /// SQLite refused an operation, such as a duplicate source ID within one run.
    Sqlite(rusqlite::Error),
}

pub type Result<T> = std::result::Result<T, Error>;

impl Error {
    pub fn invalid(message: impl Into<String>) -> Error {
        Error::Invalid(message.into())
    }

    /// An I/O error with the path it concerns, so the message says which file failed.
    pub fn io_at(path: &std::path::Path, err: io::Error) -> Error {
        Error::Io(io::Error::new(
            err.kind(),
            format!("{}: {err}", path.display()),
        ))
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Invalid(message) | Error::NotFound(message) => f.write_str(message),
            Error::Io(err) => write!(f, "{err}"),
            Error::Sqlite(err) => write!(f, "{err}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Io(err) => Some(err),
            Error::Sqlite(err) => Some(err),
            Error::Invalid(_) | Error::NotFound(_) => None,
        }
    }
}

impl From<Invalid> for Error {
    fn from(err: Invalid) -> Error {
        Error::Invalid(err.0)
    }
}

impl From<io::Error> for Error {
    fn from(err: io::Error) -> Error {
        Error::Io(err)
    }
}

impl From<rusqlite::Error> for Error {
    fn from(err: rusqlite::Error) -> Error {
        Error::Sqlite(err)
    }
}
