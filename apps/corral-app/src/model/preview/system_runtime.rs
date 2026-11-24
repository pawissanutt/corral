//! The sample's `system_runtime`: what each sample project runs, so the Runtime view shows
//! without a daemon. `corral` has binaries a CI job runs and a package starts, clap commands
//! and an operation enum; `site` has a compose file whose services build from its packages,
//! depend on each other and run its Python binaries, an image, npm scripts (folded until
//! unfolded) and packages no entry point detector applies to.

use std::collections::BTreeSet;

use corral_types::{
    EntryDetector, EntryKind, EntryPoint, RuntimeBasis, RuntimeEdge, RuntimeEdgeKind, RuntimeSite,
    RuntimeSource, RuntimeUnit, RuntimeUnitId, RuntimeUnitKind, SystemRuntime,
    SystemRuntimeRow as Row, SystemStatus,
};

use RuntimeUnitKind::{Binary, Image, Job, Script, Service};

const HEAD: &str = "85b8f96031c4e2a7d9b0";

fn id(kind: RuntimeUnitKind, name: &str, path: &str) -> RuntimeUnitId {
    RuntimeUnitId {
        kind,
        name: name.into(),
        path: path.into(),
    }
}

fn unit(kind: RuntimeUnitKind, name: &str, path: &str, line: u64, container: &str) -> RuntimeUnit {
    RuntimeUnit {
        id: id(kind, name, path),
        line,
        container: container.into(),
        command: None,
        entry_point: None,
        image: None,
        build_context: None,
        dockerfile: None,
        ports: Vec::new(),
        depends_on: Vec::new(),
    }
}

fn edge(
    kind: RuntimeEdgeKind,
    from: RuntimeSource,
    to: RuntimeUnitId,
    basis: RuntimeBasis,
    path: &str,
    line: u64,
) -> Row {
    Row::Edge {
        edge: RuntimeEdge {
            kind,
            from,
            to,
            basis,
            site: RuntimeSite {
                path: path.into(),
                line,
            },
        },
    }
}

fn container(path: &str) -> RuntimeSource {
    RuntimeSource::Container { path: path.into() }
}

fn entry(kind: EntryKind, name: &str, path: &str, line: u64, owner: RuntimeSource) -> Row {
    Row::Entry {
        entry: EntryPoint {
            kind,
            name: name.into(),
            path: path.into(),
            line,
            owner,
        },
    }
}

fn coverage(path: &str, detectors: &[EntryDetector]) -> Row {
    Row::EntryCoverage {
        container: path.into(),
        detectors: detectors.iter().copied().collect::<BTreeSet<_>>(),
    }
}

fn corral() -> Vec<Row> {
    let cli = id(Binary, "corral", "crates/corral/src/main.rs");
    let app = id(Binary, "nsh-app-corral", "apps/corral-app/src/main.rs");
    let gate = id(Job, "gate", ".github/workflows/ci.yml");
    let web = id(Job, "app-web", ".github/workflows/ci.yml");
    let mut gate_unit = unit(Job, "gate", ".github/workflows/ci.yml", 12, "");
    gate_unit.command = Some("corral gate --tier full".into());
    let owner = RuntimeSource::Unit { id: cli.clone() };
    let mut rows = vec![
        Row::Unit {
            unit: unit(
                Binary,
                "corral",
                "crates/corral/src/main.rs",
                1,
                "crates/corral",
            ),
        },
        Row::Unit {
            unit: unit(
                Binary,
                "nsh-app-corral",
                "apps/corral-app/src/main.rs",
                1,
                "apps/corral-app",
            ),
        },
        Row::Unit { unit: gate_unit },
        Row::Unit {
            unit: unit(Job, "app-web", ".github/workflows/ci.yml", 41, ""),
        },
        edge(
            RuntimeEdgeKind::Runs,
            RuntimeSource::Unit { id: gate },
            cli.clone(),
            RuntimeBasis::Declared,
            ".github/workflows/ci.yml",
            20,
        ),
        edge(
            RuntimeEdgeKind::Starts,
            container("crates/corral-ops"),
            cli.clone(),
            RuntimeBasis::Syntax,
            "crates/corral-ops/src/dispatch.rs",
            88,
        ),
        edge(
            RuntimeEdgeKind::Starts,
            container("crates/corral-daemon"),
            cli,
            RuntimeBasis::Syntax,
            "crates/corral-daemon/src/system/worker.rs",
            140,
        ),
        edge(
            RuntimeEdgeKind::Runs,
            RuntimeSource::Unit { id: web },
            app,
            RuntimeBasis::Declared,
            ".github/workflows/ci.yml",
            52,
        ),
    ];
    for (name, line) in [("dispatch", 40), ("gate", 52), ("done", 61), ("daemon", 75)] {
        rows.push(entry(
            EntryKind::Command,
            name,
            "crates/corral/src/cli.rs",
            line,
            owner.clone(),
        ));
    }
    for (name, line) in [("Dispatch", 30), ("Approve", 44), ("Drop", 58)] {
        rows.push(entry(
            EntryKind::Operation,
            name,
            "crates/corral-types/src/op.rs",
            line,
            container("crates/corral-types"),
        ));
    }
    rows.extend([
        coverage("crates/corral", &[EntryDetector::Clap]),
        coverage("crates/corral-types", &[EntryDetector::OperationEnum]),
        coverage("crates/corral-core", &[]),
        coverage("apps/corral-app", &[]),
    ]);
    rows
}

