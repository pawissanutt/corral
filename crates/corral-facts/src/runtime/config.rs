//! Read literal runtime declarations from committed configuration text only.

use std::borrow::Cow;

use crate::model::*;

use super::yaml::{self, Node};
use super::{Declarations, basename, unit};

enum Config {
    Compose,
    Image,
    Service,
    Workflow,
}

pub(super) fn recognizes(path: &str) -> bool {
    config(path).is_some()
}

pub(super) fn read(path: &str, text: &str, container: &str, output: &mut Declarations) -> bool {
    match config(path) {
        Some(Config::Compose) => compose(path, text, container, output),
        Some(Config::Image) => dockerfile(path, text, container, output),
        Some(Config::Service) => systemd(path, text, container, output),
        Some(Config::Workflow) => workflow(path, text, container, output),
        None => false,
    }
}

fn variant(name: &str, base: &str) -> bool {
    name == base
        || name
            .strip_prefix(base)
            .is_some_and(|suffix| suffix.len() > 1 && suffix.starts_with(['.', '-', '_']))
}

fn config(path: &str) -> Option<Config> {
    let name = basename(path);
    if let Some((stem, "yaml" | "yml")) = name.rsplit_once('.') {
        if path.strip_prefix(".github/workflows/") == Some(name) && !stem.is_empty() {
            return Some(Config::Workflow);
        }
        if variant(stem, "compose") || variant(stem, "docker-compose") {
            return Some(Config::Compose);
        }
    }
    if ["Dockerfile", "Containerfile"].into_iter().any(|base| {
        variant(name, base)
            || name
                .rsplit_once('.')
                .is_some_and(|(prefix, suffix)| !prefix.is_empty() && suffix == base)
    }) {
        return Some(Config::Image);
    }
    (name.len() > ".service".len() && name.ends_with(".service")).then_some(Config::Service)
}

fn site(path: &str, line: u64) -> RuntimeSite {
    RuntimeSite {
        path: path.to_owned(),
        line,
    }
}

fn literal_token(word: &str) -> Option<&str> {
    // Leave paths intact: resolution compares the literal word with binary names.
    (!word.is_empty()
        && word
            .chars()
            .all(|c| c.is_alphanumeric() || "._-/+@".contains(c)))
    .then_some(word)
}

fn first_word(command: &str, shell: bool) -> Option<&str> {
    let command = command.trim_start();
    let separator = |c: char| c.is_whitespace() || (shell && ";|&<>()".contains(c));
    // Recognize a simple quoted first token, but don't interpret escapes, shell
    // expressions, assignments, or adjacent quoted and unquoted word fragments.
    let (word, rest) = if let Some(quote @ ('\'' | '"')) = command.chars().next() {
        let (word, rest) = command[quote.len_utf8()..].split_once(quote)?;
        if rest.chars().next().is_some_and(|ch| !separator(ch)) {
            return None;
        }
        (word, rest)
    } else {
        command.split_at(command.find(separator).unwrap_or(command.len()))
    };
    if shell && rest.trim_start().starts_with('(') {
        // A shell function definition is not an invocation of that word.
        return None;
    }
    literal_token(word)
}

struct Command {
    text: String,
    program: Option<String>,
}

impl Command {
    fn scalar(text: &str, shell: bool) -> Self {
        Self {
            text: text.to_owned(),
            program: first_word(text, shell).map(str::to_owned),
        }
    }

    fn argv<T: AsRef<str> + serde::Serialize>(words: &[T]) -> Option<Self> {
        Some(Self {
            text: serde_json::to_string(words).ok()?,
            program: words
                .first()
                .and_then(|word| literal_token(word.as_ref()))
                .map(str::to_owned),
        })
    }
}

fn runs(output: &mut Declarations, from: &RuntimeUnitId, line: u64, program: Option<String>) {
    if let Some(program) = program {
        output
            .runs
            .push((from.clone(), site(&from.path, line), program));
    }
}

fn yaml_command(node: &Node<'_>) -> Option<Command> {
    if let Some(text) = node.scalar() {
        return Some(Command::scalar(text, false));
    }
    let words = node
        .sequence()?
        .iter()
        .map(Node::scalar)
        .collect::<Option<Vec<_>>>()?;
    Command::argv(&words)
}

fn optional_scalar<'a>(node: &'a Node<'_>, key: &str) -> Option<Option<&'a str>> {
    match node.field(key) {
        Some((_, value)) => Some(Some(value.scalar()?)),
        None => Some(None),
    }
}

