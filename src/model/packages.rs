/*******************************************************************************
 * Copyright (c) 2024 Cénotélie Opérations SAS (cenotelie.fr)
 ******************************************************************************/

//! Data types for crate information and description, in addition to Cargo types

use chrono::NaiveDateTime;
use serde_derive::{Deserialize, Serialize};

use super::cargo::{CrateMetadata, IndexCrateMetadata, RegistryUser};

/// Gets the last info for a crate
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CrateInfo {
    /// The last metadata, if any
    pub metadata: Option<CrateMetadata>,
    /// Gets the versions in the index
    pub versions: Vec<CrateInfoVersion>,
}

/// The data for a crate version
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CrateInfoVersion {
    /// The data from the index
    pub index: IndexCrateMetadata,
    /// The upload date time
    pub upload: NaiveDateTime,
    /// The user that uploaded the version
    #[serde(rename = "uploadedBy")]
    pub uploaded_by: RegistryUser,
    /// Whether documentation was generated for this version
    #[serde(rename = "hasDocs")]
    pub has_docs: bool,
    /// Whether the documentation generation was attempted
    #[serde(rename = "docGenAttempted")]
    pub doc_gen_attempted: bool,
    /// The number of times this version was downloaded
    #[serde(rename = "downloadCount")]
    pub download_count: i64,
}