/*******************************************************************************
 * Copyright (c) 2024 Cénotélie Opérations SAS (cenotelie.fr)
 ******************************************************************************/

//! Data types around dependency analysis

use serde_derive::{Deserialize, Serialize};

use super::cargo::DependencyKind;

/// The information about a dependency, resulting from an analysis
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DependencyInfo {
    /// URI for the owning registry, `None` for the local one
    pub registry: Option<String>,
    /// The name of the package
    pub package: String,
    /// The semver requirement for this dependency
    pub required: String,
    /// The kind of dependency
    pub kind: DependencyKind,
    /// The last known version
    #[serde(rename = "lastVersion")]
    pub last_version: String,
    /// Whether the requirement leads to the resolution of an outdated version
    #[serde(rename = "isOudated")]
    pub is_outdated: bool,
}