fn port(node: &Node<'_>) -> Option<String> {
    if let Some(port) = node.scalar() {
        return Some(port.to_owned());
    }
    // Render long-syntax bindings like the short syntax, without assuming defaults.
    if node.has_merge() {
        return None;
    }
    let (_, target) = node.field("target")?;
    let mut binding = String::new();
    let host = optional_scalar(node, "host_ip")?;
    if let Some(host) = host {
        if host.contains(':') && !host.starts_with('[') {
            binding.push('[');
            binding.push_str(host);
            binding.push(']');
        } else {
            binding.push_str(host);
        }
        binding.push(':');
    }
    if let Some(published) = optional_scalar(node, "published")? {
        binding.push_str(published);
        binding.push(':');
    } else if host.is_some() {
        binding.push(':');
    }
    binding.push_str(target.scalar()?);
    if let Some(protocol) = optional_scalar(node, "protocol")? {
        binding.push('/');
        binding.push_str(protocol);
    }
    Some(binding)
}

fn build<'a>(node: &'a Node<'_>) -> Option<(&'a str, Option<&'a str>)> {
    if let Some(context) = node.scalar() {
        return Some((context, None));
    }
    if !node.is_mapping() {
        return None;
    }
    let context = match node.field("context") {
        Some((_, context)) => context.scalar()?,
        None if node.has_merge() => return None,
        None => ".",
    };
    let dockerfile = node.field("dockerfile").and_then(|(_, file)| file.scalar());
    Some((context, dockerfile))
}

fn compose(path: &str, text: &str, container: &str, output: &mut Declarations) -> bool {
    let Some(root) = yaml::parse(text) else {
        return false;
    };
    let Some((_, services)) = root.field("services").filter(|(_, node)| node.is_mapping()) else {
        return false;
    };
    for (name, line, service) in services.fields() {
        if !service.is_mapping() {
            continue;
        }
        let mut unit = unit(RuntimeUnitKind::Service, name, path, line, container);
        if let Some((_, image)) = service.field("image") {
            unit.image = image.scalar().map(str::to_owned);
        }
        if let Some((line, build_node)) = service.field("build")
            && let Some((context, dockerfile)) = build(build_node)
        {
            let inline = build_node.field("dockerfile_inline").is_some();
            unit.build_context = Some(context.to_owned());
            unit.dockerfile = dockerfile.filter(|_| !inline).map(str::to_owned);
            // An explicit empty path tells the resolver not to look for a
            // default Dockerfile, while still resolving the declared context.
            let dockerfile = if inline
                || (unit.dockerfile.is_none()
                    && (build_node.field("dockerfile").is_some() || build_node.has_merge()))
            {
                Some(String::new())
            } else {
                unit.dockerfile.clone()
            };
            output.builds.push((
                unit.id.clone(),
                site(path, line),
                context.to_owned(),
                dockerfile,
            ));
        }
        if let Some(ports) = service.field("ports").and_then(|(_, node)| node.sequence()) {
            unit.ports.extend(ports.iter().filter_map(port));
        }
        if let Some((_, dependencies)) = service.field("depends_on") {
            let mut record = |line, name: &str| {
                if !name.is_empty() {
                    unit.depends_on.push(name.to_owned());
                    output
                        .depends_on
                        .push((unit.id.clone(), site(path, line), name.to_owned()));
                }
            };
            if let Some(names) = dependencies.sequence() {
                for name in names {
                    if let Some(text) = name.scalar() {
                        record(name.line, text);
                    }
                }
            } else {
                for (name, line, _) in dependencies.fields() {
                    record(line, name);
                }
            }
            unit.depends_on.sort_unstable();
            unit.depends_on.dedup();
        }
        if let Some((line, command)) = service.field("command")
            && let Some(command) = yaml_command(command)
        {
            runs(output, &unit.id, line, command.program);
            unit.command = Some(command.text);
        }
        output.units.push(unit);
    }
    true
}

fn workflow(path: &str, text: &str, container: &str, output: &mut Declarations) -> bool {
    let Some(root) = yaml::parse(text) else {
        return false;
    };
    let Some((_, jobs)) = root.field("jobs").filter(|(_, node)| node.is_mapping()) else {
        return false;
    };
    for (name, line, job) in jobs.fields() {
        if !job.is_mapping() {
            continue;
        }
        let unit = unit(RuntimeUnitKind::Job, name, path, line, container);
        if let Some(steps) = job.field("steps").and_then(|(_, node)| node.sequence()) {
            for step in steps {
                if let Some((line, run)) = step.field("run")
                    && let Some(script) = run.scalar()
                {
                    // Ignore initial blank/comment lines, not setup commands.
                    let program = script
                        .lines()
                        .map(str::trim_start)
                        .find(|line| !line.is_empty() && !line.starts_with('#'))
                        .and_then(|line| first_word(line, true));
                    runs(output, &unit.id, line, program.map(str::to_owned));
                }
            }
        }
        output.units.push(unit);
    }
    true
}

