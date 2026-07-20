use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};

use crate::model::{RetentionPolicy, validate_scope_patterns};

mod sample;
mod schema;

pub use sample::sample;
pub use schema::{
    Config, ExperimentalConfig, LimitsConfig, ManualRetention, PostgresConfig, RetentionConfig,
    SqliteConfig,
};

impl Config {
    #[allow(clippy::too_many_lines)] // Deserialization, normalization, and validation fail as one boundary.
    pub fn load(root: &Path) -> Result<Self> {
        let path = root.join(".savestate.toml");
        let mut config = match fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => {
                toml::from_str(
                    &fs::read_to_string(&path)
                        .with_context(|| format!("read {}", path.display()))?,
                )
                .with_context(|| format!("parse {}", path.display()))?
            }
            Ok(_) => bail!(
                "project configuration is not a regular file: {}",
                path.display()
            ),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Self::default(),
            Err(error) => return Err(error.into()),
        };
        // Preserve the original configuration's retention behavior without letting it
        // affect manual checkpoints. New projects should use `[retention]` directly.
        if config.keep_last != 20 && config.retention.agent == 20 {
            config.retention.agent = config.keep_last;
        }
        if config.retention.agent == 0
            || config.retention.recovery == 0
            || config.retention.run == 0
        {
            bail!("retention counts must be at least 1");
        }
        validate_scope_patterns(&config.include, "include")?;
        validate_scope_patterns(&config.exclude, "exclude")?;
        // An unparseable limit must fail here, where the user sees it, not
        // inside every later capture (agent hooks are fail-soft and would
        // silently stop producing checkpoints).
        parse_size(&config.limits.warn_size)
            .with_context(|| format!("invalid [limits] warn_size {:?}", config.limits.warn_size))?;
        config.store = expand_home(&config.store)?;
        if config.store.is_relative() {
            config.store = root.join(&config.store);
        }
        config.store = absolute_clean(&config.store)?;
        if config.store == root || root.starts_with(&config.store) {
            bail!(
                "snapshot store {} cannot contain the project root",
                config.store.display()
            );
        }
        for path in &mut config.external_paths {
            *path = expand_home(path)?;
            if path.is_relative() {
                *path = root.join(&*path);
            }
            *path = absolute_clean(path)?;
            validate_external(path, root, &config.store)?;
        }
        for (index, path) in config.external_paths.iter().enumerate() {
            for other in config.external_paths.iter().skip(index + 1) {
                if path == other || path.starts_with(other) || other.starts_with(path) {
                    bail!(
                        "external paths {} and {} overlap",
                        path.display(),
                        other.display()
                    );
                }
            }
        }
        for sqlite in &mut config.sqlite {
            if sqlite.path.is_relative() {
                sqlite.path = root.join(&sqlite.path);
            }
            sqlite.path = absolute_clean(&sqlite.path)?;
            if sqlite.path == config.store
                || sqlite.path.starts_with(&config.store)
                || config.store.starts_with(&sqlite.path)
            {
                bail!(
                    "SQLite path {} overlaps the snapshot store",
                    sqlite.path.display()
                );
            }
            let is_captured = sqlite.path == root
                || sqlite.path.starts_with(root)
                || config
                    .external_paths
                    .iter()
                    .any(|external| sqlite.path == *external || sqlite.path.starts_with(external));
            if !is_captured {
                bail!(
                    "SQLite path {} is outside the repository and registered external roots",
                    sqlite.path.display()
                );
            }
        }
        for (index, sqlite) in config.sqlite.iter().enumerate() {
            if config
                .sqlite
                .iter()
                .skip(index + 1)
                .any(|other| other.path == sqlite.path)
            {
                bail!(
                    "SQLite path {} is configured more than once",
                    sqlite.path.display()
                );
            }
        }
        for (index, postgres) in config.postgres.iter().enumerate() {
            validate_postgres(postgres)?;
            for other in config.postgres.iter().skip(index + 1) {
                if postgres.name == other.name {
                    bail!(
                        "PostgreSQL adapter name {:?} is configured more than once",
                        postgres.name
                    );
                }
                if postgres.url_env == other.url_env {
                    bail!(
                        "PostgreSQL connection variable {:?} is configured more than once",
                        postgres.url_env
                    );
                }
            }
        }
        Ok(config)
    }

    pub fn warn_size_bytes(&self) -> Result<u64> {
        parse_size(&self.limits.warn_size)
    }

    pub(crate) fn retention_policy(&self) -> RetentionPolicy {
        RetentionPolicy {
            agent: self.retention.agent,
            recovery: self.retention.recovery,
            run: self.retention.run,
        }
    }

    pub fn project_store(&self, root: &Path) -> Result<PathBuf> {
        if self.store.starts_with(root) {
            return Ok(self.store.clone());
        }
        let snapshots = self.store.join("snapshots");
        let snapshots_exist = match fs::symlink_metadata(&snapshots) {
            Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => true,
            Ok(_) => bail!(
                "legacy global store snapshots path is not a real directory: {}",
                snapshots.display()
            ),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
            Err(error) => return Err(error.into()),
        };
        if snapshots_exist {
            let mut owners = std::collections::BTreeSet::new();
            for entry in fs::read_dir(&snapshots)? {
                let entry = entry?;
                let path = entry.path();
                if entry.file_name().to_string_lossy().starts_with('.') {
                    continue;
                }
                let metadata = fs::symlink_metadata(&path)?;
                if metadata.file_type().is_symlink() || !metadata.is_dir() {
                    bail!("legacy global store contains an invalid snapshot entry");
                }
                for file in ["complete", "manifest.json", "manifest.blake3"] {
                    let file = path.join(file);
                    let metadata = fs::symlink_metadata(&file).with_context(|| {
                        format!(
                            "legacy global store checkpoint is incomplete: {}",
                            path.display()
                        )
                    })?;
                    if metadata.file_type().is_symlink() || !metadata.is_file() {
                        bail!("legacy global store checkpoint contains invalid metadata");
                    }
                }
                let bytes = fs::read(path.join("manifest.json"))?;
                let expected = fs::read_to_string(path.join("manifest.blake3"))?;
                if blake3::hash(&bytes).to_hex().as_str() != expected.trim() {
                    bail!("legacy global store contains a corrupt manifest");
                }
                let value: serde_json::Value = serde_json::from_slice(&bytes)?;
                let owner = value
                    .get("project_root")
                    .and_then(serde_json::Value::as_str)
                    .context("legacy global store manifest has no project root")?;
                owners.insert(PathBuf::from(owner));
            }
            if owners.contains(root) {
                if owners.len() != 1 {
                    bail!("legacy global store mixes checkpoints from multiple projects");
                }
                return Ok(self.store.clone());
            }
        }
        let namespace = &blake3::hash(root.to_string_lossy().as_bytes()).to_hex()[..32];
        Ok(self.store.join("projects").join(namespace))
    }
}

