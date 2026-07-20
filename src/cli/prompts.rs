use super::*;
use crate::{filesystem, ui};

pub(crate) fn render_restore_path_samples(report: &filesystem::DiffReport, checkpoint: &str) {
    const SAMPLE_LIMIT: usize = 4;
    if report.added.is_empty() && report.removed.is_empty() && report.modified.is_empty() {
        return;
    }

    ui::line(format_args!("{}", ui::muted("Changed paths:")));
    for path in report.added.iter().take(SAMPLE_LIMIT) {
        ui::line(format_args!("  {}", ui::added(format!("+ {path}"))));
    }
    for path in report.removed.iter().take(SAMPLE_LIMIT) {
        ui::line(format_args!("  {}", ui::removed(format!("- {path}"))));
    }
    for path in report.modified.iter().take(SAMPLE_LIMIT) {
        ui::line(format_args!("  {}", ui::modified(format!("~ {path}"))));
    }

    let hidden = report
        .added
        .len()
        .saturating_sub(SAMPLE_LIMIT)
        .saturating_add(report.removed.len().saturating_sub(SAMPLE_LIMIT))
        .saturating_add(report.modified.len().saturating_sub(SAMPLE_LIMIT));
    if hidden > 0 {
        ui::line(format_args!(
            "  {} {} more; inspect every path with {}",
            ui::muted("…"),
            hidden,
            ui::command(format!("savestate diff {checkpoint}"))
        ));
    }
}

pub(crate) fn print_restore_cancelled() {
    ui::line(format_args!("{}", ui::muted("Restore cancelled.")));
}

pub(crate) fn confirm_restore() -> Result<bool> {
    if !io::stdin().is_terminal() {
        bail!("restore confirmation requires a terminal; pass --yes or use --dry-run");
    }
    print!("Apply this restore? [y/N] ");
    io::stdout().flush()?;
    let mut answer = String::new();
    io::stdin().read_line(&mut answer)?;
    Ok(matches!(
        answer.trim().to_ascii_lowercase().as_str(),
        "y" | "yes"
    ))
}

pub(crate) fn confirm_remote_targets(tokens: &[String]) -> Result<()> {
    for token in tokens {
        if !io::stdin().is_terminal() {
            bail!("remote PostgreSQL confirmation requires a terminal");
        }
        print!("Remote database restore. Type {token} to continue: ");
        io::stdout().flush()?;
        let mut answer = String::new();
        io::stdin().read_line(&mut answer)?;
        if answer.trim() != token {
            bail!("remote PostgreSQL confirmation did not match; no changes were made");
        }
    }
    Ok(())
}

pub(crate) fn confirm_delete(id: &str) -> Result<()> {
    if !io::stdin().is_terminal() {
        bail!("delete confirmation requires a terminal; pass --yes");
    }
    print!("Permanently delete checkpoint {id}? [y/N] ");
    io::stdout().flush()?;
    let mut answer = String::new();
    io::stdin().read_line(&mut answer)?;
    if !matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes") {
        bail!("delete cancelled");
    }
    Ok(())
}
