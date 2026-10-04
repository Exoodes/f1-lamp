#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionKind {
    Practice,
    Qualifying,
    SprintQualifying,
    Sprint,
    Race,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Session {
    pub key: u32,
    pub kind: SessionKind,
    /// Unix seconds, UTC.
    pub start: i64,
    /// Unix seconds, UTC.
    pub end: i64,
}
