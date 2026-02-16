//! CRDT Merge Error - Explicit error for type mismatches (TigerStyle)

/// Error returned when attempting to merge two CrdtValues of different types.
/// This makes the conflict explicit rather than silently discarding data.
#[derive(Debug, Clone)]
pub struct CrdtTypeMismatchError {
    /// The type name of the first value (self)
    pub self_type: &'static str,
    /// The type name of the other value
    pub other_type: &'static str,
}

impl std::fmt::Display for CrdtTypeMismatchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "CRDT type mismatch: cannot merge {} with {}",
            self.self_type, self.other_type
        )
    }
}

impl std::error::Error for CrdtTypeMismatchError {}
