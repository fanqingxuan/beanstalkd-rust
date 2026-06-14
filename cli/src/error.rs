use std::fmt;
use std::io;

#[derive(Debug)]
pub(crate) struct CliError(String);

impl CliError {
    pub(crate) fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl fmt::Display for CliError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<io::Error> for CliError {
    fn from(err: io::Error) -> Self {
        Self(err.to_string())
    }
}

pub(crate) type Result<T> = std::result::Result<T, CliError>;
