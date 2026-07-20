use std::{collections::BTreeMap, path::Path, time::Duration};

use anyhow::{Context, Result, bail};
use rusqlite::{Connection, OpenFlags, backup::Backup};

use crate::{
    adapters::{Adapter, require_object},
    filesystem::SqliteCandidate,
    model::ServiceArtifact,
    store::Store,
};

#[derive(PartialEq, Eq)]
struct FileIdentity {
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
    #[cfg(not(unix))]
    canonical_path: std::path::PathBuf,
}

pub struct SqliteAdapter {
    pub candidate: SqliteCandidate,
}

impl SqliteAdapter {
    pub fn from_artifact(root: &Path, root_id: &str, relative_path: &Path) -> Self {
        Self {
            candidate: SqliteCandidate {
                root_id: root_id.into(),
                relative_path: relative_path.into(),
                live_path: root.join(relative_path),
            },
        }
    }

    fn open_readonly(path: &Path) -> Result<Connection> {
        Ok(Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?)
    }

    fn file_identity(path: &Path) -> Result<FileIdentity> {
        let metadata = std::fs::symlink_metadata(path)
            .with_context(|| format!("inspect SQLite database {}", path.display()))?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            bail!("SQLite database is not a regular file: {}", path.display());
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if metadata.nlink() > 1 {
                bail!(
                    "SQLite database {} has multiple hard links; safe restore cannot preserve that topology",
                    path.display()
                );
            }
            Ok(FileIdentity {
                device: metadata.dev(),
                inode: metadata.ino(),
            })
        }
        #[cfg(not(unix))]
        Ok(FileIdentity {
            canonical_path: path.canonicalize()?,
        })
    }

    fn quick_check(connection: &Connection) -> Result<()> {
        let result: String = connection.query_row("PRAGMA quick_check", [], |row| row.get(0))?;
        if result != "ok" {
            bail!("SQLite quick_check failed: {result}");
        }
        Ok(())
    }

    fn inventory(connection: &Connection) -> Result<(Vec<String>, BTreeMap<String, i64>)> {
        let mut statement = connection.prepare(
            "SELECT type, name, tbl_name, coalesce(sql, '') FROM sqlite_schema WHERE name NOT LIKE 'sqlite_%' ORDER BY type, name"
        )?;
        let schema = statement
            .query_map([], |row| {
                Ok(format!(
                    "{}|{}|{}|{}",
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let mut rows = BTreeMap::new();
        let has_stat: i64 = connection.query_row(
            "SELECT count(*) FROM sqlite_schema WHERE name='sqlite_stat1'",
            [],
            |row| row.get(0),
        )?;
        if has_stat > 0 {
            let mut statement = connection
                .prepare("SELECT tbl, stat FROM sqlite_stat1 WHERE idx IS NULL ORDER BY tbl")?;
            for row in statement.query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })? {
                let (table, stat) = row?;
                if let Some(value) = stat
                    .split_whitespace()
                    .next()
                    .and_then(|value| value.parse().ok())
                {
                    rows.insert(table, value);
                }
            }
        }
        Ok((schema, rows))
    }

    fn backup(source: &Connection, destination: &mut Connection) -> Result<()> {
        destination.busy_timeout(Duration::from_secs(5))?;
        let backup = Backup::new(source, destination)?;
        backup.run_to_completion(256, Duration::from_millis(10), None)?;
        Ok(())
    }

    pub fn verify_live_restore(&self, artifact: &ServiceArtifact, store: &Store) -> Result<()> {
        self.verify(artifact, store)?;
        let ServiceArtifact::Sqlite { object_hash, .. } = artifact else {
            bail!("wrong adapter artifact");
        };
        let before = Self::file_identity(&self.candidate.live_path)?;
        let source = Self::open_readonly(&self.candidate.live_path)?;
        Self::quick_check(&source)?;
        let temp = tempfile::NamedTempFile::new_in(store.path().join("transactions"))?;
        let mut destination = Connection::open(temp.path())?;
        Self::backup(&source, &mut destination)?;
        Self::quick_check(&destination)?;
        drop(destination);
        if before != Self::file_identity(&self.candidate.live_path)? {
            bail!("SQLite database path changed identity during verification");
        }
        if crate::store::hash_file(temp.path())? != *object_hash {
            bail!("live SQLite content does not match the checkpoint");
        }
        Ok(())
    }
}

impl Adapter for SqliteAdapter {
    fn preflight(&self) -> Result<()> {
        let _identity = Self::file_identity(&self.candidate.live_path)?;
        let connection = Self::open_readonly(&self.candidate.live_path)?;
        Self::quick_check(&connection)
    }
    fn snapshot(&self, store: &Store) -> Result<ServiceArtifact> {
        self.preflight()?;
        let before = Self::file_identity(&self.candidate.live_path)?;
        let source = Self::open_readonly(&self.candidate.live_path)?;
        let temp = tempfile::NamedTempFile::new_in(store.path().join("transactions"))?;
        let mut destination = Connection::open(temp.path())?;
        Self::backup(&source, &mut destination)?;
        Self::quick_check(&destination)?;
        let (schema, estimated_rows) = Self::inventory(&destination)?;
        drop(destination);
        if before != Self::file_identity(&self.candidate.live_path)? {
            bail!("SQLite database path changed identity during checkpoint capture");
        }
        let (object_hash, _) = store.put_object(temp.path(), None)?;
        Ok(ServiceArtifact::Sqlite {
            root_id: self.candidate.root_id.clone(),
            path: self.candidate.relative_path.clone(),
            object_hash,
            schema,
            estimated_rows,
        })
    }
    fn verify(&self, artifact: &ServiceArtifact, store: &Store) -> Result<()> {
        require_object(artifact, store)?;
        let ServiceArtifact::Sqlite { object_hash, .. } = artifact else {
            bail!("wrong adapter artifact");
        };
        let connection = Self::open_readonly(&store.object_path(object_hash))?;
        Self::quick_check(&connection)
    }
    fn restore(&self, artifact: &ServiceArtifact, store: &Store) -> Result<()> {
        self.verify(artifact, store)?;
        let ServiceArtifact::Sqlite { object_hash, .. } = artifact else {
            bail!("wrong adapter artifact");
        };
        let before = Self::file_identity(&self.candidate.live_path)?;
        let source = Self::open_readonly(&store.object_path(object_hash))?;
        let mut destination = Connection::open(&self.candidate.live_path).with_context(|| {
            format!(
                "open SQLite destination {}",
                self.candidate.live_path.display()
            )
        })?;
        Self::backup(&source, &mut destination).context("obtain SQLite write lock and restore")?;
        Self::quick_check(&destination)?;
        if before != Self::file_identity(&self.candidate.live_path)? {
            bail!("SQLite database path changed identity during restore");
        }
        Ok(())
    }
}
