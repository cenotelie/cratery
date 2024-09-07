/*******************************************************************************
 * Copyright (c) 2024 Cénotélie Opérations SAS (cenotelie.fr)
 ******************************************************************************/

//! Data types around documentation generation

use chrono::NaiveDateTime;
use serde_derive::{Deserialize, Serialize};

use super::cargo::RegistryUser;

/// The state of a documentation generation job
#[derive(Debug, Copy, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DocGenJobState {
    /// The job is queued
    Queued,
    /// The worker is working on this job
    Working,
    /// The job is finished and succeeded
    Success,
    /// The worker failed to complete this job
    Failure,
}

impl DocGenJobState {
    /// Gets whether the state indicates that the job is finished
    #[must_use]
    pub fn is_final(self) -> bool {
        matches!(self, Self::Success | Self::Failure)
    }

    /// Gets the serialisation value for the database
    #[must_use]
    pub fn value(self) -> i64 {
        match self {
            Self::Queued => 0,
            Self::Working => 1,
            Self::Success => 2,
            Self::Failure => 3,
        }
    }
}

impl From<i64> for DocGenJobState {
    fn from(value: i64) -> Self {
        match value {
            1 => Self::Working,
            2 => Self::Success,
            3 => Self::Failure,
            _ => Self::Queued,
        }
    }
}

/// The trigger for the job
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum DocGenTrigger {
    /// The upload of the crate version
    Upload { by: RegistryUser },
    /// The manual request of a user
    Manual { by: RegistryUser },
    /// The documentation was detected as missing on launch
    MissingOnLaunch,
}

impl DocGenTrigger {
    /// Gets the serialisation value for the database
    #[must_use]
    pub fn value(&self) -> i64 {
        match self {
            Self::Upload { by: _ } => 0,
            Self::Manual { by: _ } => 1,
            Self::MissingOnLaunch => 2,
        }
    }

    /// Gets the user that triggered the job, if any
    #[must_use]
    pub fn by(&self) -> Option<&RegistryUser> {
        match self {
            Self::Upload { by } | Self::Manual { by } => Some(by),
            Self::MissingOnLaunch => None,
        }
    }
}

impl From<(i64, Option<RegistryUser>)> for DocGenTrigger {
    fn from(spec: (i64, Option<RegistryUser>)) -> Self {
        match spec {
            (0, Some(by)) => Self::Upload { by },
            (1, Some(by)) => Self::Manual { by },
            _ => Self::MissingOnLaunch,
        }
    }
}

/// A documentation generation job
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocGenJob {
    /// The unique identifier
    pub id: i64,
    /// The name of the crate
    pub package: String,
    /// The crate's version
    pub version: String,
    /// The targets for the crate
    pub targets: Vec<String>,
    /// The state of the job
    pub state: DocGenJobState,
    /// Timestamp when the job was queued
    #[serde(rename = "queuedOn")]
    pub queued_on: NaiveDateTime,
    /// Timestamp when the job started execution
    #[serde(rename = "startedOn")]
    pub started_on: NaiveDateTime,
    /// Timestamp when the job terminated
    #[serde(rename = "finishedOn")]
    pub finished_on: NaiveDateTime,
    /// Timestamp the last time this job was touched
    #[serde(rename = "lastUpdate")]
    pub last_update: NaiveDateTime,
    /// The event that triggered the job
    pub trigger: DocGenTrigger,
}

/// An update to a documentation generation job
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocGenJobUpdate {
    /// The unique identifier of the associated job
    #[serde(rename = "jobId")]
    pub job_id: i64,
    /// The new state for the job
    pub state: DocGenJobState,
    /// The update timestamp
    #[serde(rename = "lastUpdate")]
    pub last_update: NaiveDateTime,
    /// The appended log, if any
    pub log: Option<String>,
}

/// An event for the documentation generation service
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum DocGenEvent {
    /// An new job was queued
    Queued(Box<DocGenJob>),
    /// A job was updated
    Update(DocGenJobUpdate),
}
