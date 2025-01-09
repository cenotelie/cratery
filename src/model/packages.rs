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
    /// Whether the entire package is deprecated
    #[serde(rename = "isDeprecated")]
    pub is_deprecated: bool,
    /// Whether versions of this crate can be completely removed, not simply yanked
    #[serde(rename = "canRemove")]
    pub can_remove: bool,
    /// Gets the versions in the index
    pub versions: Vec<CrateInfoVersion>,
    /// The build targets to use (for docs generation and deps analysis)
    pub targets: Vec<CrateInfoTarget>,
    /// The required capabilities for docs generation
    pub capabilities: Vec<String>,
}

/// A build targets to use (for docs generation and deps analysis)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CrateInfoTarget {
    /// The target triple
    pub target: String,
    /// Whether to require a native toolchain for this target
    #[serde(rename = "docsUseNative")]
    pub docs_use_native: bool,
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
    /// The number of times this version was downloaded
    #[serde(rename = "downloadCount")]
    pub download_count: i64,
    /// Gets the last time this crate version had its dependencies automatically checked
    #[serde(rename = "depsLastCheck")]
    pub deps_last_check: NaiveDateTime,
    /// Flag whether this crate has outdated dependencies
    #[serde(rename = "depsHasOutdated")]
    pub deps_has_outdated: bool,
    /// Flag whether CVEs have been filed against dependencies of this crate
    #[serde(rename = "depsHasCVEs")]
    pub deps_has_cves: bool,
    /// The documentation status
    pub docs: Vec<CrateInfoVersionDocs>,
}

/// The documentation status for a crate version
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CrateInfoVersionDocs {
    /// The corresponding target
    pub target: String,
    /// Whether the documentation generation was attempted
    #[serde(rename = "isAttempted")]
    pub is_attempted: bool,
    /// Whether documentation was generated for this target
    #[serde(rename = "isPresent")]
    pub is_present: bool,
}
