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
