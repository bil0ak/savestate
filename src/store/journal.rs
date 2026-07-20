use super::*;

impl Store {
    pub fn journal(&self) -> Result<Option<RestoreJournal>> {
        let path = self.root.join("journal.json");
        match fs::symlink_metadata(&path) {
            Ok(_) => ensure_regular_file(&path, "restore journal")?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        }
        Ok(Some(serde_json::from_slice(&fs::read(path)?)?))
    }

    pub fn write_journal(&self, journal: &RestoreJournal) -> Result<()> {
        atomic_write(
            &self.root.join("journal.json"),
            &serde_json::to_vec_pretty(journal)?,
        )
    }

    pub fn clear_journal(&self) -> Result<()> {
        let path = self.root.join("journal.json");
        if path_exists(&path)? {
            ensure_regular_file(&path, "restore journal")?;
            fs::remove_file(path)?;
            sync_directory(&self.root)?;
        }
        Ok(())
    }
}
