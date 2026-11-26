use std::{
    fs::{self, OpenOptions},
    io::{Read, Write},
    path::Path,
};

use anyhow::{Context, Result, ensure};
use corral_protocol::diagram::{DiagramDiagnostic, MAX_RECIPE_BYTES, validate};
use serde::Serialize;

use crate::output::Output;

#[derive(Serialize)]
struct Validation<'a> {
    renderable: bool,
    diagnostics: &'a [DiagramDiagnostic],
}

pub(crate) fn run(path: &Path, out: &mut Output<'_>) -> Result<u8> {
    let diagnostic = validate(&read_recipe(path)?).err();
    let result = Validation {
        renderable: diagnostic.is_none(),
        diagnostics: diagnostic.as_slice(),
    };
    out.value(&result, |out, result| match result.diagnostics.first() {
        None => writeln!(out, "Diagram is structurally valid"),
        Some(diagnostic) => {
            let at = if diagnostic.at.is_empty() {
                "(root)"
            } else {
                &diagnostic.at
            };
            writeln!(
                out,
                "error at {}: {}",
                at.escape_debug(),
                diagnostic.message.escape_debug()
            )
        }
    })?;
    Ok(if result.renderable { 0 } else { 2 })
}

fn read_recipe(path: &Path) -> Result<Vec<u8>> {
    ensure!(
        fs::metadata(path)
            .with_context(|| format!("inspect diagram {path:?}"))?
            .is_file(),
        "diagram is not a regular file: {path:?}"
    );
    let mut options = OpenOptions::new();
    options.read(true);
    // A FIFO swapped in after the path check must not block Unix open.
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NONBLOCK);
    }
    let file = options
        .open(path)
        .with_context(|| format!("open diagram {path:?}"))?;
    ensure!(
        file.metadata()
            .with_context(|| format!("inspect opened diagram {path:?}"))?
            .is_file(),
        "diagram is not a regular file: {path:?}"
    );
    // Read one extra byte so the protocol can report oversize, including for growing files.
    let mut bytes = Vec::new();
    file.take((MAX_RECIPE_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .with_context(|| format!("read diagram {path:?}"))?;
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_stops_after_one_byte_past_the_limit() {
        let file = tempfile::NamedTempFile::new().unwrap();
        file.as_file()
            .set_len((MAX_RECIPE_BYTES + 2) as u64)
            .unwrap();
        assert_eq!(
            read_recipe(file.path()).unwrap().len(),
            MAX_RECIPE_BYTES + 1
        );
    }
}