/// Join physical continuation lines and retain the first line as the anchor.
/// `between` is empty for Dockerfiles and a space for systemd unit files.
fn logical_line<'a>(
    lines: &mut impl Iterator<Item = (usize, &'a str)>,
    comments: &[char],
    escape: char,
    between: &str,
) -> Option<(u64, Cow<'a, str>)> {
    let mut data = lines.filter_map(|(index, line)| {
        let line = line.trim_end();
        (!line.is_empty() && !line.trim_start().starts_with(comments)).then_some((index, line))
    });
    let (index, part) = data.next()?;
    let mut part = part.trim_start();
    let mut text = Cow::Borrowed(part);
    while let Some(prefix) = part
        .strip_suffix(escape)
        .filter(|prefix| !prefix.ends_with(escape))
    {
        let (_, next) = data.next()?;
        let next = if between.is_empty() {
            next
        } else {
            next.trim_start()
        };
        let joined = text.to_mut();
        joined.truncate(joined.len() - part.len() + prefix.len());
        joined.push_str(between);
        joined.push_str(next);
        part = next;
    }
    Some((index as u64 + 1, text))
}

fn docker_escape(text: &str) -> char {
    // Parser directives are recognized only in the opening directive block.
    for line in text.lines() {
        let Some((key, value)) = line
            .trim_start()
            .strip_prefix('#')
            .and_then(|line| line.split_once('='))
        else {
            break;
        };
        let key = key.trim();
        if key.eq_ignore_ascii_case("escape") {
            return if value.trim() == "`" { '`' } else { '\\' };
        }
        if !key.eq_ignore_ascii_case("syntax") && !key.eq_ignore_ascii_case("check") {
            break;
        }
    }
    '\\'
}

fn docker_command(args: &str) -> Option<Command> {
    if args.starts_with('[') {
        let words = serde_json::from_str::<Vec<String>>(args).ok()?;
        Command::argv(&words)
    } else {
        (!args.is_empty()).then(|| Command::scalar(args, true))
    }
}

fn dockerfile(path: &str, text: &str, container: &str, output: &mut Declarations) -> bool {
    let mut lines = text.lines().enumerate();
    let escape = docker_escape(text);
    let mut stage = None;
    let mut entry_point = None;
    let mut command = None;
    while let Some((line, instruction)) = logical_line(&mut lines, &['#'], escape, "") {
        let Some((opcode, args)) = instruction.split_once(char::is_whitespace) else {
            continue;
        };
        let args = args.trim_start();
        if opcode.eq_ignore_ascii_case("FROM") {
            // Only the final stage describes the resulting image. Do not infer
            // instructions inherited from another stage or an external image.
            stage = Some(line);
            entry_point = None;
            command = None;
        } else if opcode.eq_ignore_ascii_case("ENTRYPOINT") {
            entry_point = docker_command(args).map(|command| (line, command));
        } else if opcode.eq_ignore_ascii_case("CMD") {
            command = docker_command(args);
        } else if ["RUN", "COPY", "ADD"]
            .into_iter()
            .any(|name| opcode.eq_ignore_ascii_case(name))
        {
            skip_heredocs(args, &mut lines);
        }
    }
    let Some(line) = stage else { return false };
    let mut unit = unit(
        RuntimeUnitKind::Image,
        basename(path),
        path,
        line,
        container,
    );
    if let Some((line, entry_point)) = entry_point {
        runs(output, &unit.id, line, entry_point.program);
        unit.entry_point = Some(entry_point.text);
    }
    unit.command = command.map(|command| command.text);
    output.units.push(unit);
    true
}

fn docker_words(mut input: &str) -> impl Iterator<Item = &str> {
    // Only separate words for heredoc detection. Keep quotes and escapes intact.
    std::iter::from_fn(move || {
        input = input.trim_start();
        if input.is_empty() {
            return None;
        }
        let mut quote = None;
        let mut escaped = false;
        let end = input
            .char_indices()
            .find_map(|(index, ch)| {
                if escaped {
                    escaped = false;
                } else if ch == '\\' && quote != Some('\'') {
                    escaped = true;
                } else if let Some(current) = quote {
                    if ch == current {
                        quote = None;
                    }
                } else if matches!(ch, '\'' | '"') {
                    quote = Some(ch);
                } else if ch.is_ascii_whitespace() {
                    return Some(index);
                }
                None
            })
            .unwrap_or(input.len());
        let (word, rest) = input.split_at(end);
        input = rest;
        Some(word)
    })
}

