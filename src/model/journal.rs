use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RestoreJournal {
    pub version: u32,
    pub target_id: String,
    pub recovery_id: String,
    pub phase: JournalPhase,
    /// True once the commit phase has begun; unlike `phase`, never reset when
    /// the journal moves to `RollingBack`.
    #[serde(default)]
    pub commit_started: bool,
    pub swaps: Vec<SwapRecord>,
    #[serde(default)]
    pub database_swaps: Vec<DatabaseSwap>,
    #[serde(default)]
    pub preserved_paths: Vec<PreserveRecord>,
    #[serde(default)]
    pub preservation_root: PathBuf,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PreserveRecord {
    pub live: PathBuf,
    pub held: PathBuf,
    #[serde(default)]
    pub detached: bool,
    #[serde(default)]
    pub reattached: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DatabaseSwap {
    pub name: String,
    pub url_env: String,
    pub database: String,
    pub temporary_database: String,
    pub old_database: String,
    #[serde(default)]
    pub system_identifier: Option<String>,
    #[serde(default)]
    pub operation_id: Option<String>,
    #[serde(default)]
    pub original_database_oid: Option<u32>,
    #[serde(default)]
    pub temporary_database_oid: Option<u32>,
    #[serde(default)]
    pub temporary_owned: bool,
    #[serde(default)]
    pub cutover_owned: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum JournalPhase {
    Prepared,
    Committing,
    RollingBack,
    Complete,
}

impl JournalPhase {
    /// Returns the stable persisted phase name.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Prepared => "prepared",
            Self::Committing => "committing",
            Self::RollingBack => "rolling_back",
            Self::Complete => "complete",
        }
    }
}

/// An invalid runtime transition in a persisted restore journal.
#[derive(Clone, Debug, Error, PartialEq, Eq)]
#[error("invalid restore journal phase transition from {from} to {to}")]
pub struct JournalTransitionError {
    from: &'static str,
    to: &'static str,
}

impl RestoreJournal {
    /// Advances the journal only along a valid transaction or recovery edge.
    ///
    /// Repeating the current phase is accepted to support idempotent recovery.
    pub fn transition_to(&mut self, next: JournalPhase) -> Result<(), JournalTransitionError> {
        let valid = self.phase == next
            || matches!(
                (&self.phase, &next),
                (
                    JournalPhase::Prepared,
                    JournalPhase::Committing | JournalPhase::RollingBack
                ) | (
                    JournalPhase::Committing,
                    JournalPhase::Complete | JournalPhase::RollingBack
                ) | (JournalPhase::RollingBack, JournalPhase::Prepared)
            );
        if !valid {
            return Err(JournalTransitionError {
                from: self.phase.as_str(),
                to: next.as_str(),
            });
        }
        self.phase = next;
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SwapRecord {
    #[serde(default)]
    pub root_id: String,
    #[serde(default)]
    pub root_path: PathBuf,
    #[serde(default)]
    pub entry_relative: PathBuf,
    pub live: PathBuf,
    pub staged: PathBuf,
    pub old: PathBuf,
    pub live_existed: bool,
    #[serde(default)]
    pub target_existed: bool,
    pub committed: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn complete_is_a_one_way_barrier() {
        let mut journal = RestoreJournal {
            version: 2,
            target_id: String::new(),
            recovery_id: String::new(),
            phase: JournalPhase::Committing,
            commit_started: true,
            swaps: Vec::new(),
            database_swaps: Vec::new(),
            preserved_paths: Vec::new(),
            preservation_root: PathBuf::new(),
        };
        journal.transition_to(JournalPhase::Complete).unwrap();
        assert!(journal.transition_to(JournalPhase::RollingBack).is_err());
    }
}
