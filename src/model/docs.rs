/*******************************************************************************
 * Copyright (c) 2024 Cénotélie Opérations SAS (cenotelie.fr)
 ******************************************************************************/

//! Data types around documentation generation

use chrono::NaiveDateTime;
use serde_derive::{Deserialize, Serialize};

use super::JobCrate;

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

/// A documentation generation job
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocGenJob {
    /// The specification for the job
    pub spec: JobCrate,
    /// The state of the job
    pub state: DocGenJobState,
    /// Timestamp the last time this job was touched
    #[serde(rename = "lastUpdate")]
    pub last_update: NaiveDateTime,
    /// The output log, if any
    pub output: String,
}
