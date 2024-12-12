use std::{error, fmt};

/// Indicates that the user cancelled the operation.
#[derive(Debug)]
pub struct Cancelled;

impl fmt::Display for Cancelled {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Task cancelled")
    }
}

impl error::Error for Cancelled {}
