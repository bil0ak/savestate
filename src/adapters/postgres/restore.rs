use super::*;

impl PostgresAdapter {
    pub fn plan_swap(
        &self,
        artifact: &ServiceArtifact,
        checkpoint_id: &str,
    ) -> Result<DatabaseSwap> {
        let ServiceArtifact::Postgres {
            name,
            url_env,
            database,
            ..
        } = artifact
        else {
            bail!("wrong adapter artifact");
        };
        if name != &self.config.name || url_env != &self.config.url_env {
            bail!("PostgreSQL artifact does not match configured adapter");
        }
        self.ensure_restore_allowed(artifact)?;
        let operation_id: String =
            blake3::hash(Ulid::new().to_string().as_bytes()).to_hex()[..10].into();
        let (temporary_database, old_database) =
            Self::swap_names(checkpoint_id, name, &operation_id);
        Ok(DatabaseSwap {
            name: name.clone(),
            url_env: url_env.clone(),
            database: database.clone(),
            temporary_database,
            old_database,
            system_identifier: match artifact {
                ServiceArtifact::Postgres {
                    target_identity: Some(identity),
                    ..
                } => Some(identity.system_identifier.clone()),
                _ => None,
            },
            operation_id: Some(operation_id),
            original_database_oid: Some(Self::identity_at(&self.url)?.database_oid),
            temporary_database_oid: None,
            temporary_owned: false,
            cutover_owned: false,
        })
    }

    pub(super) fn ensure_swap_cluster(&self, swap: &DatabaseSwap) -> Result<()> {
        let expected = swap.system_identifier.as_deref().context(
            "restore journal has no PostgreSQL cluster identity; destructive recovery is refused",
        )?;
        let live = self.target_identity()?;
        if live.system_identifier != expected {
            bail!("PostgreSQL cluster identity changed during restore");
        }
        self.ensure_maintenance_cluster(swap)
    }

    pub(super) fn ensure_maintenance_cluster(&self, swap: &DatabaseSwap) -> Result<()> {
        let expected = swap.system_identifier.as_deref().context(
            "restore journal has no PostgreSQL cluster identity; destructive recovery is refused",
        )?;
        let maintenance = self.url_for_database(&self.config.maintenance_db)?;
        let identity = self.target_identity_at(&maintenance)?;
        if identity.system_identifier != expected {
            bail!("PostgreSQL maintenance database cluster changed during restore");
        }
        if identity.database != self.config.maintenance_db {
            bail!("PostgreSQL maintenance connection changed database");
        }
        Ok(())
    }

    pub(super) fn database_exists(&self, maintenance: &str, database: &str) -> Result<bool> {
        Ok(self.database_oid(maintenance, database)?.is_some())
    }

    #[allow(clippy::unused_self)] // Queries must stay associated with the configured adapter.
    pub(super) fn database_oid(&self, maintenance: &str, database: &str) -> Result<Option<u32>> {
        let value = Self::query_at(
            maintenance,
            &format!(
                "SELECT oid::text FROM pg_database WHERE datname={}",
                Self::quote_literal(database)
            ),
        )?;
        if value.is_empty() {
            Ok(None)
        } else {
            Ok(Some(
                value.parse().context("invalid PostgreSQL database OID")?,
            ))
        }
    }

    pub(super) fn require_database_oid(
        &self,
        maintenance: &str,
        database: &str,
        expected_oid: u32,
    ) -> Result<()> {
        match self.database_oid(maintenance, database)? {
            Some(actual) if actual == expected_oid => Ok(()),
            Some(actual) => bail!(
                "PostgreSQL database {database} has OID {actual}, expected owned OID {expected_oid}"
            ),
            None => bail!("PostgreSQL database {database} is missing"),
        }
    }

    pub(super) fn drop_database_exact(
        &self,
        maintenance: &str,
        database: &str,
        expected_oid: u32,
    ) -> Result<()> {
        self.require_database_oid(maintenance, database, expected_oid)?;
        Self::query_at(
            maintenance,
            &format!("DROP DATABASE {} WITH (FORCE)", Self::quote_ident(database)),
        )?;
        Ok(())
    }

