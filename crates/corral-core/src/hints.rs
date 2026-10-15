//! Optional task classification used by model routing.

use std::{fmt, str::FromStr};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TaskKind {
    Ui,
    Backend,
    Logic,
    Docs,
    Test,
    Refactor,
    Research,
}

impl fmt::Display for TaskKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl TaskKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Ui => "ui",
            Self::Backend => "backend",
            Self::Logic => "logic",
            Self::Docs => "docs",
            Self::Test => "test",
            Self::Refactor => "refactor",
            Self::Research => "research",
        }
    }
}

impl FromStr for TaskKind {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "ui" => Ok(Self::Ui),
            "backend" => Ok(Self::Backend),
            "logic" => Ok(Self::Logic),
            "docs" => Ok(Self::Docs),
            "test" => Ok(Self::Test),
            "refactor" => Ok(Self::Refactor),
            "research" => Ok(Self::Research),
            _ => Err(format!(
                "unknown kind '{s}' (ui, backend, logic, docs, test, refactor, research)"
            )),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TaskSize {
    Xs,
    S,
    M,
    L,
    Xl,
}

impl fmt::Display for TaskSize {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Xs => "xs",
            Self::S => "s",
            Self::M => "m",
            Self::L => "l",
            Self::Xl => "xl",
        })
    }
}

impl FromStr for TaskSize {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "xs" => Ok(Self::Xs),
            "s" => Ok(Self::S),
            "m" => Ok(Self::M),
            "l" => Ok(Self::L),
            "xl" => Ok(Self::Xl),
            _ => Err(format!("unknown size '{s}' (xs, s, m, l, xl)")),
        }
    }
}
