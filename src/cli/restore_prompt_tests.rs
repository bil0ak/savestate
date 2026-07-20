use std::{collections::VecDeque, fs, path::PathBuf};

use anyhow::Result;
use chrono::{TimeZone, Utc};
use tempfile::tempdir;

use super::*;
use crate::{App, model::CaptureScope};

fn manifest(label: Option<&str>) -> SnapshotManifest {
    SnapshotManifest {
        schema_version: 2,
        id: "01KXN8RKTX41D3RJNJE3SBRWQV".into(),
        created_at: Utc.with_ymd_and_hms(2026, 7, 16, 8, 4, 0).unwrap(),
        label: label.map(str::to_owned),
        kind: None,
        project_root: "/tmp/project".into(),
        platform: "test".into(),
        engine: "test".into(),
        consistency: "test".into(),
        capture_started_at: Utc::now(),
        capture_finished_at: Utc::now(),
        roots: Vec::new(),
        files: Vec::new(),
        services: Vec::new(),
        capture_scope: CaptureScope::default(),
    }
}

#[test]
fn classifies_and_humanizes_checkpoint_labels() {
    assert_eq!(checkpoint_kind(Some("Codex · Add dark mode")), "codex");
    assert_eq!(checkpoint_kind(Some("agent:claude:session-start")), "agent");
    assert_eq!(
        checkpoint_kind(Some("pre-restore:01KXN8RKTX41")),
        "recovery"
    );
    assert_eq!(checkpoint_kind(Some("pre-resume:01KXN8RKTX41")), "recovery");
    assert_eq!(checkpoint_kind(Some("before:pnpm test")), "run");
    assert_eq!(checkpoint_kind(Some("known good")), "manual");
    assert_eq!(display_label(None), "(unlabeled)");
    assert_eq!(
        display_label(Some("agent:codex:session-start")),
        "Codex session start"
    );
    assert_eq!(
        display_label(Some("pre-restore:01KXN8RKTX41D3RJNJE3SBRWQV")),
        "Before restore to 01KXN8RKTX41"
    );
    assert_eq!(
        display_label(Some("pre-resume:01KXN8RKTX41D3RJNJE3SBRWQV")),
        "Before resume to 01KXN8RKTX41"
    );
    assert_eq!(
        display_label(Some("before:pnpm test")),
        "Before running pnpm test"
    );
}

#[test]
fn picker_row_is_width_bounded_and_searches_the_full_id() {
    let manifest = manifest(Some(
        "Codex · Add a deliberately long multilingual label 日本語 without breaking characters",
    ));
    let choice = CheckpointChoice::from_manifest(&manifest, 72);
    assert!(measure_text_width(visible_item(&choice.item)) + SELECTOR_PREFIX_COLUMNS <= 72);
    assert!(visible_item(&choice.item).contains("[codex]"));
    assert!(visible_item(&choice.item).contains("01KXN8RKTX41"));
    assert!(choice.item.ends_with(&manifest.id));

    for columns in [20, 28, 30, 48, 72, 120] {
        let row = CheckpointChoice::from_manifest(&manifest, columns);
        assert!(
            measure_text_width(visible_item(&row.item)) + SELECTOR_PREFIX_COLUMNS <= columns,
            "picker row exceeded {columns} columns"
        );
    }
}

#[test]
fn inline_filter_searches_labels_badges_and_hidden_id_suffixes() {
    let codex =
        CheckpointChoice::from_manifest(&manifest(Some("Codex · Add interactive restore")), 80);
    let mut manual_manifest = manifest(Some("Known good settings"));
    manual_manifest.id = "01KXN8RKTJE2C7D6UNIQUE99999".into();
    let manual = CheckpointChoice::from_manifest(&manual_manifest, 80);
    let choices = [codex, manual];
    let matcher = SkimMatcherV2::default();

    assert_eq!(filtered_choices(&choices, "codex", &matcher)[0].0, 0);
    assert_eq!(filtered_choices(&choices, "settings", &matcher)[0].0, 1);
    assert_eq!(filtered_choices(&choices, "UNIQUE99999", &matcher)[0].0, 1);
    assert!(filtered_choices(&choices, "does-not-exist", &matcher).is_empty());
}