fn parse_size(value: &str) -> Result<u64> {
    let value = value.trim();
    let split = value
        .find(|character: char| !character.is_ascii_digit())
        .unwrap_or(value.len());
    let number: u64 = value[..split]
        .parse()
        .with_context(|| format!("invalid size {value:?}"))?;
    let multiplier = match value[split..].trim().to_ascii_lowercase().as_str() {
        "" | "b" => 1,
        "kib" => 1024,
        "mib" => 1024u64.pow(2),
        "gib" => 1024u64.pow(3),
        "tib" => 1024u64.pow(4),
        "kb" => 1000,
        "mb" => 1000u64.pow(2),
        "gb" => 1000u64.pow(3),
        "tb" => 1000u64.pow(4),
        _ => bail!("invalid size unit in {value:?}"),
    };
    number
        .checked_mul(multiplier)
        .with_context(|| format!("size {value:?} is too large"))
}

/// Expands a leading `~` component to the user's home directory so
/// `store = "~/backups"` names the home directory rather than a literal
/// project-relative `~` folder.
fn expand_home(path: &Path) -> Result<PathBuf> {
    let mut components = path.components();
    match components.next() {
        Some(std::path::Component::Normal(first)) if first == "~" => {
            let home = std::env::var_os("HOME")
                .or_else(|| std::env::var_os("USERPROFILE"))
                .context("configured path uses ~, but no home directory is set")?;
            Ok(PathBuf::from(home).join(components.as_path()))
        }
        _ => Ok(path.to_path_buf()),
    }
}

fn absolute_clean(path: &Path) -> Result<PathBuf> {
    if path_exists(path)? {
        Ok(path
            .canonicalize()
            .with_context(|| format!("canonicalize {}", path.display()))?)
    } else {
        let absolute = if path.is_absolute() {
            path.to_path_buf()
        } else {
            std::env::current_dir()?.join(path)
        };
        let mut ancestor = None;
        for candidate in absolute.ancestors() {
            if path_exists(candidate)? {
                ancestor = Some(candidate);
                break;
            }
        }
        let ancestor = ancestor.context("path has no existing ancestor")?;
        let mut clean = ancestor
            .canonicalize()
            .with_context(|| format!("canonicalize {}", ancestor.display()))?;
        let suffix = absolute.strip_prefix(ancestor)?;
        for component in suffix.components() {
            match component {
                std::path::Component::CurDir => {}
                std::path::Component::ParentDir => {
                    clean.pop();
                }
                std::path::Component::Normal(value) => clean.push(value),
                std::path::Component::Prefix(_) | std::path::Component::RootDir => {
                    bail!("invalid absolute path suffix {}", suffix.display())
                }
            }
        }
        Ok(clean)
    }
}

