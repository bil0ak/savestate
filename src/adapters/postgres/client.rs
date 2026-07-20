use super::*;

impl PostgresAdapter {
    pub(crate) fn swap_names(
        checkpoint_id: &str,
        adapter_name: &str,
        operation_id: &str,
    ) -> (String, String) {
        let suffix = checkpoint_id
            .chars()
            .take(10)
            .collect::<String>()
            .to_ascii_lowercase();
        let adapter_hash = &blake3::hash(adapter_name.as_bytes()).to_hex()[..8];
        (
            format!("savestate_new_{suffix}_{adapter_hash}_{operation_id}"),
            format!("savestate_old_{suffix}_{adapter_hash}_{operation_id}"),
        )
    }

    pub(crate) fn valid_operation_id(value: &str) -> bool {
        crate::model::OperationId::new(value).is_ok()
    }

    pub fn new(config: PostgresConfig) -> Result<Self> {
        let url = std::env::var(&config.url_env)
            .with_context(|| format!("environment variable {} is not set", config.url_env))?;
        Ok(Self { config, url })
    }

    pub(super) fn command(program: &str, args: &[&str], envs: &[(&str, &str)]) -> Result<Output> {
        let output = Command::new(program)
            .args(args)
            .envs(envs.iter().copied())
            .output()
            .with_context(|| format!("run {program}"))?;
        if !output.status.success() {
            bail!(
                "{program} failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
        Ok(output)
    }

    /// Splits a connection string into an argv-safe form and the password to
    /// deliver via PGPASSWORD, so secrets never appear in process listings.
    /// Password-less URLs and non-URL (keyword/value) connection strings pass
    /// through byte-for-byte with no password.
    pub(super) fn connection(url: &str) -> Result<(String, Option<String>)> {
        let Ok(mut parsed) = Url::parse(url) else {
            return Ok((url.to_owned(), None));
        };
        let Some(encoded) = parsed.password().filter(|password| !password.is_empty()) else {
            return Ok((url.to_owned(), None));
        };
        let password = percent_decode_str(encoded)
            .decode_utf8()
            .context("percent-decoded PostgreSQL URL password is not valid UTF-8")?
            .into_owned();
        if parsed.set_password(None).is_err() {
            bail!("cannot strip the password from the PostgreSQL URL");
        }
        Ok((parsed.into(), Some(password)))
    }

    pub(super) fn query_at(url: &str, sql: &str) -> Result<String> {
        let (dbname, password) = Self::connection(url)?;
        let env = password.as_deref().map(|password| ("PGPASSWORD", password));
        let output = Self::command(
            "psql",
            &[
                "--no-psqlrc",
                "--tuples-only",
                "--no-align",
                "--dbname",
                &dbname,
                "--command",
                sql,
            ],
            env.as_slice(),
        )?;
        Ok(String::from_utf8(output.stdout)?.trim().to_owned())
    }

    pub(super) fn identity_at(url: &str) -> Result<Identity> {
        let value = Self::query_at(
            url,
            r"SELECT json_build_object(
                'database', current_database(),
                'database_oid', (SELECT oid FROM pg_database WHERE datname = current_database()),
                'user', current_user,
                'version', current_setting('server_version_num'),
                'system_identifier', system_identifier::text,
                'database_owner', pg_get_userbyid((SELECT datdba FROM pg_database WHERE datname = current_database())),
                'server_address', inet_server_addr()::text,
                'server_port', inet_server_port()
            )::text FROM pg_control_system()",
        )?;
        Self::parse_identity(&value)
    }

    pub(super) fn parse_identity(value: &str) -> Result<Identity> {
        serde_json::from_str(value).context("parse PostgreSQL server identity")
    }

    #[allow(clippy::unused_self)] // The adapter instance is the credential/command context.
    pub(super) fn target_identity_at(&self, url: &str) -> Result<PostgresTargetIdentity> {
        let identity = Self::identity_at(url)?;
        Self::target_identity_from(&identity)
    }

    pub(super) fn target_identity_from(identity: &Identity) -> Result<PostgresTargetIdentity> {
        Ok(PostgresTargetIdentity {
            system_identifier: identity.system_identifier.clone(),
            database: identity.database.clone(),
            host: identity
                .server_address
                .clone()
                .unwrap_or_else(|| "local-socket".into()),
            port: identity.server_port.unwrap_or(5432),
            user: identity.user.clone(),
            database_owner: Some(identity.database_owner.clone()),
            server_major: identity.version.parse::<u32>()? / 10_000,
        })
    }

    pub fn target_identity(&self) -> Result<PostgresTargetIdentity> {
        self.target_identity_at(&self.url)
    }

    pub(super) fn is_local_host(host: &str) -> bool {
        matches!(
            host.trim().to_ascii_lowercase().as_str(),
            "" | "local-socket" | "localhost" | "127.0.0.1" | "::1" | "[::1]"
        ) || host.starts_with('/')
    }

    pub(super) fn client_major(program: &str) -> Result<u32> {
        let output = Self::command(program, &["--version"], &[])?;
        String::from_utf8(output.stdout)?
            .split_whitespace()
            .find_map(|part| {
                part.trim_start_matches(|character: char| !character.is_ascii_digit())
                    .split('.')
                    .next()
                    .and_then(|major| major.parse().ok())
            })
            .with_context(|| format!("could not parse {program} version"))
    }

    pub(super) fn inventory_at(url: &str) -> Result<(Vec<String>, BTreeMap<String, i64>)> {
        let sql = r"
SELECT kind || '|' || identity FROM (
 SELECT 'extension' kind, extname || '|' || extversion identity FROM pg_extension
 UNION ALL SELECT 'table', n.nspname || '.' || c.relname FROM pg_class c JOIN pg_namespace n ON n.oid=c.relnamespace WHERE c.relkind IN ('r','p','v','m','S') AND n.nspname NOT IN ('pg_catalog','information_schema') AND n.nspname !~ '^pg_toast'
 UNION ALL SELECT 'column', table_schema || '.' || table_name || '.' || column_name || '|' || data_type || '|' || is_nullable || '|' || coalesce(column_default,'') FROM information_schema.columns WHERE table_schema NOT IN ('pg_catalog','information_schema')
 UNION ALL SELECT 'index', schemaname || '.' || indexname || '|' || indexdef FROM pg_indexes WHERE schemaname NOT IN ('pg_catalog','information_schema')
 UNION ALL SELECT 'constraint', n.nspname || '.' || c.relname || '.' || con.conname || '|' || pg_get_constraintdef(con.oid) FROM pg_constraint con JOIN pg_class c ON c.oid=con.conrelid JOIN pg_namespace n ON n.oid=c.relnamespace WHERE n.nspname NOT IN ('pg_catalog','information_schema')
) inventory ORDER BY kind, identity";
        let schema = Self::query_at(url, sql)?
            .lines()
            .map(str::to_owned)
            .collect();
        let estimates = Self::query_at(
            url,
            "SELECT n.nspname || '.' || c.relname || '|' || greatest(c.reltuples::bigint,0) FROM pg_class c JOIN pg_namespace n ON n.oid=c.relnamespace WHERE c.relkind IN ('r','p') AND n.nspname NOT IN ('pg_catalog','information_schema') AND n.nspname !~ '^pg_toast' ORDER BY 1",
        )?;
        let rows = estimates
            .lines()
            .filter_map(|line| {
                let (name, count) = line.rsplit_once('|')?;
                Some((name.to_owned(), count.parse().unwrap_or(0)))
            })
            .collect();
        Ok((schema, rows))
    }

    pub(super) fn url_for_database(&self, database: &str) -> Result<String> {
        let mut url = Url::parse(&self.url).context("DATABASE_URL must be a postgres URL")?;
        let query = url
            .query_pairs()
            .map(|(key, value)| (key.into_owned(), value.into_owned()))
            .collect::<Vec<_>>();
        if query.iter().any(|(key, _)| key == "service") {
            bail!("PostgreSQL service indirection is unsupported for safe restore");
        }
        url.set_path(&format!("/{database}"));
        url.set_query(None);
        let query = query
            .iter()
            .filter(|(key, _)| key != "dbname")
            .collect::<Vec<_>>();
        if !query.is_empty() {
            url.query_pairs_mut().extend_pairs(query);
        }
        Ok(url.into())
    }

    pub(super) fn quote_ident(value: &str) -> String {
        format!("\"{}\"", value.replace('"', "\"\""))
    }
    pub(super) fn quote_literal(value: &str) -> String {
        format!("'{}'", value.replace('\'', "''"))
    }
}