#[test]
fn filtered_enter_returns_the_original_choice_index() {
    let codex =
        CheckpointChoice::from_manifest(&manifest(Some("Codex · Add interactive restore")), 80);
    let mut manual_manifest = manifest(Some("Known good settings"));
    manual_manifest.id = "01KXN8RKTJE2C7D6UNIQUE99999".into();
    let manual = CheckpointChoice::from_manifest(&manual_manifest, 80);
    let choices = [codex, manual];
    let matcher = SkimMatcherV2::default();
    let mut state = PickerState::default();
    let unfiltered = filtered_choices(&choices, "", &matcher);

    for character in "settings".chars() {
        assert_eq!(
            state.handle_key(Key::Char(character), &unfiltered, PICKER_ROWS),
            PickerEvent::Continue
        );
    }
    let filtered = filtered_choices(&choices, &state.search, &matcher);
    state.normalize(filtered.len(), PICKER_ROWS);

    assert_eq!(
        state.handle_key(Key::Enter, &filtered, PICKER_ROWS),
        PickerEvent::Select(1)
    );
}

#[test]
fn empty_filter_enter_is_safe_and_cancel_keys_cancel() {
    let mut state = PickerState::default();
    assert_eq!(
        state.handle_key(Key::Enter, &[], PICKER_ROWS),
        PickerEvent::Continue
    );
    assert_eq!(
        state.handle_key(Key::Escape, &[], PICKER_ROWS),
        PickerEvent::Cancel
    );
    assert_eq!(
        state.handle_key(Key::CtrlC, &[], PICKER_ROWS),
        PickerEvent::Cancel
    );
}

#[test]
fn typing_q_filters_instead_of_cancelling() {
    let mut state = PickerState::default();
    assert_eq!(
        state.handle_key(Key::Char('q'), &[], PICKER_ROWS),
        PickerEvent::Continue
    );
    assert_eq!(state.search, "q");
    assert_eq!(
        state.handle_key(Key::Escape, &[], PICKER_ROWS),
        PickerEvent::Cancel
    );
}

#[test]
fn action_keys_are_safe_by_default_and_support_navigation() {
    let mut selected = 2;
    assert_eq!(
        handle_action_key(&mut selected, Key::Escape),
        Some(RestoreDecision::Cancel)
    );
    assert_eq!(
        handle_action_key(&mut selected, Key::CtrlC),
        Some(RestoreDecision::Cancel)
    );
    assert_eq!(handle_action_key(&mut selected, Key::ArrowUp), None);
    assert_eq!(selected, 1);
    assert_eq!(
        handle_action_key(&mut selected, Key::Enter),
        Some(RestoreDecision::Back)
    );
}

#[test]
fn prompts_show_match_and_restore_counts() {
    assert!(checkpoint_prompt("", 40, 40).contains("40 total"));
    assert!(checkpoint_prompt("dark", 3, 40).contains("3/40 matches"));
    assert!(checkpoint_prompt("missing", 0, 40).contains("Backspace to clear"));

    let prompt = action_prompt(RestoreSummary {
        added: 2,
        removed: 1,
        modified: 4,
    });
    assert!(prompt.contains("+2"));
    assert!(prompt.contains("-1"));
    assert!(prompt.contains("~4"));
}

struct ScriptedPrompter {
    selections: VecDeque<Option<usize>>,
    decisions: VecDeque<RestoreDecision>,
}

impl RestorePrompter for ScriptedPrompter {
    fn choose_checkpoint(&mut self, _choices: &[CheckpointChoice]) -> Result<Option<usize>> {
        Ok(self
            .selections
            .pop_front()
            .expect("scripted checkpoint selection"))
    }

    fn choose_action(&mut self, _summary: RestoreSummary) -> Result<RestoreDecision> {
        Ok(self
            .decisions
            .pop_front()
            .expect("scripted restore decision"))
    }
}

fn project() -> Result<(tempfile::TempDir, App)> {
    let project = tempdir()?;
    fs::write(project.path().join(".savestate.toml"), "")?;
    let app = App::open(project.path().to_path_buf())?;
    Ok((project, app))
}

