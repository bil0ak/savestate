use std::{
    collections::BTreeMap,
    process::{Command, Output},
};

use anyhow::{Context, Result, bail};
use percent_encoding::percent_decode_str;
use serde::Deserialize;
use ulid::Ulid;
use url::Url;

use crate::{
    adapters::{Adapter, executable, object_to_temp, require_object},
    config::PostgresConfig,
    model::{DatabaseSwap, PostgresTargetIdentity, ServiceArtifact},
    store::Store,
};

mod client;
mod restore;
mod safety;
mod snapshot;

#[cfg(test)]
mod tests;

pub struct PostgresAdapter {
    pub config: PostgresConfig,
    pub url: String,
}

#[derive(Debug, PartialEq, Eq, Deserialize)]
struct Identity {
    pub(super) database: String,
    pub(super) database_oid: u32,
    pub(super) user: String,
    pub(super) version: String,
    pub(super) system_identifier: String,
    pub(super) database_owner: String,
    pub(super) server_address: Option<String>,
    pub(super) server_port: Option<u16>,
}