    pub fn preflight_staging(
        &self,
        artifact: &ServiceArtifact,
        store: &Store,
        swap: &DatabaseSwap,
    ) -> Result<()> {
        self.ensure_restore_allowed(artifact)?;
        self.ensure_swap_cluster(swap)?;
        self.verify(artifact, store)?;
        let ServiceArtifact::Postgres { database, .. } = artifact else {
            bail!("wrong adapter artifact");
        };
        if database != &swap.database {
            bail!("PostgreSQL restore journal does not match the artifact");
        }
        let identity = Self::identity_at(&self.url)?;
        if identity.database != *database {
            bail!(
                "configured PostgreSQL target changed: expected {database}, found {}",
                identity.database
            );
        }
        let original_oid = swap
            .original_database_oid
            .context("PostgreSQL journal has no original database OID")?;
        if identity.database_oid != original_oid {
            bail!("configured PostgreSQL target changed database identity during restore");
        }
        let maintenance = self.url_for_database(&self.config.maintenance_db)?;
        if self.database_exists(&maintenance, &swap.old_database)?
            || self.database_exists(&maintenance, &swap.temporary_database)?
        {
            bail!("refusing restore because its reserved PostgreSQL database names already exist");
        }
        Ok(())
    }

    pub fn create_staged_database(
        &self,
        artifact: &ServiceArtifact,
        store: &Store,
        swap: &DatabaseSwap,
    ) -> Result<u32> {
        if swap.temporary_owned || swap.temporary_database_oid.is_some() {
            bail!("PostgreSQL temporary database is already marked as owned");
        }
        self.ensure_restore_allowed(artifact)?;
        self.ensure_swap_cluster(swap)?;
        self.verify(artifact, store)?;
        let ServiceArtifact::Postgres {
            database,
            target_identity: Some(target_identity),
            ..
        } = artifact
        else {
            bail!("wrong adapter artifact");
        };
        let database_owner = target_identity
            .database_owner
            .as_deref()
            .context("PostgreSQL artifact has no database owner identity")?;
        if database != &swap.database {
            bail!("PostgreSQL restore journal does not match the artifact");
        }
        let identity = Self::identity_at(&self.url)?;
        if identity.database != *database {
            bail!(
                "configured PostgreSQL target changed: expected {database}, found {}",
                identity.database
            );
        }
        let original_oid = swap
            .original_database_oid
            .context("PostgreSQL journal has no original database OID")?;
        if identity.database_oid != original_oid {
            bail!("configured PostgreSQL target changed database identity during restore");
        }
        let maintenance = self.url_for_database(&self.config.maintenance_db)?;
        if self.database_exists(&maintenance, &swap.old_database)? {
            bail!(
                "refusing restore because recovery database {} already exists",
                swap.old_database
            );
        }
        if self.database_exists(&maintenance, &swap.temporary_database)? {
            bail!(
                "refusing to overwrite reserved temporary database {}",
                swap.temporary_database
            );
        }
        let create = format!(
            "CREATE DATABASE {} WITH TEMPLATE template0 OWNER {}",
            Self::quote_ident(&swap.temporary_database),
            Self::quote_ident(database_owner)
        );
        Self::query_at(&maintenance, &create)?;
        self.database_oid(&maintenance, &swap.temporary_database)?
            .context("created PostgreSQL temporary database has no OID")
    }

    pub fn populate_staged(
        &self,
        artifact: &ServiceArtifact,
        store: &Store,
        swap: &DatabaseSwap,
    ) -> Result<()> {
        if !swap.temporary_owned {
            bail!("PostgreSQL temporary database ownership is not journaled");
        }
        self.ensure_restore_allowed(artifact)?;
        self.ensure_maintenance_cluster(swap)?;
        self.verify(artifact, store)?;
        let ServiceArtifact::Postgres {
            object_hash,
            schema,
            ..
        } = artifact
        else {
            bail!("wrong adapter artifact");
        };
        let temporary_oid = swap
            .temporary_database_oid
            .context("PostgreSQL journal has no temporary database OID")?;
        let maintenance = self.url_for_database(&self.config.maintenance_db)?;
        self.require_database_oid(&maintenance, &swap.temporary_database, temporary_oid)?;
        let temp_url = self.url_for_database(&swap.temporary_database)?;
        let dump = object_to_temp(store, object_hash, &store.path().join("transactions"))?;
        let dump_path = dump
            .path()
            .to_str()
            .context("PostgreSQL dump path is not valid UTF-8")?;
        let (dbname, password) = Self::connection(&temp_url)?;
        let env = password.as_deref().map(|password| ("PGPASSWORD", password));
        Self::command(
            "pg_restore",
            &[
                "--exit-on-error",
                "--single-transaction",
                "--dbname",
                &dbname,
                dump_path,
            ],
            env.as_slice(),
        )?;
        let (actual_schema, _) = Self::inventory_at(&temp_url)?;
        if &actual_schema != schema {
            bail!("restored PostgreSQL schema did not match the checkpoint");
        }
        Ok(())
    }

