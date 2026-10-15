use serde::{Deserialize, Serialize};

use crate::{Artifact, ArtifactKind};

/// Storage lifetime classification, independent of pin protection in operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ArtifactTier {
    Keep,
    Normal,
    Transient,
}

impl ArtifactTier {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Keep => "keep",
            Self::Normal => "normal",
            Self::Transient => "transient",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "keep" => Some(Self::Keep),
            "normal" => Some(Self::Normal),
            "transient" => Some(Self::Transient),
            _ => None,
        }
    }
}

impl Artifact {
    /// Explicit classification wins; operations separately protect pinned artifacts.
    pub fn tier(&self) -> ArtifactTier {
        if let Some(tier) = self.tier_override {
            return tier;
        }
        match self.kind {
            ArtifactKind::Report
            | ArtifactKind::Summary
            | ArtifactKind::Plan
            | ArtifactKind::Screenshot => return ArtifactTier::Keep,
            ArtifactKind::Log => return ArtifactTier::Transient,
            ArtifactKind::Diff => return ArtifactTier::Normal,
            ArtifactKind::Evidence | ArtifactKind::Other => {}
        }
        let title = self.title.to_ascii_lowercase();
        let path = std::path::Path::new(&self.path)
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_ascii_lowercase();
        let source = std::path::Path::new(&self.source)
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_ascii_lowercase();
        let names = [&path, &source, &title];
        let has_ext = |extensions: &[&str]| {
            names
                .iter()
                .any(|name| extensions.iter().any(|ext| name.ends_with(ext)))
        };
        if has_ext(&[".png", ".jpg", ".jpeg", ".gif", ".webp", ".bmp", ".svg"])
            || (has_ext(&[".md", ".markdown", ".html", ".htm"])
                && names.iter().any(|name| {
                    name.split(|c: char| !c.is_ascii_alphanumeric())
                        .any(|word| {
                            matches!(
                                word,
                                "review"
                                    | "acceptance"
                                    | "report"
                                    | "summary"
                                    | "plan"
                                    | "screenshot"
                            )
                        })
                }))
        {
            ArtifactTier::Keep
        } else if has_ext(&[".log", ".ansi"]) || (self.bytes >= 1_000_000 && has_ext(&[".json"])) {
            ArtifactTier::Transient
        } else {
            ArtifactTier::Normal
        }
    }
}
