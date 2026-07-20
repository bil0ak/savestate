use super::*;

#[test]
#[ignore = "creates the 200,000-file size-regression fixture"]
fn ignored_200k_file_tree_is_pruned_but_include_ignored_captures_it() -> Result<()> {
    let project = tempdir()?;
    git(project.path(), &["init", "-q"])?;
    fs::write(project.path().join(".gitignore"), "node_modules/\n")?;
    let modules = project.path().join("node_modules/pkg");
    fs::create_dir_all(&modules)?;
    for index in 0..200_000 {
        fs::write(modules.join(format!("{index:06}")), [])?;
    }
    fs::write(project.path().join("source"), "small")?;
    let mut app = App::open(project.path().to_path_buf())?;
    let normal = app.create(None, false)?;
    let store = Store::open(project.path().join(".savestate"))?;
    assert!(store.load(&normal)?.files.len() < 10);
    let full = app.create_with_options(None, false, true)?;
    assert!(store.load(&full)?.files.len() >= 200_000);
    Ok(())
}