fn skip_heredocs<'a>(args: &str, lines: &mut impl Iterator<Item = (usize, &'a str)>) {
    // Handle conventional <<EOF / <<-"EOF" Dockerfile tokens so body lines
    // that look like Docker instructions aren't scanned as instructions.
    for word in docker_words(args) {
        let operator = word.trim_start_matches(|ch: char| ch.is_ascii_digit());
        let Some(heredoc) = operator.strip_prefix("<<") else {
            continue;
        };
        let (delimiter, strip_tabs) = heredoc
            .strip_prefix('-')
            .map_or((heredoc, false), |end| (end, true));
        let delimiter = delimiter
            .strip_prefix('"')
            .and_then(|word| word.strip_suffix('"'))
            .or_else(|| {
                delimiter
                    .strip_prefix('\'')
                    .and_then(|word| word.strip_suffix('\''))
            })
            .unwrap_or(delimiter);
        if literal_token(delimiter).is_none() {
            continue;
        }
        for (_, line) in &mut *lines {
            let line = if strip_tabs {
                line.trim_start_matches('\t')
            } else {
                line
            };
            if line == delimiter {
                break;
            }
        }
    }
}

fn systemd(path: &str, text: &str, container: &str, output: &mut Declarations) -> bool {
    let mut unit_line = None;
    let mut in_service = false;
    let mut exec = Vec::new();
    let mut lines = text.lines().enumerate();
    while let Some((line, declaration)) = logical_line(&mut lines, &['#', ';'], '\\', " ") {
        if declaration.starts_with('[') && declaration.ends_with(']') {
            in_service = declaration == "[Service]";
            if in_service {
                unit_line.get_or_insert(line);
            }
        } else if in_service
            && let Some(("ExecStart", args)) = declaration
                .split_once('=')
                .map(|(key, value)| (key.trim(), value.trim()))
        {
            if args.is_empty() {
                exec.clear();
            } else {
                exec.push((line, args.to_owned()));
            }
        }
    }
    let Some(line) = unit_line else { return false };
    let mut unit = unit(RuntimeUnitKind::Unit, basename(path), path, line, container);
    let mut commands = Vec::new();
    for (line, args) in exec {
        // systemd flags change invocation semantics, not the executable name.
        // Don't follow the `|` shell prefix or expand specifiers/variables.
        let executable = args.trim_start_matches(['-', '@', ':', '+', '!']);
        runs(
            output,
            &unit.id,
            line,
            first_word(executable, false).map(str::to_owned),
        );
        commands.push(args);
    }
    if !commands.is_empty() {
        unit.command = Some(commands.join("\n"));
    }
    output.units.push(unit);
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workflow_anchors_real_steps_not_run_like_text() {
        let path = ".github/workflows/checks.yaml";
        let mut output = Declarations::default();
        read(
            path,
            "# jobs:\njobs:\n  'test':\n    steps:\n      - name: 'run: fake'\n        run: |\n          # setup\n          worker --check\n      - run: ${{ inputs.tool }}\n",
            "ci",
            &mut output,
        );
        assert_eq!(output.units.len(), 1);
        assert_eq!(output.units[0].id.name, "test");
        assert_eq!(output.units[0].line, 3);
        assert_eq!(
            output.runs,
            [(output.units[0].id.clone(), site(path, 6), "worker".into())]
        );
    }

    #[test]
    fn dockerfile_does_not_treat_heredoc_body_as_an_image_stage() {
        let path = "deploy/Containerfile.local";
        let mut output = Declarations::default();
        read(
            path,
            "FROM scratch AS builder\nENTRYPOINT [\"builder\"]\nFROM scratch\nRUN echo \"not a <<TAG\"\nRUN <<'EOF'\nFROM bogus\nENTRYPOINT [\"not-run\"]\nEOF\nentrypoint worker \\\n  --watch\nCMD [\"args\"]\n",
            "deploy",
            &mut output,
        );
        assert_eq!(output.units.len(), 1);
        assert_eq!(output.units[0].line, 3);
        assert_eq!(
            output.units[0].entry_point.as_deref(),
            Some("worker   --watch")
        );
        assert_eq!(output.units[0].command.as_deref(), Some("[\"args\"]"));
        assert_eq!(
            output.runs,
            [(output.units[0].id.clone(), site(path, 9), "worker".into())]
        );
    }

    #[test]
    fn systemd_exec_start_resets_within_service_section() {
        let path = "deploy/worker.service";
        let mut output = Declarations::default();
        read(
            path,
            "[Unit]\nExecStart=outside\n[Service]\nExecStart=reset\nExecStart=\nExecStart=-@worker --watch\n[Install]\nExecStart=outside\n",
            "deploy",
            &mut output,
        );
        assert_eq!(output.units.len(), 1);
        assert_eq!(output.units[0].line, 3);
        assert_eq!(output.units[0].command.as_deref(), Some("-@worker --watch"));
        assert_eq!(
            output.runs,
            [(output.units[0].id.clone(), site(path, 6), "worker".into())]
        );
    }
}
