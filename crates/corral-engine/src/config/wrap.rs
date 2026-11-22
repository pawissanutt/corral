use std::{borrow::Cow, path::Path};

use anyhow::{Result, bail, ensure};
use corral_types::ConfigProblem;

use super::{Quote, absolute_text, append_quoted, append_shell_char, problem};

pub(super) fn check(
    template: Option<&str>,
    key: impl FnOnce() -> String,
    problems: &mut Vec<ConfigProblem>,
) {
    if let Some(template) = template
        && let Err(error) = render(template, |_| Ok(Cow::Borrowed("")))
    {
        problems.push(problem(key(), error.to_string()));
    }
}

pub(crate) fn render_wrap(
    template: &str,
    command: &str,
    root: &Path,
    worktree: &Path,
    dir: &Path,
) -> Result<String> {
    render(template, |token| {
        let path = match token {
            "{command}" => return Ok(Cow::Borrowed(command)),
            "{root}" => root,
            "{worktree}" => worktree,
            "{dir}" => dir,
            _ => bail!("unsupported wrapper placeholder {token}"),
        };
        absolute_text(path).map(Cow::Owned)
    })
}

fn render<'a>(template: &str, value: impl Fn(&str) -> Result<Cow<'a, str>>) -> Result<String> {
    ensure!(!template.contains('\0'), "wrap must not contain NUL");
    let mut saw_command = false;
    let mut output = String::with_capacity(template.len());
    let mut remaining = template;
    let mut quote = Quote::None;
    while !remaining.is_empty() {
        if let Some(token) = ["{command}", "{root}", "{worktree}", "{dir}"]
            .into_iter()
            .find(|token| remaining.starts_with(token))
        {
            saw_command |= token == "{command}";
            append_quoted(&mut output, &value(token)?, quote)?;
            remaining = &remaining[token.len()..];
        } else {
            append_shell_char(&mut output, &mut remaining, &mut quote);
        }
    }
    ensure!(saw_command, "wrap must contain {{command}}");
    Ok(output)
}
