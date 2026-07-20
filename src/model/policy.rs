use std::path::Path;

use anyhow::{Context, Result, bail};
use globset::Glob;

/// Validated checkpoint counts consumed by retention logic.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RetentionPolicy {
    /// Number of automatic agent checkpoints to keep.
    pub agent: usize,
    /// Number of restore-recovery checkpoints to keep.
    pub recovery: usize,
    /// Number of pre-command checkpoints to keep.
    pub run: usize,
}

pub(crate) fn validate_scope_patterns(patterns: &[String], description: &str) -> Result<()> {
    use std::path::Component;

    for pattern in patterns {
        let path = Path::new(pattern);
        if pattern.is_empty()
            || path.is_absolute()
            || path
                .components()
                .any(|component| !matches!(component, Component::Normal(_)))
        {
            bail!("{description} pattern must stay relative to its filesystem root: {pattern:?}");
        }
        Glob::new(pattern).with_context(|| format!("invalid {description} pattern {pattern:?}"))?;
    }
    Ok(())
}
