use std::{
    fmt,
    path::{Component, Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use thiserror::Error;
use ulid::Ulid;

/// A validation failure for a persisted identifier or path.
#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum ValueError {
    #[error("invalid checkpoint ID {0:?}")]
    CheckpointId(String),
    #[error("invalid checkpoint selector {0:?}")]
    CheckpointSelector(String),
    #[error("invalid content hash {0:?}")]
    ObjectHash(String),
    #[error("invalid filesystem root ID {0:?}")]
    RootId(String),
    #[error("invalid PostgreSQL operation ID {0:?}")]
    OperationId(String),
    #[error("unsafe relative path {0}")]
    RelativePath(PathBuf),
}

/// A canonical 26-character ULID used by a published checkpoint.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct CheckpointId(String);

impl CheckpointId {
    /// Validates a canonical checkpoint ID.
    pub fn new(value: impl Into<String>) -> Result<Self, ValueError> {
        let value = value.into();
        let valid = value.len() == 26
            && Ulid::from_string(&value).is_ok_and(|parsed| parsed.to_string() == value);
        valid
            .then_some(Self(value.clone()))
            .ok_or(ValueError::CheckpointId(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn into_string(self) -> String {
        self.0
    }
}

impl TryFrom<String> for CheckpointId {
    type Error = ValueError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl From<CheckpointId> for String {
    fn from(value: CheckpointId) -> Self {
        value.0
    }
}

impl fmt::Display for CheckpointId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// A user-facing checkpoint reference: `latest` or a non-empty ULID prefix.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct CheckpointSelector(String);

impl CheckpointSelector {
    /// Selects the most recent published checkpoint.
    #[must_use]
    pub fn latest() -> Self {
        Self("latest".to_owned())
    }

    /// Parses `latest` or a non-empty checkpoint ID prefix.
    pub fn parse(value: &str) -> Result<Self, ValueError> {
        if value == "latest" {
            return Ok(Self::latest());
        }
        let valid = value.len() <= 26
            && !value.is_empty()
            && value.bytes().all(|byte| byte.is_ascii_alphanumeric());
        valid
            .then(|| Self(value.to_owned()))
            .ok_or_else(|| ValueError::CheckpointSelector(value.to_owned()))
    }

    /// Returns the selector in its CLI/store representation.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for CheckpointSelector {
    type Error = ValueError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(&value)
    }
}

impl From<CheckpointSelector> for String {
    fn from(value: CheckpointSelector) -> Self {
        value.0
    }
}

/// A BLAKE3 object identifier in its persisted hexadecimal representation.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct ObjectHash(String);

impl ObjectHash {
    /// Validates a 64-character hexadecimal content hash.
    pub fn new(value: impl Into<String>) -> Result<Self, ValueError> {
        let value = value.into();
        (value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit()))
            .then_some(Self(value.clone()))
            .ok_or(ValueError::ObjectHash(value))
    }

    /// Returns the persisted hash representation.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for ObjectHash {
    type Error = ValueError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl From<ObjectHash> for String {
    fn from(value: ObjectHash) -> Self {
        value.0
    }
}

/// A non-empty filesystem root identifier safe for use as a path component.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct RootId(String);

impl RootId {
    /// Validates a filesystem root identifier.
    pub fn new(value: impl Into<String>) -> Result<Self, ValueError> {
        let value = value.into();
        (!value.is_empty()
            && value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_')))
        .then_some(Self(value.clone()))
        .ok_or(ValueError::RootId(value))
    }

    /// Returns the identifier as persisted in manifests.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for RootId {
    type Error = ValueError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl From<RootId> for String {
    fn from(value: RootId) -> Self {
        value.0
    }
}

/// A ten-character lowercase hexadecimal `PostgreSQL` restore operation ID.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct OperationId(String);

impl OperationId {
    /// Validates an operation-scoped `PostgreSQL` identifier suffix.
    pub fn new(value: impl Into<String>) -> Result<Self, ValueError> {
        let value = value.into();
        (value.len() == 10
            && value
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase()))
        .then_some(Self(value.clone()))
        .ok_or(ValueError::OperationId(value))
    }

    /// Returns the validated identifier.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for OperationId {
    type Error = ValueError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl From<OperationId> for String {
    fn from(value: OperationId) -> Self {
        value.0
    }
}

/// A manifest path containing only normal, relative components.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "PathBuf", into = "PathBuf")]
pub struct RelativePath(PathBuf);

impl RelativePath {
    /// Validates a relative path, permitting the root's empty representation.
    pub fn new(value: impl Into<PathBuf>) -> Result<Self, ValueError> {
        let value = value.into();
        let valid = !value.is_absolute()
            && (value.as_os_str().is_empty()
                || value
                    .components()
                    .all(|part| matches!(part, Component::Normal(_))));
        valid
            .then_some(Self(value.clone()))
            .ok_or(ValueError::RelativePath(value))
    }

    /// Validates a non-empty relative path for transaction and journal use.
    pub fn require_nonempty(value: impl Into<PathBuf>) -> Result<Self, ValueError> {
        let value = Self::new(value)?;
        if value.0.as_os_str().is_empty() {
            return Err(ValueError::RelativePath(value.0));
        }
        Ok(value)
    }

    /// Returns the validated relative path.
    pub fn as_path(&self) -> &Path {
        &self.0
    }
}

impl TryFrom<PathBuf> for RelativePath {
    type Error = ValueError;

    fn try_from(value: PathBuf) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl From<RelativePath> for PathBuf {
    fn from(value: RelativePath) -> Self {
        value.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn value_objects_reject_unsafe_representations() {
        assert!(CheckpointId::new("not-an-id").is_err());
        assert!(ObjectHash::new("xyz").is_err());
        assert!(RootId::new("../root").is_err());
        assert!(OperationId::new("ABCDEF0123").is_err());
        assert!(RelativePath::new("../escape").is_err());
        assert!(RelativePath::require_nonempty("").is_err());
    }
}
