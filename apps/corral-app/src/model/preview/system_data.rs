//! The sample's `system_data`: a few stored and wire types per sample project with their
//! fields, the `has` edges between them, the containers of the sample map with their languages,
//! and per-language coverage, so the Data view shows without a daemon. `corral` has serialized
//! Rust types and its JSON Schema; `site` has SQL tables and one schema message, and nothing
//! reads its TypeScript or Python, so those containers say "not provided". `data_changes` is
//! what a sample task changed in them, for Impact's Data model category.

use std::collections::{BTreeMap, BTreeSet};

use corral_types::{
    DataChange, DataChangeKind, DataEdge, DataEdgeKind, DataField, DataKind, DataSource, DataType,
    SourceExcerpt, SystemData, SystemDataRow, SystemMapRow, SystemStatus,
};

use super::system::detail as map;

const HEAD: &str = "85b8f96031c4e2a7d9b0";

type Field = (&'static str, &'static str, bool);
/// From, field, to: a field naming another type of the project.
type Has = (&'static str, &'static str, &'static str);

struct Sample {
    path: &'static str,
    name: &'static str,
    line: u64,
    kind: DataKind,
    source: DataSource,
    fields: &'static [Field],
}

const fn t(
    path: &'static str,
    name: &'static str,
    line: u64,
    kind: DataKind,
    source: DataSource,
    fields: &'static [Field],
) -> Sample {
    Sample {
        path,
        name,
        line,
        kind,
        source,
        fields,
    }
}

const TYPES: &str = "crates/corral-types/src/lib.rs";
const SCHEMA: &str = "crates/corral-protocol/schema/corral.v1.json";
const CONFIG: &str = "crates/corral-engine/src/config.rs";

const CORRAL: &[Sample] = &[
    t(
        TYPES,
        "Task",
        40,
        DataKind::Record,
        DataSource::RustSerde,
        &[
            ("id", "String", false),
            ("title", "String", false),
            ("status", "Status", false),
            ("project", "Option<String>", true),
            ("runs", "Vec<Run>", false),
            ("labels", "Vec<String>", false),
        ],
    ),
    t(
        TYPES,
        "Status",
        12,
        DataKind::Enum,
        DataSource::RustSerde,
        &[
            ("Backlog", "", false),
            ("Ready", "", false),
            ("Running", "", false),
            ("Blocked", "", false),
            ("Review", "", false),
            ("Done", "", false),
        ],
    ),
    t(
        TYPES,
        "Run",
        70,
        DataKind::Record,
        DataSource::RustSerde,
        &[
            ("id", "String", false),
            ("profile", "String", false),
            ("check", "Option<Check>", true),
        ],
    ),
    t(
        TYPES,
        "Check",
        90,
        DataKind::Record,
        DataSource::RustSerde,
        &[("round", "u64", false), ("verdict", "String", false)],
    ),
    t(
        CONFIG,
        "Config",
        20,
        DataKind::Record,
        DataSource::RustSerde,
        &[
            ("projects", "BTreeMap<String, ProjectConfig>", false),
            ("routing", "Vec<Route>", false),
        ],
    ),
    t(
        CONFIG,
        "ProjectConfig",
        48,
        DataKind::Record,
        DataSource::RustSerde,
        &[("path", "PathBuf", false), ("gate", "Option<String>", true)],
    ),
    t(
        SCHEMA,
        "Refusal",
        210,
        DataKind::Message,
        DataSource::JsonSchema,
        &[("reason", "string", false), ("next", "array<Step>", false)],
    ),
    t(
        SCHEMA,
        "Step",
        240,
        DataKind::Message,
        DataSource::JsonSchema,
        &[("op", "string", false), ("force", "boolean", true)],
    ),
];

const SITE: &[Sample] = &[
    t(
        "services/api/db/schema.sql",
        "users",
        1,
        DataKind::Table,
        DataSource::Sql,
        &[
            ("id", "uuid", false),
            ("email", "text", false),
            ("created_at", "timestamptz", false),
        ],
    ),
    t(
        "services/api/db/schema.sql",
        "orders",
        8,
        DataKind::Table,
        DataSource::Sql,
        &[
            ("id", "uuid", false),
            ("user_id", "uuid references users", false),
            ("total_cents", "bigint", false),
            ("note", "text", true),
        ],
    ),
    t(
        "packages/api-client/schema/order.json",
        "Order",
        3,
        DataKind::Message,
        DataSource::JsonSchema,
        &[
            ("id", "string", false),
            ("user", "User", false),
            ("total", "integer", false),
        ],
    ),
    t(
        "packages/api-client/schema/order.json",
        "User",
        30,
        DataKind::Message,
        DataSource::JsonSchema,
        &[("id", "string", false), ("email", "string", false)],
    ),
];

const CORRAL_HAS: &[Has] = &[
    ("Task", "status", "Status"),
    ("Task", "runs", "Run"),
    ("Run", "check", "Check"),
    ("Config", "projects", "ProjectConfig"),
    ("Refusal", "next", "Step"),
];
const SITE_HAS: &[Has] = &[("orders", "user_id", "users"), ("Order", "user", "User")];

fn samples(project: &str) -> Result<(&'static [Sample], &'static [Has]), String> {
    match project {
        "corral" => Ok((CORRAL, CORRAL_HAS)),
        "site" => Ok((SITE, SITE_HAS)),
        _ => Err(format!("not_found: no system facts for project {project}")),
    }
}

fn data_type(s: &Sample) -> DataType {
    DataType {
        path: s.path.into(),
        name: s.name.into(),
        line: s.line,
        kind: s.kind,
        source: s.source,
        fields: s
            .fields
            .iter()
            .enumerate()
            .map(|(i, (name, ty, optional))| DataField {
                name: (*name).into(),
                ty: (*ty).into(),
                line: s.line + 1 + i as u64,
                optional: *optional,
            })
            .collect(),
    }
}

pub(super) fn detail(project: &str) -> Result<SystemData, String> {
    let (types, has) = samples(project)?;
    let served = map(project, None)?;
    let mut rows = Vec::new();
    let read: BTreeMap<&str, DataSource> = match project {
        "corral" => [
            ("Rust", DataSource::RustSerde),
            ("JSON", DataSource::JsonSchema),
        ]
        .into(),
        _ => [("SQL", DataSource::Sql), ("JSON", DataSource::JsonSchema)].into(),
    };
    let mut languages: BTreeSet<String> = BTreeSet::new();
    for row in &served.rows {
        if let SystemMapRow::Container { container, stats } = row {
            let langs: BTreeSet<String> = stats.languages.keys().cloned().collect();
            languages.extend(langs.iter().cloned());
            rows.push(SystemDataRow::Container {
                path: container.path.clone(),
                name: container.name.clone(),
                sources: langs
                    .iter()
                    .filter_map(|l| read.get(l.as_str()).copied())
                    .collect(),
                languages: langs,
            });
        }
    }
    languages.extend(read.keys().map(|l| (*l).to_owned()));
    rows.extend(
        languages
            .into_iter()
            .map(|language| SystemDataRow::Coverage {
                sources: read.get(language.as_str()).into_iter().copied().collect(),
                language,
            }),
    );
    let by_name: BTreeMap<&str, DataType> = types.iter().map(|s| (s.name, data_type(s))).collect();
    rows.extend(by_name.values().map(|ty| SystemDataRow::Type {
        data_type: ty.clone(),
    }));
    for (from, field, to) in has {
        let (Some(a), Some(b)) = (by_name.get(from), by_name.get(to)) else {
            continue;
        };
        let line = a
            .fields
            .iter()
            .find(|f| f.name == *field)
            .map_or(a.line, |f| f.line);
        rows.push(SystemDataRow::Has {
            edge: DataEdge {
                kind: DataEdgeKind::Has,
                from: a.id(),
                to: b.id(),
                field: (*field).into(),
                line,
            },
        });
    }
    Ok(SystemData {
        project: project.into(),
        commit: Some(HEAD.into()),
        status: SystemStatus::Ready,
        error: None,
        provider: served.provider,
        omitted_rows: 0,
        rows,
    })
}

/// The committed lines a change's anchor would show, made up from the sample declaration.
fn excerpt(path: &str, commit: &str, line: u64, lines: &[&str]) -> SourceExcerpt {
    SourceExcerpt {
        path: path.into(),
        commit: commit.into(),
        line,
        first: line.saturating_sub(2).max(1),
        lines: lines.iter().map(|l| (*l).to_owned()).collect(),
    }
}

/// What a sample task changed in its project's data model, each with the excerpt corrald would
/// serve; one change is left without, as when its line could not be read within bounds.
pub(super) fn data_changes(
    project: &str,
    base: &str,
    head: &str,
) -> Vec<(DataChange, Option<SourceExcerpt>)> {
    let field = |name: &str, ty: &str, line: u64, optional: bool| DataField {
        name: name.into(),
        ty: ty.into(),
        line,
        optional,
    };
    let change = |path: &str, name: &str, line: u64, kind, source, change| DataChange {
        path: path.into(),
        name: name.into(),
        line,
        kind,
        source,
        contract: true,
        change,
    };
    match project {
        "corral" => vec![
            (
                change(
                    TYPES,
                    "Task",
                    40,
                    DataKind::Record,
                    DataSource::RustSerde,
                    DataChangeKind::FieldAdded {
                        field: field("labels", "Vec<String>", 46, false),
                    },
                ),
                Some(excerpt(
                    TYPES,
                    head,
                    46,
                    &[
                        "    pub project: Option<String>,",
                        "    pub runs: Vec<Run>,",
                        "    pub labels: Vec<String>,",
                        "}",
                        "",
                    ],
                )),
            ),
            (
                change(
                    TYPES,
                    "Check",
                    90,
                    DataKind::Record,
                    DataSource::RustSerde,
                    DataChangeKind::FieldRetyped {
                        before: field("round", "u32", 91, false),
                        after: field("round", "u64", 91, false),
                    },
                ),
                Some(excerpt(
                    TYPES,
                    head,
                    91,
                    &[
                        "#[derive(Debug, Clone, Serialize, Deserialize)]",
                        "pub struct Check {",
                        "    pub round: u64,",
                        "    pub verdict: String,",
                        "}",
                    ],
                )),
            ),
            (
                change(
                    TYPES,
                    "LegacyRun",
                    120,
                    DataKind::Record,
                    DataSource::RustSerde,
                    DataChangeKind::TypeRemoved,
                ),
                Some(excerpt(
                    TYPES,
                    base,
                    120,
                    &[
                        "",
                        "#[derive(Serialize, Deserialize)]",
                        "pub struct LegacyRun {",
                        "    pub pane: String,",
                        "}",
                    ],
                )),
            ),
            (
                change(
                    SCHEMA,
                    "Step",
                    240,
                    DataKind::Message,
                    DataSource::JsonSchema,
                    DataChangeKind::FieldOptionalityChanged {
                        before: field("force", "boolean", 242, false),
                        after: field("force", "boolean", 242, true),
                    },
                ),
                None,
            ),
        ],
        "site" => vec![(
            change(
                "services/api/db/schema.sql",
                "orders",
                8,
                DataKind::Table,
                DataSource::Sql,
                DataChangeKind::FieldAdded {
                    field: field("note", "text", 12, true),
                },
            ),
            Some(excerpt(
                "services/api/db/schema.sql",
                head,
                12,
                &[
                    "  user_id uuid NOT NULL REFERENCES users(id),",
                    "  total_cents bigint NOT NULL,",
                    "  note text",
                    ");",
                ],
            )),
        )],
        _ => Vec::new(),
    }
}
