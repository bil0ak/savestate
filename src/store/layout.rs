use super::*;

impl Store {
    /// Returns the validated store root for diagnostics and controlled tooling.
    pub fn path(&self) -> &Path {
        &self.root
    }

    pub fn open(root: PathBuf) -> Result<Self> {
        let existing = root
            .ancestors()
            .find(|path| path.exists())
            .context("snapshot store path has no existing ancestor")?;
        let suffix = root.strip_prefix(existing)?;
        let root = existing.canonicalize()?.join(suffix);
        Self::open_strict(root)
    }

    pub(crate) fn open_strict(root: PathBuf) -> Result<Self> {
        if !root.is_absolute() {
            bail!("snapshot store path must be absolute");
        }
        let existing = root
            .ancestors()
            .find(|path| path.exists())
            .context("snapshot store path has no existing ancestor")?;
        if existing.canonicalize()? != existing {
            bail!(
                "snapshot store path descends through a symlink: {}",
                root.display()
            );
        }
        ensure_real_directory(&root)?;
        let directories = [
            "snapshots",
            "objects",
            "transactions",
            "labels",
            "pins",
            "hooks",
            "hooks/codex",
        ];
        for directory in directories {
            ensure_real_directory(&root.join(directory))?;
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&root, fs::Permissions::from_mode(0o700))?;
            for directory in directories {
                fs::set_permissions(root.join(directory), fs::Permissions::from_mode(0o700))?;
            }
        }
        Ok(Self { root })
    }

    pub fn lock(&self) -> Result<File> {
        let path = self.root.join("lock");
        if path_exists(&path)? {
            ensure_regular_file(&path, "store lock")?;
        }
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(path)?;
        file.lock_exclusive()?;
        Ok(file)
    }

    pub fn object_path(&self, hash: &str) -> PathBuf {
        self.root
            .join("objects")
            .join(hash.get(..2).unwrap_or("__invalid"))
            .join(hash)
    }

    pub fn new_checkpoint_id(&self) -> Result<String> {
        let candidate = u128::from(Ulid::new());
        let next = match self.ids()?.first() {
            Some(latest) => {
                let latest = u128::from(Ulid::from_string(latest)?);
                candidate.max(
                    latest
                        .checked_add(1)
                        .context("checkpoint ID space is exhausted")?,
                )
            }
            None => candidate,
        };
        Ok(Ulid::from(next).to_string())
    }
}
