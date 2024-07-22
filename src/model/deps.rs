/*******************************************************************************
 * Copyright (c) 2024 Cénotélie Opérations SAS (cenotelie.fr)
 ******************************************************************************/

//! Data types around dependency analysis

use std::str::FromStr;

use serde_derive::{Deserialize, Serialize};

/// The kind of dependency
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum DependencyKind {
    /// A normal dependency
    #[serde(rename = "normal")]
    Normal,
    /// A dev dependency (for tests, etc.)
    #[serde(rename = "dev")]
    Dev,
    /// A build dependency (for build.rs)
    #[serde(rename = "build")]
    Build,
}

impl FromStr for DependencyKind {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "normal" => Ok(DependencyKind::Normal),
            "dev" => Ok(DependencyKind::Dev),
            "build" => Ok(DependencyKind::Build),
            _ => Err(()),
        }
    }
}

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
