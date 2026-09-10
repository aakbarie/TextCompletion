#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Delimiter {
    Space,
    Tab,
    Enter,
}

impl Delimiter {
    pub fn as_char(self) -> char {
        match self {
            Self::Space => ' ',
            Self::Tab => '\t',
            Self::Enter => '\n',
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExpansionPlan {
    pub backspaces: usize,
    pub replacement: String,
    pub delimiter: Delimiter,
}

pub fn build_expansion_plan(
    trigger: &str,
    replacement: impl Into<String>,
    delimiter: Delimiter,
) -> ExpansionPlan {
    ExpansionPlan {
        backspaces: trigger.chars().count(),
        replacement: replacement.into(),
        delimiter,
    }
}
