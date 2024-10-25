/*******************************************************************************
 * Copyright (c) 2024 Cénotélie Opérations SAS (cenotelie.fr)
 ******************************************************************************/

//! Data model

pub mod auth;
pub mod cargo;
pub mod config;
pub mod deps;
pub mod docs;
pub mod errors;
pub mod namegen;
pub mod osv;
pub mod packages;
pub mod stats;
pub mod worker;

use auth::TokenUsage;
use serde_derive::{Deserialize, Serialize};

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
    /// The version of the locally installed toolchain
    #[serde(rename = "toolchainVersionStable")]
    pub toolchain_version_stable: semver::Version,
    /// The version of the locally installed toolchain
    #[serde(rename = "toolchainVersionNightly")]
    pub toolchain_version_nightly: semver::Version,
    /// The host target of the locally installed toolchain
    #[serde(rename = "toolchainHost")]
    pub toolchain_host: String,
    /// The known built-in targets in rustc
    #[serde(rename = "toolchainTargets")]
    pub toolchain_targets: Vec<String>,
}

/// A couple describing a crate with its name and the associated version
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct CrateVersion {
    /// The name of the crate
    pub package: String,
    /// The crate's version
    pub version: String,
}

/// An event can be handled asynchronously by the application
#[derive(Debug, Clone)]
pub enum AppEvent {
    /// The use of a token to authenticate
    TokenUse(TokenUsage),
    /// The download of a crate
    CrateDownload(CrateVersion),
}

/// The modifier for the stable channel
pub const CHANNEL_STABLE: &str = "+stable";

/// The modifier for the nightly channel
pub const CHANNEL_NIGHTLY: &str = "+nightly";
