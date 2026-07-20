use super::*;
use savestate::{CheckpointSelector, SavestateError};

#[test]
fn structured_api_preserves_checkpoint_lookup_errors() -> Result<()> {
    let project = tempdir()?;
    let mut app = App::open(project.path().to_path_buf())?;

    let missing = CheckpointSelector::parse("missing")?;
    let Err(error) = app.plan_restore(&missing) else {
        panic!("missing selector unexpectedly produced a restore plan");
    };
    assert!(matches!(error, SavestateError::MissingCheckpoint(_)));

    fs::write(project.path().join("value"), "one")?;
    app.create(None, false)?;
    fs::write(project.path().join("value"), "two")?;
    app.create(None, false)?;

    let ids = Store::open(project.path().join(".savestate"))?.ids()?;
    let common = ids[0]
        .bytes()
        .zip(ids[1].bytes())
        .take_while(|(left, right)| left == right)
        .count();
    assert!(common > 0, "contemporaneous ULIDs should share a prefix");
    let ambiguous = CheckpointSelector::parse(&ids[0][..common])?;
    let Err(error) = app.plan_restore(&ambiguous) else {
        panic!("ambiguous selector unexpectedly produced a restore plan");
    };
    assert!(matches!(error, SavestateError::AmbiguousCheckpoint(_)));
    Ok(())
}
