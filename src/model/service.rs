use std::{collections::BTreeMap, path::PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ServiceArtifact {
    Sqlite {
        root_id: String,
        path: PathBuf,
        object_hash: String,
        schema: Vec<String>,
        estimated_rows: BTreeMap<String, i64>,
    },
    Postgres {
        name: String,
        url_env: String,
        database: String,
        server_version: String,
        object_hash: String,
        schema: Vec<String>,
        estimated_rows: BTreeMap<String, i64>,
        #[serde(default)]
        target_identity: Option<PostgresTargetIdentity>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PostgresTargetIdentity {
    pub system_identifier: String,
    pub database: String,
    pub host: String,
    pub port: u16,
    pub user: String,
    #[serde(default)]
    pub database_owner: Option<String>,
    pub server_major: u32,
}

impl ServiceArtifact {
    pub fn object_hash(&self) -> &str {
        match self {
            Self::Sqlite { object_hash, .. } | Self::Postgres { object_hash, .. } => object_hash,
        }
    }
}