fn site() -> Vec<Row> {
    let compose = "compose.yaml";
    let server = id(Binary, "api-server", "services/api/pyproject.toml");
    let worker_bin = id(Binary, "site-worker", "services/worker/pyproject.toml");
    let web = id(Service, "web", compose);
    let api = id(Service, "api", compose);
    let worker = id(Service, "worker", compose);
    let db = id(Service, "db", compose);
    let image = id(Image, "Dockerfile", "services/api/Dockerfile");
    let mut web_unit = unit(Service, "web", compose, 3, "");
    web_unit.build_context = Some("apps/web".into());
    web_unit.ports = vec!["3000:3000".into()];
    web_unit.depends_on = vec!["api".into()];
    let mut api_unit = unit(Service, "api", compose, 12, "");
    api_unit.build_context = Some("services/api".into());
    api_unit.ports = vec!["8080:8080".into()];
    api_unit.command = Some("api-server --port 8080".into());
    api_unit.depends_on = vec!["db".into()];
    let mut worker_unit = unit(Service, "worker", compose, 22, "");
    worker_unit.command = Some("site-worker".into());
    worker_unit.depends_on = vec!["db".into()];
    let mut db_unit = unit(Service, "db", compose, 30, "");
    db_unit.image = Some("postgres:16".into());
    db_unit.ports = vec!["5432:5432".into()];
    let mut image_unit = unit(
        Image,
        "Dockerfile",
        "services/api/Dockerfile",
        1,
        "services/api",
    );
    image_unit.entry_point = Some("api-server".into());
    let mut rows: Vec<Row> = [web_unit, api_unit, worker_unit, db_unit, image_unit]
        .into_iter()
        .map(|unit| Row::Unit { unit })
        .collect();
    rows.extend([
        Row::Unit {
            unit: unit(
                Binary,
                "api-server",
                "services/api/pyproject.toml",
                14,
                "services/api",
            ),
        },
        Row::Unit {
            unit: unit(
                Binary,
                "site-worker",
                "services/worker/pyproject.toml",
                12,
                "services/worker",
            ),
        },
        Row::Unit {
            unit: unit(Job, "deploy", ".github/workflows/deploy.yml", 8, ""),
        },
    ]);
    for (n, name) in ["dev", "build", "test", "lint", "preview", "e2e"]
        .into_iter()
        .enumerate()
    {
        rows.push(Row::Unit {
            unit: unit(
                Script,
                name,
                "apps/web/package.json",
                5 + n as u64,
                "apps/web",
            ),
        });
    }
    let declared = RuntimeBasis::Declared;
    let from = |id: &RuntimeUnitId| RuntimeSource::Unit { id: id.clone() };
    rows.extend([
        edge(
            RuntimeEdgeKind::DependsOn,
            from(&web),
            api.clone(),
            declared,
            compose,
            9,
        ),
        edge(
            RuntimeEdgeKind::DependsOn,
            from(&api),
            db.clone(),
            declared,
            compose,
            19,
        ),
        edge(
            RuntimeEdgeKind::DependsOn,
            from(&worker),
            db,
            declared,
            compose,
            27,
        ),
        edge(
            RuntimeEdgeKind::Runs,
            from(&api),
            server.clone(),
            declared,
            compose,
            16,
        ),
        edge(
            RuntimeEdgeKind::Runs,
            from(&worker),
            worker_bin,
            declared,
            compose,
            24,
        ),
        edge(
            RuntimeEdgeKind::Runs,
            from(&image),
            server,
            declared,
            "services/api/Dockerfile",
            9,
        ),
        edge(
            RuntimeEdgeKind::Builds,
            container("apps/web"),
            web,
            declared,
            compose,
            4,
        ),
        edge(
            RuntimeEdgeKind::Builds,
            container("services/api"),
            api,
            declared,
            compose,
            13,
        ),
        edge(
            RuntimeEdgeKind::Builds,
            container("services/api"),
            image,
            declared,
            "services/api/Dockerfile",
            1,
        ),
    ]);
    for (name, line) in [("GET /", 12), ("GET /tasks", 18), ("POST /tasks", 27)] {
        rows.push(entry(
            EntryKind::Route,
            name,
            "apps/web/src/routes/index.ts",
            line,
            container("apps/web"),
        ));
    }
    rows.extend([
        coverage("apps/web", &[EntryDetector::Express, EntryDetector::NpmBin]),
        coverage("packages/ui", &[EntryDetector::NpmBin]),
        coverage("services/api", &[]),
        coverage("services/worker", &[]),
        coverage("infra", &[]),
    ]);
    rows
}

pub(super) fn detail(project: &str) -> Result<SystemRuntime, String> {
    let rows = match project {
        "corral" => corral(),
        "site" => site(),
        _ => return Err(format!("not_found: no system facts for project {project}")),
    };
    Ok(SystemRuntime {
        project: project.into(),
        head: Some(HEAD.into()),
        status: SystemStatus::Ready,
        error: None,
        omitted_rows: 0,
        rows,
    })
}
