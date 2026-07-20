use super::*;

#[test]
fn legacy_artifact_is_refused_before_any_database_command() {
    let adapter = PostgresAdapter {
        config: PostgresConfig {
            name: "development".into(),
            url_env: "DATABASE_URL".into(),
            maintenance_db: "postgres".into(),
            allow_restore: true,
            allow_remote_restore: false,
        },
        url: "postgresql://unreachable.invalid/development".into(),
    };
    let artifact = ServiceArtifact::Postgres {
        name: "development".into(),
        url_env: "DATABASE_URL".into(),
        database: "development".into(),
        server_version: "160000".into(),
        object_hash: "0".repeat(64),
        schema: Vec::new(),
        estimated_rows: BTreeMap::new(),
        target_identity: None,
    };
    let error = adapter
        .ensure_restore_allowed(&artifact)
        .expect_err("legacy PostgreSQL artifact must fail closed");
    assert!(error.to_string().contains("no PostgreSQL target identity"));
}

#[test]
fn server_identity_is_structured_and_handles_identifier_delimiters() -> Result<()> {
    let identity = PostgresAdapter::parse_identity(
        r#"{"database":"dev|copy","database_oid":16384,"user":"owner|admin","version":"160004","system_identifier":"742019337","database_owner":"database-owner","server_address":"203.0.113.7","server_port":5433}"#,
    )?;
    assert_eq!(identity.database, "dev|copy");
    assert_eq!(identity.database_oid, 16384);
    assert_eq!(identity.user, "owner|admin");
    assert_eq!(identity.database_owner, "database-owner");
    assert_eq!(identity.server_address.as_deref(), Some("203.0.113.7"));
    assert_eq!(identity.server_port, Some(5433));
    assert!(!PostgresAdapter::is_local_host("203.0.113.7"));
    assert!(PostgresAdapter::is_local_host("local-socket"));
    Ok(())
}

#[test]
fn reserved_database_names_are_operation_scoped_and_bounded() {
    let first =
        PostgresAdapter::swap_names("01KXP3EXAMPLECHECKPOINTID", "development", "0123456789");
    let second =
        PostgresAdapter::swap_names("01KXP3EXAMPLECHECKPOINTID", "development", "abcdef0123");
    assert_ne!(first, second);
    assert!(first.0.len() <= 63 && first.1.len() <= 63);
    assert!(PostgresAdapter::valid_operation_id("abcdef0123"));
    assert!(!PostgresAdapter::valid_operation_id("ABCDEF0123"));
    assert!(!PostgresAdapter::valid_operation_id("../unsafe"));
}

#[test]
fn connection_moves_url_passwords_out_of_argv() -> Result<()> {
    let (dbname, password) =
        PostgresAdapter::connection("postgresql://user:p%40ss@localhost/app?sslmode=require")?;
    assert_eq!(dbname, "postgresql://user@localhost/app?sslmode=require");
    assert_eq!(password.as_deref(), Some("p@ss"));

    let plain = "postgresql://user@localhost/app";
    assert_eq!(
        PostgresAdapter::connection(plain)?,
        (plain.to_owned(), None)
    );

    let keyword = "host=localhost dbname=app password=secret";
    assert_eq!(
        PostgresAdapter::connection(keyword)?,
        (keyword.to_owned(), None)
    );

    assert!(PostgresAdapter::connection("postgresql://user:%FF@localhost/app").is_err());
    Ok(())
}

#[test]
fn maintenance_url_cannot_be_overridden_by_database_query_parameters() -> Result<()> {
    let adapter = PostgresAdapter {
        config: PostgresConfig {
            name: "development".into(),
            url_env: "DATABASE_URL".into(),
            maintenance_db: "postgres".into(),
            allow_restore: true,
            allow_remote_restore: false,
        },
        url: "postgresql://localhost/app?dbname=wrong&sslmode=require".into(),
    };
    let maintenance = Url::parse(&adapter.url_for_database("postgres")?)?;
    assert_eq!(maintenance.path(), "/postgres");
    assert!(maintenance.query_pairs().all(|(key, _)| key != "dbname"));
    assert!(
        maintenance
            .query_pairs()
            .any(|(key, value)| key == "sslmode" && value == "require")
    );

    let mut opaque = adapter;
    opaque.url = "postgresql://localhost/app?service=production".into();
    assert!(opaque.url_for_database("postgres").is_err());
    Ok(())
}
