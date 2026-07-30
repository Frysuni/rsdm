#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Session {
    pub id: String,
    pub name: String,
    pub comment: Option<String>,
    pub exec: String,
    pub desktop_names: Vec<String>,
    pub source_path: String,
}

impl Session {
    pub fn new(
        id: impl Into<String>,
        name: impl Into<String>,
        exec: impl Into<String>,
        source_path: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            comment: None,
            exec: exec.into(),
            desktop_names: Vec::new(),
            source_path: source_path.into(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionExit {
    Success,
    Failed(i32),
    Signaled(i32),
}