    pub fn commit_staged(&self, swap: &DatabaseSwap) -> Result<()> {
        if !swap.temporary_owned || !swap.cutover_owned {
            bail!("PostgreSQL cutover ownership is not journaled");
        }
        self.ensure_swap_cluster(swap)?;
        let maintenance = self.url_for_database(&self.config.maintenance_db)?;
        if self.database_exists(&maintenance, &swap.old_database)? {
            bail!(
                "refusing PostgreSQL cutover because {} already exists",
                swap.old_database
            );
        }
        if !self.database_exists(&maintenance, &swap.temporary_database)? {
            bail!("prepared PostgreSQL database is missing");
        }
        let original_oid = swap
            .original_database_oid
            .context("PostgreSQL journal has no original database OID")?;
        let temporary_oid = swap
            .temporary_database_oid
            .context("PostgreSQL journal has no temporary database OID")?;
        self.require_database_oid(&maintenance, &swap.database, original_oid)?;
        self.require_database_oid(&maintenance, &swap.temporary_database, temporary_oid)?;
        Self::query_at(
            &maintenance,
            &format!(
                "SELECT pg_terminate_backend(pid) FROM pg_stat_activity WHERE datname={} AND pid<>pg_backend_pid()",
                Self::quote_literal(&swap.database)
            ),
        )?;
        Self::query_at(
            &maintenance,
            &format!(
                "ALTER DATABASE {} RENAME TO {}",
                Self::quote_ident(&swap.database),
                Self::quote_ident(&swap.old_database)
            ),
        )?;
        Self::query_at(
            &maintenance,
            &format!(
                "ALTER DATABASE {} RENAME TO {}",
                Self::quote_ident(&swap.temporary_database),
                Self::quote_ident(&swap.database)
            ),
        )?;
        Ok(())
    }

    pub fn rollback_staged(&self, swap: &DatabaseSwap) -> Result<()> {
        self.ensure_maintenance_cluster(swap)?;
        let maintenance = self.url_for_database(&self.config.maintenance_db)?;
        let original_oid = swap
            .original_database_oid
            .context("PostgreSQL journal has no original database OID")?;
        let temporary_oid = swap.temporary_database_oid;
        let old_oid = self.database_oid(&maintenance, &swap.old_database)?;
        if old_oid.is_some() && !swap.cutover_owned {
            bail!("PostgreSQL recovery database exists without journaled cutover ownership");
        }
        if let Some(actual) = old_oid {
            if actual != original_oid {
                bail!("PostgreSQL recovery database does not match the original database OID");
            }
            if let Some(target_oid) = self.database_oid(&maintenance, &swap.database)? {
                let expected_temporary = temporary_oid.context(
                    "PostgreSQL journal cannot prove ownership of the replacement database",
                )?;
                if target_oid != expected_temporary {
                    bail!("refusing to drop an unowned PostgreSQL target database");
                }
                self.drop_database_exact(&maintenance, &swap.database, expected_temporary)?;
            }
            Self::query_at(
                &maintenance,
                &format!(
                    "ALTER DATABASE {} RENAME TO {}",
                    Self::quote_ident(&swap.old_database),
                    Self::quote_ident(&swap.database)
                ),
            )?;
        } else {
            self.require_database_oid(&maintenance, &swap.database, original_oid)?;
        }
        if let Some(actual) = self.database_oid(&maintenance, &swap.temporary_database)? {
            if !swap.temporary_owned || temporary_oid != Some(actual) {
                bail!("refusing to drop an unowned PostgreSQL temporary database");
            }
            self.drop_database_exact(&maintenance, &swap.temporary_database, actual)?;
        }
        Ok(())
    }

    pub fn finalize_swap(&self, swap: &DatabaseSwap) -> Result<()> {
        self.ensure_maintenance_cluster(swap)?;
        let maintenance = self.url_for_database(&self.config.maintenance_db)?;
        if !swap.temporary_owned || !swap.cutover_owned {
            bail!("completed PostgreSQL restore has incomplete ownership records");
        }
        let original_oid = swap
            .original_database_oid
            .context("PostgreSQL journal has no original database OID")?;
        let temporary_oid = swap
            .temporary_database_oid
            .context("PostgreSQL journal has no temporary database OID")?;
        self.require_database_oid(&maintenance, &swap.database, temporary_oid)?;
        if self
            .database_oid(&maintenance, &swap.old_database)?
            .is_some()
        {
            self.drop_database_exact(&maintenance, &swap.old_database, original_oid)?;
        }
        if self
            .database_oid(&maintenance, &swap.temporary_database)?
            .is_some()
        {
            self.drop_database_exact(&maintenance, &swap.temporary_database, temporary_oid)?;
        }
        Ok(())
    }
}
