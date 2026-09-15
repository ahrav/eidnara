//! Maps a SQLite `OperationInterrupted` failure to a budget stop so every bounded row scan ends the same way when the connection's progress handler fires.

use crate::ProjectionError;

pub(crate) enum ScanStop {
    /// The request budget or the connection's interrupt ended the statement.
    Budget,
    Projection(ProjectionError),
}

impl From<rusqlite::Error> for ScanStop {
    fn from(error: rusqlite::Error) -> Self {
        if storage::is_interrupted(&error) {
            Self::Budget
        } else {
            Self::Projection(error.into())
        }
    }
}

impl From<ProjectionError> for ScanStop {
    fn from(error: ProjectionError) -> Self {
        Self::Projection(error)
    }
}
