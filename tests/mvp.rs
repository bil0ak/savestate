use std::fs;
use std::path::PathBuf;
use std::process::Command;

use anyhow::Result;
use rusqlite::Connection;
use savestate::{
    App,
    integrations::{self, Agent, CodexHookEvent},
    model::{JournalPhase, PreserveRecord, RestoreJournal, SwapRecord},
    store::Store,
};
use tempfile::tempdir;

#[path = "mvp/api.rs"]
mod api;
#[path = "mvp/checkpoints.rs"]
mod checkpoints;
#[path = "mvp/databases.rs"]
mod databases;
#[path = "mvp/external_roots.rs"]
mod external_roots;
#[path = "mvp/ignored_state.rs"]
mod ignored_state;
#[path = "mvp/performance.rs"]
mod performance;
#[path = "mvp/recovery.rs"]
mod recovery;
#[path = "mvp/restore.rs"]
mod restore;
#[path = "mvp/retention.rs"]
mod retention;
#[path = "mvp/validation.rs"]
mod validation;

fn psql(url: &str, sql: &str) -> Result<String> {
    let output = Command::new("psql")
        .args([
            "--no-psqlrc",
            "--tuples-only",
            "--no-align",
            "--dbname",
            url,
            "--command",
            sql,
        ])
        .output()?;
    if !output.status.success() {
        anyhow::bail!("psql failed: {}", String::from_utf8_lossy(&output.stderr));
    }
    Ok(String::from_utf8(output.stdout)?)
}

fn git(root: &std::path::Path, args: &[&str]) -> Result<()> {
    let output = Command::new("git").current_dir(root).args(args).output()?;
    if !output.status.success() {
        anyhow::bail!("git failed: {}", String::from_utf8_lossy(&output.stderr));
    }
    Ok(())
}

fn git_output(root: &std::path::Path, args: &[&str]) -> Result<String> {
    let output = Command::new("git").current_dir(root).args(args).output()?;
    if !output.status.success() {
        anyhow::bail!("git failed: {}", String::from_utf8_lossy(&output.stderr));
    }
    Ok(String::from_utf8(output.stdout)?)
}