#[test]
fn interactive_cancel_does_not_create_recovery_state_or_mutate_files() -> Result<()> {
    let (project, mut app) = project()?;
    fs::write(project.path().join("value"), "checkpoint")?;
    app.create(Some("baseline".into()), false)?;
    fs::write(project.path().join("value"), "current")?;
    let before_ids = app.store.ids()?;

    app.restore_interactive_with(
        false,
        &mut ScriptedPrompter {
            selections: VecDeque::from([Some(0)]),
            decisions: VecDeque::from([RestoreDecision::Cancel]),
        },
    )?;

    assert_eq!(fs::read_to_string(project.path().join("value"))?, "current");
    assert_eq!(app.store.ids()?, before_ids);
    assert!(app.store.journal()?.is_none());
    Ok(())
}

#[test]
fn interactive_back_can_choose_and_restore_another_checkpoint() -> Result<()> {
    let (project, mut app) = project()?;
    fs::write(project.path().join("value"), "first")?;
    let first = app.create(Some("first".into()), false)?;
    fs::write(project.path().join("value"), "second")?;
    let second = app.create(Some("second".into()), false)?;
    fs::write(project.path().join("value"), "current")?;
    let choices = app.restore_choices(100)?;
    let first_index = choices
        .iter()
        .position(|choice| choice.id == first)
        .expect("first checkpoint choice");
    let second_index = choices
        .iter()
        .position(|choice| choice.id == second)
        .expect("second checkpoint choice");

    app.restore_interactive_with(
        false,
        &mut ScriptedPrompter {
            selections: VecDeque::from([Some(second_index), Some(first_index)]),
            decisions: VecDeque::from([RestoreDecision::Back, RestoreDecision::Restore]),
        },
    )?;

    assert_eq!(fs::read_to_string(project.path().join("value"))?, "first");
    assert!(app.store.journal()?.is_none());
    Ok(())
}

#[test]
fn interactive_dry_run_stops_after_preview() -> Result<()> {
    let (project, mut app) = project()?;
    fs::write(project.path().join("value"), "checkpoint")?;
    app.create(Some("baseline".into()), false)?;
    fs::write(project.path().join("value"), "current")?;
    let before_ids = app.store.ids()?;

    app.restore_interactive_with(
        true,
        &mut ScriptedPrompter {
            selections: VecDeque::from([Some(0)]),
            decisions: VecDeque::new(),
        },
    )?;

    assert_eq!(fs::read_to_string(project.path().join("value"))?, "current");
    assert_eq!(app.store.ids()?, before_ids);
    assert!(app.store.journal()?.is_none());
    Ok(())
}

struct MutatingPrompter {
    path: PathBuf,
    selections: VecDeque<Option<usize>>,
    actions: usize,
}

impl RestorePrompter for MutatingPrompter {
    fn choose_checkpoint(&mut self, _choices: &[CheckpointChoice]) -> Result<Option<usize>> {
        Ok(self.selections.pop_front().expect("selection"))
    }

    fn choose_action(&mut self, _summary: RestoreSummary) -> Result<RestoreDecision> {
        if self.actions == 0 {
            fs::create_dir_all(self.path.parent().expect("ignored path parent"))?;
            fs::write(&self.path, "created after preview")?;
        }
        self.actions += 1;
        Ok(RestoreDecision::Restore)
    }
}

#[test]
fn state_created_between_preview_and_confirmation_is_replanned_and_preserved() -> Result<()> {
    let (project, mut app) = project()?;
    fs::write(project.path().join(".gitignore"), "late-cache/\n")?;
    let status = std::process::Command::new("git")
        .current_dir(project.path())
        .args(["init", "-q"])
        .status()?;
    assert!(status.success());
    fs::write(project.path().join("value"), "checkpoint")?;
    app.create(Some("baseline".into()), false)?;
    fs::write(project.path().join("value"), "current")?;

    app.restore_interactive_with(
        false,
        &mut MutatingPrompter {
            path: project.path().join("late-cache/state"),
            selections: VecDeque::from([Some(0), Some(0)]),
            actions: 0,
        },
    )?;

    assert_eq!(
        fs::read_to_string(project.path().join("value"))?,
        "checkpoint"
    );
    assert_eq!(
        fs::read_to_string(project.path().join("late-cache/state"))?,
        "created after preview"
    );
    Ok(())
}
