use super::*;

impl PostgresAdapter {
    pub fn ensure_restore_allowed(&self, artifact: &ServiceArtifact) -> Result<()> {
        if !self.config.allow_restore {
            bail!(
                "PostgreSQL restore is disabled for {}; set allow_restore = true explicitly",
                self.config.name
            );
        }
        let ServiceArtifact::Postgres {
            database,
            target_identity: Some(recorded),
            ..
        } = artifact
        else {
            bail!(
                "checkpoint has no PostgreSQL target identity; destructive legacy restore is refused"
            );
        };
        let recorded_owner = recorded
            .database_owner
            .as_deref()
            .filter(|owner| !owner.trim().is_empty())
            .context(
                "checkpoint has no PostgreSQL database owner identity; destructive legacy restore is refused",
            )?;
        self.restore_preflight()?;
        let live = self.target_identity()?;
        if live.system_identifier != recorded.system_identifier {
            bail!(
                "PostgreSQL cluster identity mismatch: checkpoint {}, current {}",
                recorded.system_identifier,
                live.system_identifier
            );
        }
        if live.database != *database || live.database != recorded.database {
            bail!(
                "PostgreSQL database target changed: expected {database}, found {}",
                live.database
            );
        }
        if live.server_major != recorded.server_major {
            bail!(
                "PostgreSQL server major changed: expected {}, found {}",
                recorded.server_major,
                live.server_major
            );
        }
        if live.database_owner.as_deref() != Some(recorded_owner) {
            bail!("PostgreSQL database owner changed since the checkpoint");
        }
        let maintenance = self.url_for_database(&self.config.maintenance_db)?;
        let maintenance_identity = self.target_identity_at(&maintenance)?;
        if maintenance_identity.system_identifier != recorded.system_identifier {
            bail!("PostgreSQL maintenance database belongs to a different cluster");
        }
        if maintenance_identity.database != self.config.maintenance_db {
            bail!("PostgreSQL maintenance connection resolved to an unexpected database");
        }
        if maintenance_identity.database == live.database {
            bail!("PostgreSQL maintenance database must differ from the restore target");
        }
        if !Self::is_local_host(&live.host) && !self.config.allow_remote_restore {
            bail!(
                "remote PostgreSQL restore to {}:{} is disabled; set allow_remote_restore = true explicitly",
                live.host,
                live.port
            );
        }
        Ok(())
    }

    pub fn restore_preflight(&self) -> Result<()> {
        let identity = self.client_preflight()?;
        let server_major = identity
            .version
            .parse::<u32>()
            .context("invalid PostgreSQL server version")?
            / 10_000;
        if server_major < 13 {
            bail!("PostgreSQL restore requires server major 13 or newer");
        }
        let privileges = Self::query_at(
            &self.url,
            "SELECT (r.rolsuper OR r.rolcreatedb)::int || '|' || (d.datdba=r.oid)::int FROM pg_roles r JOIN pg_database d ON d.datname=current_database() WHERE r.rolname=current_user",
        )?;
        if privileges != "1|1" {
            bail!("PostgreSQL restore requires CREATEDB (or superuser) and database ownership");
        }
        let maintenance = Self::identity_at(&self.url_for_database(&self.config.maintenance_db)?)?;
        if maintenance.database != self.config.maintenance_db {
            bail!("PostgreSQL maintenance connection resolved to an unexpected database");
        }
        let unmanageable_sessions = Self::query_at(
            &self.url,
            "SELECT count(*) FROM pg_stat_activity WHERE datname=current_database() AND pid<>pg_backend_pid() AND usename<>current_user AND NOT pg_has_role(current_user, usename, 'MEMBER')",
        )?
        .parse::<u64>()?;
        if unmanageable_sessions > 0 {
            bail!("PostgreSQL has active sessions that the configured role cannot terminate");
        }
        Ok(())
    }

    pub(super) fn client_preflight(&self) -> Result<Identity> {
        for executable_name in ["psql", "pg_dump", "pg_restore"] {
            if !executable(executable_name) {
                bail!("required executable {executable_name} is unavailable");
            }
        }
        let identity = Self::identity_at(&self.url)?;
        let unsupported_database_state = Self::query_at(
            &self.url,
            "SELECT (d.datacl IS NOT NULL)::int || '|' || (EXISTS (SELECT 1 FROM pg_db_role_setting WHERE setdatabase=d.oid))::int || '|' || (obj_description(d.oid, 'pg_database') IS NOT NULL)::int || '|' || ((d.encoding, d.datcollate, d.datctype, d.dattablespace) IS DISTINCT FROM (t.encoding, t.datcollate, t.datctype, t.dattablespace) OR (to_jsonb(d)->>'datlocprovider', to_jsonb(d)->>'daticulocale', to_jsonb(d)->>'datlocale', to_jsonb(d)->>'datcollversion') IS DISTINCT FROM (to_jsonb(t)->>'datlocprovider', to_jsonb(t)->>'daticulocale', to_jsonb(t)->>'datlocale', to_jsonb(t)->>'datcollversion'))::int || '|' || (d.datconnlimit <> -1 OR d.datistemplate)::int FROM pg_database d CROSS JOIN pg_database t WHERE d.datname=current_database() AND t.datname='template0'",
        )?;
        if unsupported_database_state != "0|0|0|0|0" {
            bail!(
                "PostgreSQL database-level ACLs, role settings, comments, locale/encoding/tablespace, connection limit, or template status are customized and cannot yet be restored losslessly"
            );
        }
        let server_major = identity
            .version
            .parse::<u32>()
            .context("invalid PostgreSQL server version")?
            / 10_000;
        let dump_major = Self::client_major("pg_dump")?;
        if dump_major < server_major {
            bail!(
                "pg_dump major {dump_major} is older than PostgreSQL server major {server_major}"
            );
        }
        let restore_major = Self::client_major("pg_restore")?;
        if restore_major < dump_major {
            bail!("pg_restore major {restore_major} is older than pg_dump major {dump_major}");
        }
        Ok(identity)
    }

    pub fn remote_confirmation_token(&self, artifact: &ServiceArtifact) -> Result<Option<String>> {
        let ServiceArtifact::Postgres { database, .. } = artifact else {
            return Ok(None);
        };
        let live = self.target_identity()?;
        Ok((!Self::is_local_host(&live.host)).then(|| format!("{database}@{}", live.host)))
    }

    pub fn verify_live_restore(&self, artifact: &ServiceArtifact) -> Result<()> {
        self.ensure_restore_allowed(artifact)?;
        let ServiceArtifact::Postgres { schema, .. } = artifact else {
            bail!("wrong adapter artifact");
        };
        let (actual_schema, _) = Self::inventory_at(&self.url)?;
        if &actual_schema != schema {
            bail!("live PostgreSQL schema does not match the checkpoint");
        }
        Ok(())
    }
}
