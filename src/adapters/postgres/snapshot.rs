use super::*;

impl Adapter for PostgresAdapter {
    fn preflight(&self) -> Result<()> {
        self.client_preflight()?;
        Ok(())
    }
    fn snapshot(&self, store: &Store) -> Result<ServiceArtifact> {
        self.preflight()?;
        let before_identity = Self::identity_at(&self.url)?;
        let (before_schema, _) = Self::inventory_at(&self.url)?;
        let dump = tempfile::NamedTempFile::new_in(store.path().join("transactions"))?;
        let dump_path = dump
            .path()
            .to_str()
            .context("PostgreSQL dump path is not valid UTF-8")?;
        let (dbname, password) = Self::connection(&self.url)?;
        let env = password.as_deref().map(|password| ("PGPASSWORD", password));
        Self::command(
            "pg_dump",
            &["--format=custom", "--dbname", &dbname, "--file", dump_path],
            env.as_slice(),
        )?;
        Self::command("pg_restore", &["--list", dump_path], &[])?;
        let (schema, estimated_rows) = Self::inventory_at(&self.url)?;
        let after_identity = Self::identity_at(&self.url)?;
        if before_identity != after_identity {
            bail!("PostgreSQL target identity changed during checkpoint capture");
        }
        if before_schema != schema {
            bail!("PostgreSQL schema changed during checkpoint capture");
        }
        let (object_hash, _) = store.put_object(dump.path(), None)?;
        Ok(ServiceArtifact::Postgres {
            name: self.config.name.clone(),
            url_env: self.config.url_env.clone(),
            database: before_identity.database.clone(),
            server_version: before_identity.version.clone(),
            object_hash,
            schema,
            estimated_rows,
            target_identity: Some(Self::target_identity_from(&before_identity)?),
        })
    }
    fn verify(&self, artifact: &ServiceArtifact, store: &Store) -> Result<()> {
        require_object(artifact, store)?;
        let ServiceArtifact::Postgres { object_hash, .. } = artifact else {
            bail!("wrong adapter artifact");
        };
        let dump = object_to_temp(store, object_hash, &store.path().join("transactions"))?;
        let path = dump
            .path()
            .to_str()
            .context("PostgreSQL dump path is not valid UTF-8")?;
        Self::command("pg_restore", &["--list", path], &[])?;
        Ok(())
    }
    fn restore(&self, _artifact: &ServiceArtifact, _store: &Store) -> Result<()> {
        bail!("PostgreSQL restore requires Savestate's journaled restore coordinator")
    }
}
