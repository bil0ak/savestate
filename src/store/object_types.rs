use super::*;

/// Removes an object staging file on drop unless a successful rename already
/// consumed it, so failed captures never strand debris that blocks retries.
pub(super) struct StagingGuard {
    pub(super) path: PathBuf,
}

impl Drop for StagingGuard {
    fn drop(&mut self) {
        if path_exists(&self.path).unwrap_or(false) {
            let _ = fs::remove_file(&self.path);
        }
    }
}

pub(crate) enum ObjectWrite {
    Stored((String, String)),
    SourceChanged,
}

pub fn hash_file(path: &Path) -> Result<String> {
    let mut file = File::open(path)?;
    let mut hasher = blake3::Hasher::new();
    let mut buffer = vec![0u8; 1024 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hasher.finalize().to_hex().to_string())
}
