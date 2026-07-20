use std::io::IsTerminal;

use chrono::Local;
use console::{Alignment, Key, Term, measure_text_width, pad_str, style, truncate_str};
use fuzzy_matcher::{FuzzyMatcher, skim::SkimMatcherV2};

use crate::model::{CheckpointKind, SnapshotManifest};

const SEARCH_SEPARATOR: char = '\u{1f}';
const PICKER_ROWS: usize = 12;
const FALLBACK_COLUMNS: usize = 80;
// The visible selector is two columns wide. Keep two additional columns unused so terminals do
// not soft-wrap a row at the right edge and make cursor accounting ambiguous.
const SELECTOR_PREFIX_COLUMNS: usize = 4;

#[derive(Clone, Debug)]
pub(crate) struct CheckpointChoice {
    pub id: String,
    item: String,
}

impl CheckpointChoice {
    pub(crate) fn from_manifest(manifest: &SnapshotManifest, columns: usize) -> Self {
        let visible = picker_row(manifest, columns);
        let item = format!("{visible}{SEARCH_SEPARATOR}{}", manifest.id);
        Self {
            id: manifest.id.clone(),
            item,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RestoreDecision {
    Restore,
    Back,
    Cancel,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct RestoreSummary {
    pub added: usize,
    pub removed: usize,
    pub modified: usize,
}

pub(crate) trait RestorePrompter {
    fn choose_checkpoint(&mut self, choices: &[CheckpointChoice]) -> anyhow::Result<Option<usize>>;
    fn choose_action(&mut self, summary: RestoreSummary) -> anyhow::Result<RestoreDecision>;
    fn columns(&self) -> usize {
        FALLBACK_COLUMNS
    }
}

pub(crate) struct TerminalRestorePrompter {
    term: Term,
}

struct InlinePicker<'a> {
    term: &'a Term,
    rows: usize,
    active: bool,
}

#[derive(Debug, Default, PartialEq, Eq)]
struct PickerState {
    search: String,
    selected: usize,
    starting_row: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PickerEvent {
    Continue,
    Select(usize),
    Cancel,
}

impl PickerState {
    fn normalize(&mut self, matches: usize, visible_rows: usize) {
        if matches == 0 {
            self.selected = 0;
            self.starting_row = 0;
            return;
        }
        self.selected = self.selected.min(matches - 1);
        if self.selected < self.starting_row {
            self.starting_row = self.selected;
        } else if self.selected >= self.starting_row + visible_rows {
            self.starting_row = self.selected + 1 - visible_rows;
        }
    }

    fn handle_key(
        &mut self,
        key: Key,
        filtered: &[(usize, i64)],
        visible_rows: usize,
    ) -> PickerEvent {
        match key {
            Key::ArrowUp | Key::BackTab if !filtered.is_empty() => {
                self.selected = if self.selected == 0 {
                    filtered.len() - 1
                } else {
                    self.selected - 1
                };
            }
            Key::ArrowDown | Key::Tab if !filtered.is_empty() => {
                self.selected = (self.selected + 1) % filtered.len();
            }
            Key::PageUp if !filtered.is_empty() => {
                self.selected = self.selected.saturating_sub(visible_rows);
            }
            Key::PageDown if !filtered.is_empty() => {
                self.selected = (self.selected + visible_rows).min(filtered.len() - 1);
            }
            Key::Home if !filtered.is_empty() => self.selected = 0,
            Key::End if !filtered.is_empty() => self.selected = filtered.len() - 1,
            Key::Enter if !filtered.is_empty() => {
                return PickerEvent::Select(filtered[self.selected].0);
            }
            // 'q' is a valid Crockford base32 digit in checkpoint ULIDs, so it must reach the
            // search filter; only Esc and Ctrl-C cancel the picker.
            Key::Escape | Key::CtrlC => return PickerEvent::Cancel,
            Key::Backspace => {
                self.search.pop();
                self.selected = 0;
                self.starting_row = 0;
            }
            Key::Char(character) if !character.is_ascii_control() => {
                self.search.push(character);
                self.selected = 0;
                self.starting_row = 0;
            }
            _ => {}
        }
        PickerEvent::Continue
    }
}

impl<'a> InlinePicker<'a> {
    fn reserve(term: &'a Term, rows: usize) -> std::io::Result<Self> {
        #[cfg(unix)]
        crate::terminal_signals::install_cursor_restore_signal_handlers();
        term.hide_cursor()?;
        if let Err(error) = term
            .write_str(&"\r\n".repeat(rows))
            .and_then(|()| term.flush())
        {
            let _ = term.show_cursor();
            return Err(error);
        }
        Ok(Self {
            term,
            rows,
            active: true,
        })
    }

    fn render(&self, prompt: &str, items: &[&str], selected: usize) -> std::io::Result<()> {
        self.term.move_cursor_up(self.rows)?;
        let width = usize::from(self.term.size().1).saturating_sub(2).max(1);

        for row in 0..self.rows {
            self.term.clear_line()?;
            if row == 0 {
                self.term.write_str(&bounded(prompt, width))?;
            } else if let Some(item) = items.get(row - 1) {
                let prefix = if row - 1 == selected {
                    format!("{} ", style("❯").green().bold())
                } else {
                    "  ".to_owned()
                };
                let available = width.saturating_sub(2).max(1);
                self.term
                    .write_str(&format!("{prefix}{}", bounded(item, available)))?;
            }
            self.term.write_str("\r\n")?;
        }
        self.term.flush()
    }

    fn finish(&mut self) -> std::io::Result<()> {
        if self.active {
            self.active = false;
            let clear_result = self.term.clear_last_lines(self.rows);
            let cursor_result = self.term.show_cursor();
            let flush_result = self.term.flush();
            clear_result?;
            cursor_result?;
            flush_result?;
        }
        Ok(())
    }
}

impl Drop for InlinePicker<'_> {
    fn drop(&mut self) {
        if self.active {
            let _ = self.term.clear_last_lines(self.rows);
            let _ = self.term.show_cursor();
        }
        let _ = self.term.flush();
    }
}

impl TerminalRestorePrompter {
    pub(crate) fn new() -> Self {
        Self {
            term: Term::stderr(),
        }
    }

    pub(crate) fn is_available() -> bool {
        std::io::stdin().is_terminal() && Term::stderr().is_term()
    }
}

impl RestorePrompter for TerminalRestorePrompter {
    fn choose_checkpoint(&mut self, choices: &[CheckpointChoice]) -> anyhow::Result<Option<usize>> {
        let terminal_rows = usize::from(self.term.size().0);
        let visible_rows = PICKER_ROWS
            .min(choices.len().max(1))
            .min(terminal_rows.saturating_sub(2).max(1));
        let mut picker = InlinePicker::reserve(&self.term, visible_rows + 1)?;
        let matcher = SkimMatcherV2::default();
        let mut state = PickerState::default();

        loop {
            let filtered = filtered_choices(choices, &state.search, &matcher);
            state.normalize(filtered.len(), visible_rows);

            let visible = if filtered.is_empty() {
                vec!["(no matching checkpoints)"]
            } else {
                filtered
                    .iter()
                    .skip(state.starting_row)
                    .take(visible_rows)
                    .map(|(index, _)| visible_item(&choices[*index].item))
                    .collect::<Vec<_>>()
            };
            let prompt = checkpoint_prompt(&state.search, filtered.len(), choices.len());
            picker.render(
                &prompt,
                &visible,
                state.selected.saturating_sub(state.starting_row),
            )?;

            match state.handle_key(self.term.read_key()?, &filtered, visible_rows) {
                PickerEvent::Select(choice) => {
                    picker.finish()?;
                    return Ok(Some(choice));
                }
                PickerEvent::Cancel => {
                    picker.finish()?;
                    return Ok(None);
                }
                PickerEvent::Continue => {}
            }
        }
    }

    fn choose_action(&mut self, summary: RestoreSummary) -> anyhow::Result<RestoreDecision> {
        let actions = ["Restore", "Choose another checkpoint", "Cancel"];
        let mut selected = 2;
        let mut picker = InlinePicker::reserve(&self.term, actions.len() + 1)?;

        loop {
            picker.render(&action_prompt(summary), &actions, selected)?;
            if let Some(decision) = handle_action_key(&mut selected, self.term.read_key()?) {
                picker.finish()?;
                return Ok(decision);
            }
        }
    }

    fn columns(&self) -> usize {
        usize::from(self.term.size().1).max(1)
    }
}

fn checkpoint_prompt(search: &str, matches: usize, total: usize) -> String {
    if search.is_empty() {
        format!(
            "{} Choose a checkpoint · {total} total · type to filter · Esc cancel › ",
            style("?").yellow().bold()
        )
    } else if matches == 0 {
        format!(
            "{} 0/{total} matches · Backspace to clear › {search}",
            style("?").yellow().bold()
        )
    } else {
        format!(
            "{} {matches}/{total} matches › {search}",
            style("?").yellow().bold()
        )
    }
}

fn action_prompt(summary: RestoreSummary) -> String {
    format!(
        "{} What next? ({} {} {})",
        style("?").yellow().bold(),
        style(format!("+{}", summary.added)).green(),
        style(format!("-{}", summary.removed)).red(),
        style(format!("~{}", summary.modified)).yellow()
    )
}

fn handle_action_key(selected: &mut usize, key: Key) -> Option<RestoreDecision> {
    const ACTIONS: usize = 3;
    match key {
        Key::ArrowUp | Key::BackTab => {
            *selected = if *selected == 0 {
                ACTIONS - 1
            } else {
                *selected - 1
            };
            None
        }
        Key::ArrowDown | Key::Tab => {
            *selected = (*selected + 1) % ACTIONS;
            None
        }
        Key::Home => {
            *selected = 0;
            None
        }
        Key::End => {
            *selected = ACTIONS - 1;
            None
        }
        Key::Enter => Some(match *selected {
            0 => RestoreDecision::Restore,
            1 => RestoreDecision::Back,
            _ => RestoreDecision::Cancel,
        }),
        Key::Escape | Key::CtrlC | Key::Char('q') => Some(RestoreDecision::Cancel),
        _ => None,
    }
}

fn filtered_choices(
    choices: &[CheckpointChoice],
    search: &str,
    matcher: &SkimMatcherV2,
) -> Vec<(usize, i64)> {
    if search.is_empty() {
        return choices
            .iter()
            .enumerate()
            .map(|(index, _)| (index, 0))
            .collect();
    }

    let mut matches = choices
        .iter()
        .enumerate()
        .filter_map(|(index, choice)| {
            matcher
                .fuzzy_match(&choice.item, search)
                .map(|score| (index, score))
        })
        .collect::<Vec<_>>();
    matches.sort_unstable_by(|(_, left), (_, right)| right.cmp(left));
    matches
}

fn bounded(value: &str, width: usize) -> String {
    truncate_str(value, width, "…").into_owned()
}

fn visible_item(item: &str) -> &str {
    item.split_once(SEARCH_SEPARATOR)
        .map_or(item, |(visible, _)| visible)
}

fn picker_row(manifest: &SnapshotManifest, columns: usize) -> String {
    let kind = match manifest.resolved_kind() {
        CheckpointKind::Manual => "manual",
        CheckpointKind::Recovery => "recovery",
        CheckpointKind::Run => "run",
        CheckpointKind::Agent
            if manifest
                .label
                .as_deref()
                .is_some_and(|label| label.starts_with("Codex ·")) =>
        {
            "codex"
        }
        CheckpointKind::Agent => "agent",
    };
    let badge = pad_str(&format!("[{kind}]"), 10, Alignment::Left, None).into_owned();
    let timestamp = manifest
        .created_at
        .with_timezone(&Local)
        .format("%Y-%m-%d %H:%M")
        .to_string();
    let label = display_label(manifest.label.as_deref());
    let short_id = manifest.id.get(..12).unwrap_or(&manifest.id);
    let terminal_columns = if columns == 0 {
        FALLBACK_COLUMNS
    } else {
        columns
    };
    let columns = terminal_columns
        .saturating_sub(SELECTOR_PREFIX_COLUMNS)
        .max(1);

    let row = if columns >= 48 {
        let label_width = columns - 44;
        let label = padded_label(&label, label_width);
        format!("{badge}  {timestamp}  {label}  {short_id}")
    } else if columns >= 30 {
        let label_width = columns - 26;
        let label = padded_label(&label, label_width);
        format!("{badge}  {label}  {short_id}")
    } else {
        format!("{badge} {short_id}")
    };
    if measure_text_width(&row) <= columns {
        row
    } else {
        truncate_str(&row, columns, "…").into_owned()
    }
}

fn padded_label(label: &str, width: usize) -> String {
    let label = truncate_str(label, width, "…");
    pad_str(&label, width, Alignment::Left, None).into_owned()
}

#[cfg(test)]
fn checkpoint_kind(label: Option<&str>) -> &'static str {
    match label {
        Some(label) if label.starts_with("pre-restore:") || label.starts_with("pre-resume:") => {
            "recovery"
        }
        Some(label) if label.starts_with("before:") => "run",
        Some(label) if label.starts_with("Codex ·") => "codex",
        Some(label) if label.starts_with("agent:") => "agent",
        _ => "manual",
    }
}

fn display_label(label: Option<&str>) -> String {
    let Some(label) = label else {
        return "(unlabeled)".into();
    };
    if let Some(target) = label.strip_prefix("pre-restore:") {
        return format!("Before restore to {}", short_id(target));
    }
    if let Some(target) = label.strip_prefix("pre-resume:") {
        return format!("Before resume to {}", short_id(target));
    }
    if let Some(command) = label.strip_prefix("before:") {
        return format!("Before running {command}");
    }
    if let Some(agent) = label
        .strip_prefix("agent:")
        .and_then(|label| label.strip_suffix(":session-start"))
    {
        return format!("{} session start", title_case(agent));
    }
    label.to_owned()
}

fn short_id(id: &str) -> &str {
    id.get(..12).unwrap_or(id)
}

fn title_case(value: &str) -> String {
    let mut characters = value.chars();
    match characters.next() {
        Some(first) => first.to_uppercase().chain(characters).collect(),
        None => String::new(),
    }
}

#[cfg(test)]
#[path = "restore_prompt_tests.rs"]
mod tests;
