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

use rand::distributions::Alphanumeric;
use rand::{thread_rng, Rng};
use serde_derive::{Deserialize, Serialize};

/// The object representing the application version
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct AppVersion {
    /// The changeset that was used to build the app
    pub commit: String,
    /// The version tag, if any
    pub tag: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct RegistryInformation {
    pub registry_name: String,
}

/// Generates a token
pub fn generate_token(length: usize) -> String {
    let rng = thread_rng();
    String::from_utf8(rng.sample_iter(&Alphanumeric).take(length).collect()).unwrap()
}

/// A couple describing a crate with its name and the associated version
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct CrateAndVersion {
    /// The name of the crate
    pub name: String,
    /// The crate's version
    pub version: String,
}

impl From<JobCrate> for CrateAndVersion {
    fn from(value: JobCrate) -> Self {
        CrateAndVersion {
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
