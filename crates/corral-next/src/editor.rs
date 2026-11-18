use std::{env, fs, io::Write, path::Path, process::Command};

use anyhow::{Context, Result, ensure};
use corral_engine::{Engine, Task, TaskEdit};

use crate::args::Edit;

pub(crate) fn prepare(engine: &Engine, flags: Edit) -> Result<(String, TaskEdit)> {
    let edit = TaskEdit {
        title: flags.title,
        body: flags.body_file.as_deref().map(read_text).transpose()?,
        project: flags.project.map(nonempty),
        priority: flags.priority,
        kind: flags.kind.map(nonempty),
        size: flags.size.map(nonempty),
        profile: flags.profile.map(nonempty),
        parent: flags.parent.map(nonempty),
        depends_on: flags.depends_on.as_deref().map(dependencies),
    };
    if edit == TaskEdit::default() {
        let task = engine.store.require_task(&flags.id)?;
        return Ok((flags.id, edit_interactively(&engine.state_dir, &task)?));
    }
    Ok((flags.id, edit))
}

fn nonempty(value: String) -> Option<String> {
    (!value.is_empty()).then_some(value)
}

pub(crate) fn dependencies(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(str::to_owned)
        .collect()
}

pub(crate) fn read_text(path: &Path) -> Result<String> {
    fs::read_to_string(path).with_context(|| format!("read {} as UTF-8", path.display()))
}

fn edit_interactively(state_dir: &Path, task: &Task) -> Result<TaskEdit> {
    let editor = env::var_os("EDITOR").unwrap_or_else(|| "vi".into());
    let editor = editor.to_str().context("EDITOR must be UTF-8")?;
    let args = shlex::split(editor).context("parse EDITOR arguments")?;
    let (program, args) = args.split_first().context("EDITOR is empty")?;
    let mut file = tempfile::Builder::new()
        .suffix(".md")
        .tempfile_in(state_dir)
        .context("create temporary task editor file")?;
    write!(file, "{}\n\n{}", task.title, task.body).context("write task editor file")?;
    let status = Command::new(program)
        .args(args)
        .arg(file.path())
        .status()
        .with_context(|| format!("start editor {program}"))?;
    ensure!(status.success(), "editor exited with {status}");
    let document = read_text(file.path())?;
    let (title, body) = title_and_body(&document);
    Ok(TaskEdit {
        title: Some(title.to_owned()),
        body: Some(body.to_owned()),
        ..Default::default()
    })
}

fn title_and_body(document: &str) -> (&str, &str) {
    for (end, _) in document.match_indices('\n') {
        let rest = &document[end + 1..];
        if let Some(body) = rest
            .strip_prefix('\n')
            .or_else(|| rest.strip_prefix("\r\n"))
        {
            return (document[..end].trim_end_matches('\r'), body);
        }
    }
    let (title, body) = document.split_once('\n').unwrap_or((document, ""));
    (title.trim_end_matches('\r'), body)
}

#[cfg(test)]
mod tests {
    use super::title_and_body;

    #[test]
    fn editor_document_preserves_body_whitespace() {
        assert_eq!(title_and_body("Title\n\n\nBody \n"), ("Title", "\nBody \n"));
        assert_eq!(
            title_and_body("Other title\r\n\r\nBody\n\nSecond paragraph"),
            ("Other title", "Body\n\nSecond paragraph")
        );
        assert_eq!(
            title_and_body("Title\nBody without a separator"),
            ("Title", "Body without a separator")
        );
    }
}
