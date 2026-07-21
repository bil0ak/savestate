use super::*;

impl Store {
    pub fn put_object(
        &self,
        source: &Path,
        expected_hash: Option<&str>,
    ) -> Result<(String, String)> {
        match self.put_object_retryable(source, expected_hash)? {
            ObjectWrite::Stored(stored) => Ok(stored),
            ObjectWrite::SourceChanged => {
                bail!("source changed while being captured: {}", source.display())
            }
        }
    }

    pub(crate) fn put_object_retryable(
        &self,
        source: &Path,
        expected_hash: Option<&str>,
    ) -> Result<ObjectWrite> {
        let hash = match expected_hash {
            Some(hash) => hash.to_owned(),
            None => hash_file(source)?,
        };
        if !valid_hash(&hash) {
            bail!("invalid expected object hash {hash:?}");
        }
        let destination = self.object_path(&hash);
        if path_exists(&destination)? {
            ensure_regular_file(&destination, "content object")?;
            if hash_file(&destination)? != hash {
                bail!("existing content object {hash} is corrupt");
            }
            return Ok(ObjectWrite::Stored((hash, "deduplicated".into())));
        }
        let parent = destination
            .parent()
            .context("object destination has no parent directory")?;
        ensure_real_directory(parent)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(parent, fs::Permissions::from_mode(0o700))?;
        }
        let temporary = parent.join(format!(".{}.tmp-{}", hash, std::process::id()));
        if path_exists(&temporary)? {
            // This process is the only live owner of its PID, so a file with
            // this name is debris from a crashed capture.
            ensure_regular_file(&temporary, "object staging file")?;
            fs::remove_file(&temporary)?;
        }
        let staging = StagingGuard {
            path: temporary.clone(),
        };
        let mut engine = platform::clone_or_copy(source, &temporary)
            .with_context(|| format!("store object for {}", source.display()))?;
        if hash_file(&temporary)? != hash {
            return Ok(ObjectWrite::SourceChanged);
        }
        platform::sync_file(&temporary)?;
        if let Err(error) = fs::rename(&temporary, &destination) {
            if path_exists(&destination)? {
                ensure_regular_file(&destination, "content object")?;
                if hash_file(&destination)? != hash {
                    return Err(error).context("content object destination is corrupt");
                }
                engine = "deduplicated";
            } else {
                return Err(error.into());
            }
        }
        drop(staging);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&destination, fs::Permissions::from_mode(0o600))?;
        }
        platform::sync_file(&destination)?;
        sync_directory(parent)?;
        Ok(ObjectWrite::Stored((hash, engine.into())))
    }

    pub fn verify_object(&self, hash: &str) -> Result<()> {
        if !valid_hash(hash) {
            bail!("invalid object hash {hash:?}");
        }
        let path = self.object_path(hash);
        if !path.exists() {
            bail!("missing object {hash}");
        }
        ensure_regular_file(&path, "content object")?;
        if hash_file(&path)? != hash {
            bail!("object {hash} checksum mismatch");
        }
        Ok(())
    }
}
