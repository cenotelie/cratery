/*******************************************************************************
 * Copyright (c) 2024 Cénotélie Opérations SAS (cenotelie.fr)
 ******************************************************************************/

//! Data model

pub mod auth;
pub mod cargo;
pub mod config;
pub mod deps;
pub mod errors;
pub mod namegen;
pub mod osv;
pub mod packages;
pub mod stats;

use chrono::NaiveDateTime;
use serde_derive::{Deserialize, Serialize};

use crate::utils::comma_sep_to_vec;

/// The object representing the application version
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct AppVersion {
    /// The changeset that was used to build the app
    pub commit: String,
    /// The version tag, if any
    pub tag: String,
}

/// Information about the registry, as exposed on the web API
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct RegistryInformation {
    /// The name to use for the registry in cargo and git config
    #[serde(rename = "registryName")]
    pub registry_name: String,
}

/// A couple describing a crate with its name and the associated version
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct CrateVersion {
    /// The name of the crate
    pub name: String,
    /// The crate's version
    pub version: String,
}

impl From<JobCrate> for CrateVersion {
    fn from(value: JobCrate) -> Self {
        CrateVersion {
            name: value.name,
            version: value.version,
        }
    }
}

impl From<CrateVersionDepsCheckState> for CrateVersion {
    fn from(value: CrateVersionDepsCheckState) -> Self {
        CrateVersion {
            name: value.name,
            version: value.version,
        }
    }
}

/// The description of a crate for a job
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct JobCrate {
    /// The name of the crate
    pub name: String,
    /// The crate's version
    pub version: String,
    /// The targets for the crate
    pub targets: Vec<String>,
}

impl From<CrateVersionDepsCheckState> for JobCrate {
    fn from(value: CrateVersionDepsCheckState) -> Self {
        JobCrate {
            name: value.name,
            version: value.version,
            targets: comma_sep_to_vec(&value.targets),
        }
    }
}

/// Metadata about a crate version and its analysis state
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct CrateVersionDepsCheckState {
    /// The name of the crate
    pub name: String,
    /// The crate's version
    pub version: String,
    /// Whether the version has outdated dependencies
    #[serde(rename = "depsHasOutdated")]
    pub deps_has_outdated: bool,
    /// When this crate version was last checked
    #[serde(rename = "depsLastCheck")]
    pub deps_last_check: NaiveDateTime,
    /// The targets associated with the crate
    pub targets: String,
}