fn path_exists(path: &Path) -> Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.into()),
    }
}

fn validate_postgres(config: &PostgresConfig) -> Result<()> {
    if config.name.trim().is_empty() {
        bail!("PostgreSQL adapter name cannot be empty");
    }
    if config.maintenance_db.trim().is_empty() {
        bail!("PostgreSQL maintenance database cannot be empty");
    }
    if config.allow_remote_restore && !config.allow_restore {
        bail!("allow_remote_restore requires allow_restore = true");
    }
    let mut characters = config.url_env.chars();
    let valid_start = characters
        .next()
        .is_some_and(|character| character == '_' || character.is_ascii_alphabetic());
    if !valid_start
        || !characters.all(|character| character == '_' || character.is_ascii_alphanumeric())
    {
        bail!(
            "PostgreSQL connection variable {:?} is not a valid environment variable name",
            config.url_env
        );
    }
    Ok(())
}

fn validate_external(path: &Path, root: &Path, store: &Path) -> Result<()> {
    if path == Path::new("/") || path.parent().is_none() {
        bail!("refusing unsafe external path {}", path.display());
    }
    if path == root || path.starts_with(root) {
        bail!("external path {} overlaps the repository", path.display());
    }
    if path == store || path.starts_with(store) || store.starts_with(path) {
        bail!(
            "external path {} overlaps the snapshot store",
            path.display()
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn toml_path(path: &Path) -> String {
        toml::Value::String(path.to_string_lossy().into_owned()).to_string()
    }

    #[test]
    fn overlapping_external_roots_are_refused() -> Result<()> {
        let sandbox = tempdir()?;
        let root = sandbox.path().join("project");
        let external = sandbox.path().join("external");
        fs::create_dir_all(&root)?;
        fs::create_dir_all(external.join("nested"))?;
        fs::write(
            root.join(".savestate.toml"),
            format!(
                "external_paths = [{}, {}]\n",
                toml_path(&external),
                toml_path(&external.join("nested"))
            ),
        )?;

        let error = Config::load(&root).unwrap_err().to_string();
        assert!(error.contains("overlap"), "{error}");
        Ok(())
    }

    #[test]
    fn sqlite_must_belong_to_a_captured_root() -> Result<()> {
        let sandbox = tempdir()?;
        let root = sandbox.path().join("project");
        fs::create_dir_all(&root)?;
        let database = sandbox.path().join("outside.sqlite");
        fs::write(&database, "not opened during config validation")?;
        fs::write(
            root.join(".savestate.toml"),
            format!("[[sqlite]]\npath = {}\n", toml_path(&database)),
        )?;

        let error = Config::load(&root).unwrap_err().to_string();
        assert!(error.contains("outside the repository"), "{error}");
        Ok(())
    }

    #[test]
    fn duplicate_postgres_targets_are_refused() -> Result<()> {
        let root = tempdir()?;
        fs::write(
            root.path().join(".savestate.toml"),
            "[[postgres]]\nname = \"one\"\nurl_env = \"DATABASE_URL\"\n\n\
             [[postgres]]\nname = \"two\"\nurl_env = \"DATABASE_URL\"\n",
        )?;

        let error = Config::load(root.path()).unwrap_err().to_string();
        assert!(error.contains("configured more than once"), "{error}");
        Ok(())
    }

    #[test]
    fn store_tilde_expands_to_the_home_directory() -> Result<()> {
        let root = tempdir()?;
        fs::write(
            root.path().join(".savestate.toml"),
            "store = \"~/stores\"\n",
        )?;

        let config = Config::load(root.path())?;
        let home = PathBuf::from(
            std::env::var_os("HOME")
                .or_else(|| std::env::var_os("USERPROFILE"))
                .unwrap(),
        );
        assert!(
            config.store.starts_with(home.canonicalize()?),
            "{}",
            config.store.display()
        );
        assert!(config.store.ends_with("stores"));
        Ok(())
    }

    #[test]
    fn invalid_warn_size_fails_at_config_load() -> Result<()> {
        let root = tempdir()?;
        fs::write(
            root.path().join(".savestate.toml"),
            "[limits]\nwarn_size = \"2 gigs\"\n",
        )?;

        let error = Config::load(root.path()).unwrap_err().to_string();
        assert!(error.contains("warn_size"), "{error}");
        Ok(())
    }

    #[test]
    fn scope_patterns_cannot_escape_their_filesystem_root() -> Result<()> {
        let root = tempdir()?;
        fs::write(
            root.path().join(".savestate.toml"),
            "include = [\"../secret\"]\n",
        )?;

        let error = Config::load(root.path()).unwrap_err().to_string();
        assert!(error.contains("stay relative"), "{error}");
        Ok(())
    }
}
