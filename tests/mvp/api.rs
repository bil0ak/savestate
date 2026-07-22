use super::*;
use savestate::{CheckpointSelector, CreateOptions, DatabaseSelection, SavestateError};

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

#[test]
fn concurrent_if_changed_calls_report_only_the_publisher_as_created() -> Result<()> {
    let project = tempdir()?;
    fs::write(project.path().join("payload"), vec![b'x'; 1024 * 1024])?;
    let root = project.path().to_path_buf();
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(3));

    let outcomes = std::thread::scope(|scope| {
        let workers = (0..2)
            .map(|_| {
                let root = root.clone();
                let barrier = barrier.clone();
                scope.spawn(move || -> Result<_> {
                    let mut app = App::open(root)?;
                    barrier.wait();
                    Ok(app.create_checkpoint(CreateOptions {
                        label: None,
                        if_changed: true,
                        include_ignored: false,
                        databases: DatabaseSelection::FilesystemOnly,
                    })?)
                })
            })
            .collect::<Vec<_>>();
        barrier.wait();
        workers
            .into_iter()
            .map(|worker| worker.join().expect("checkpoint worker should not panic"))
            .collect::<Result<Vec<_>>>()
    })?;

    assert_eq!(outcomes.iter().filter(|outcome| outcome.created).count(), 1);
    assert_eq!(outcomes[0].checkpoint_id, outcomes[1].checkpoint_id);
    assert_eq!(
        Store::open(project.path().join(".savestate"))?.ids()?.len(),
        1
    );
    Ok(())
}
