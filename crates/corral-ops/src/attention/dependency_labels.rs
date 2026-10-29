use super::Neighborhood;

pub(super) fn label(n: &Neighborhood<'_>, id: &str) -> String {
    format!(
        "{id} ({})",
        n.task(id).map_or("missing", |task| task.status.as_str())
    )
}
