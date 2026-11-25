//! Saving an artifact's original bytes: the file name the save offers, where a save stands and
//! the words for it. The bytes are read again from corrald page by page, apart from the
//! preview's cache, and only a complete read that matches the catalog's size and SHA-256 saves.

use corral_types::Artifact;

pub const DOWNLOAD: &str = "Download";
pub const CANCEL_DOWNLOAD: &str = "Cancel download";

/// Where one artifact's save stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Saving {
    Idle,
    /// The platform's save dialog is open.
    Choosing,
    /// Reading pages from corrald under request `id`.
    Reading {
        id: String,
        percent: u8,
    },
    /// All bytes read and verified; the file is being put in place.
    Finishing,
    /// Saved; where, as the platform names it.
    Saved(String),
    Failed(String),
}

impl Saving {
    /// The line under the actions; none while nothing is under way.
    pub fn note(&self) -> Option<String> {
        match self {
            Saving::Idle => None,
            Saving::Choosing => Some("Choosing where to save…".into()),
            Saving::Reading { percent, .. } => Some(format!("Downloading… {percent}%")),
            Saving::Finishing => Some("Saving…".into()),
            Saving::Saved(at) => Some(format!("Saved {at}")),
            Saving::Failed(why) => Some(format!("Download failed: {why}")),
        }
    }
}

/// Names Windows keeps for devices, whatever the extension.
const RESERVED: [&str; 22] = [
    "con", "prn", "aux", "nul", "com1", "com2", "com3", "com4", "com5", "com6", "com7", "com8",
    "com9", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9",
];
/// Bytes of a suggested name at most, under every platform's file name limit (255 bytes).
const NAME_BYTES: usize = 180;

/// A file name safe on every platform the app saves on, from the artifact's title: its last path
/// segment, with separators, control and reserved characters replaced, no leading dot, at most
/// [`NAME_BYTES`] bytes keeping the extension; else one from its reference.
pub fn file_name(artifact: &Artifact) -> String {
    let title = artifact
        .title
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or_default();
    let cleaned: String = title
        .chars()
        .map(|c| match c {
            c if c.is_control() => '_',
            '<' | '>' | ':' | '"' | '|' | '?' | '*' => '_',
            c => c,
        })
        .collect();
    let trimmed = cleaned
        .trim_start_matches(['.', ' '])
        .trim_end_matches(['.', ' ']);
    let name = shorten(trimmed);
    let stem = name
        .split('.')
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    if name.is_empty() || RESERVED.contains(&stem.as_str()) {
        let fallback = format!("{}-{}", artifact.task, artifact.id).replace(':', "-");
        return if name.is_empty() {
            fallback
        } else {
            format!("{fallback}-{name}")
        };
    }
    name
}

fn shorten(name: &str) -> String {
    if name.len() <= NAME_BYTES {
        return name.to_owned();
    }
    let extension = name
        .rsplit_once('.')
        .map(|(_, extension)| extension)
        .filter(|extension| (1..=16).contains(&extension.len()));
    let room = NAME_BYTES - extension.map_or(0, |extension| extension.len() + 1);
    let stem = name[..name.floor_char_boundary(room)].trim_end_matches(['.', ' ']);
    match extension {
        Some(extension) => format!("{stem}.{extension}"),
        None => stem.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn named(title: &str) -> String {
        file_name(&Artifact {
            task: "T-12".into(),
            id: 3,
            sha256: String::new(),
            bytes: 0,
            kind: corral_types::ArtifactKind::Report,
            title: title.into(),
            at: jiff::Timestamp::UNIX_EPOCH,
            by: corral_types::Actor::Agent,
            project: None,
            run: None,
            replaces: None,
            replaced_by: None,
        })
    }

    #[test]
    fn suggested_names_cannot_leave_the_chosen_folder_or_name_a_device() {
        assert_eq!(named("../../etc/passwd"), "passwd");
        assert_eq!(named("C:\\Windows\\notes.md"), "notes.md");
        assert_eq!(named(".hidden"), "hidden");
        assert_eq!(named("a:b?c*.log"), "a_b_c_.log");
        assert_eq!(named(".."), "T-12-3");
        assert_eq!(named("CON.txt"), "T-12-3-CON.txt");
        let long = named(&format!("{}.diff", "😀".repeat(90)));
        assert!(long.len() <= NAME_BYTES);
        assert!(long.ends_with(".diff"));
    }
}
