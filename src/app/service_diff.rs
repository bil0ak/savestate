use super::*;

pub(super) fn snapshot_content_equal(a: &SnapshotManifest, b: &SnapshotManifest) -> bool {
    a.roots == b.roots
        && filesystem::entries_equivalent(&a.files, &b.files)
        && a.services == b.services
        && a.capture_scope == b.capture_scope
}

pub(super) fn normalize_explicit_label(label: Option<String>) -> Result<Option<String>> {
    label
        .map(|label| normalize_label(&label, 200, false))
        .transpose()
}

pub(crate) fn normalize_label(value: &str, max_chars: usize, truncate: bool) -> Result<String> {
    let normalized = value.split_whitespace().collect::<Vec<_>>().join(" ");
    let normalized = normalized
        .trim_start_matches(['#', '>', '-', '*', '`', ' '])
        .trim();
    if normalized.is_empty() {
        bail!("checkpoint label cannot be empty");
    }
    if normalized.chars().any(char::is_control) {
        bail!("checkpoint label contains unsafe control characters");
    }
    if normalized.chars().count() <= max_chars {
        return Ok(normalized.to_owned());
    }
    if !truncate {
        bail!("checkpoint label cannot exceed {max_chars} characters");
    }
    let mut output = normalized
        .chars()
        .take(max_chars.saturating_sub(1))
        .collect::<String>();
    output.push('…');
    Ok(output)
}

#[derive(PartialEq, Eq, Serialize)]
pub(super) struct ServiceDiff {
    pub(super) name: String,
    pub(super) summary: String,
    pub(super) schema_added: Vec<String>,
    pub(super) schema_removed: Vec<String>,
    pub(super) estimated_row_changes: Vec<EstimatedRowChange>,
}

#[derive(PartialEq, Eq, Serialize)]
pub(super) struct EstimatedRowChange {
    pub(super) table: String,
    pub(super) before: Option<i64>,
    pub(super) after: Option<i64>,
}

pub(super) fn diff_services(a: &[ServiceArtifact], b: &[ServiceArtifact]) -> Vec<ServiceDiff> {
    let mut output = Vec::new();
    let before_names: BTreeSet<_> = a
        .iter()
        .map(|service| service_identity(service).0)
        .collect();
    for before in a {
        let (name, schema_a, rows_a) = service_identity(before);
        if let Some(after) = b.iter().find(|value| service_identity(value).0 == name) {
            let (_, schema_b, rows_b) = service_identity(after);
            let schema_a: BTreeSet<_> = schema_a.iter().collect();
            let schema_b: BTreeSet<_> = schema_b.iter().collect();
            let schema_added = schema_b
                .difference(&schema_a)
                .map(|value| (*value).clone())
                .collect::<Vec<_>>();
            let schema_removed = schema_a
                .difference(&schema_b)
                .map(|value| (*value).clone())
                .collect::<Vec<_>>();
            let tables: BTreeSet<_> = rows_a.keys().chain(rows_b.keys()).collect();
            let estimated_row_changes = tables
                .into_iter()
                .filter_map(|table| {
                    let before = rows_a.get(table).copied();
                    let after = rows_b.get(table).copied();
                    (before != after).then(|| EstimatedRowChange {
                        table: table.clone(),
                        before,
                        after,
                    })
                })
                .collect::<Vec<_>>();
            let content = sqlite_content_changed(before, after).map(|changed| {
                if changed {
                    "; content differs"
                } else {
                    "; content identical"
                }
            });
            output.push(ServiceDiff {
                name,
                summary: format!(
                    "schema +{}/-{}; {} tables have estimated row-count changes{}",
                    schema_added.len(),
                    schema_removed.len(),
                    estimated_row_changes.len(),
                    content.unwrap_or_default()
                ),
                schema_added,
                schema_removed,
                estimated_row_changes,
            });
        } else {
            output.push(ServiceDiff {
                name,
                summary: "removed".into(),
                schema_added: Vec::new(),
                schema_removed: schema_a.clone(),
                estimated_row_changes: Vec::new(),
            });
        }
    }
    for after in b {
        let name = service_identity(after).0;
        if !before_names.contains(&name) {
            output.push(ServiceDiff {
                name,
                summary: "added".into(),
                schema_added: service_identity(after).1.clone(),
                schema_removed: Vec::new(),
                estimated_row_changes: Vec::new(),
            });
        }
    }
    output
}

pub(super) fn sqlite_content_changed(
    before: &ServiceArtifact,
    after: &ServiceArtifact,
) -> Option<bool> {
    match (before, after) {
        (
            ServiceArtifact::Sqlite {
                object_hash: before,
                ..
            },
            ServiceArtifact::Sqlite {
                object_hash: after, ..
            },
        ) => Some(before != after),
        _ => None,
    }
}

pub(super) fn display_estimate(value: Option<i64>) -> String {
    value.map_or_else(|| "n/a".into(), |value| value.to_string())
}

pub(super) fn service_identity(
    service: &ServiceArtifact,
) -> (String, &Vec<String>, &BTreeMap<String, i64>) {
    match service {
        ServiceArtifact::Sqlite {
            root_id,
            path,
            schema,
            estimated_rows,
            ..
        } => (
            format!("sqlite:{root_id}:{}", path.display()),
            schema,
            estimated_rows,
        ),
        ServiceArtifact::Postgres {
            name,
            schema,
            estimated_rows,
            ..
        } => (format!("postgres:{name}"), schema, estimated_rows),
    }
}

pub(super) fn human_bytes(value: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut number = value as f64;
    let mut unit = 0usize;
    while number >= 1024.0 && unit < UNITS.len() - 1 {
        number /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{value} B")
    } else {
        format!("{number:.1} {}", UNITS[unit])
    }
}
