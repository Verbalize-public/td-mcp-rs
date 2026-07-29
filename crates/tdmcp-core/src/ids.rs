//! Boundary newtypes: process identity and operator paths.

use std::fmt;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// OS process id — sole address for tools and queues.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(transparent)]
pub struct Pid(pub u32);

impl Pid {
    /// Wrap a raw OS pid.
    #[must_use]
    pub const fn new(raw: u32) -> Self {
        Self(raw)
    }

    /// Raw OS pid value.
    #[must_use]
    pub const fn get(self) -> u32 {
        self.0
    }
}

impl fmt::Display for Pid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<u32> for Pid {
    fn from(value: u32) -> Self {
        Self(value)
    }
}

impl From<Pid> for u32 {
    fn from(value: Pid) -> Self {
        value.0
    }
}

/// Operator path string passed to TD for resolution via `td.op()`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(transparent)]
pub struct OpPath(pub String);

impl OpPath {
    /// Construct from an owned string.
    #[must_use]
    pub fn new(path: impl Into<String>) -> Self {
        Self(path.into())
    }

    /// Borrow the path string.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for OpPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<String> for OpPath {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<&str> for OpPath {
    fn from(value: &str) -> Self {
        Self(value.to_owned())
    }
}

impl AsRef<str> for OpPath {
    fn as_ref(&self) -> &str {
        &self.0
    }
}
