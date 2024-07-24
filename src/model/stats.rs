/*******************************************************************************
 * Copyright (c) 2024 Cénotélie Opérations SAS (cenotelie.fr)
 ******************************************************************************/

//! Data types for global statistics

use serde_derive::{Deserialize, Serialize};

/// The global stats for the registry
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct GlobalStats {
    /// Total number of downloads
    #[serde(rename = "totalDownloads")]
    pub total_downloads: i64,
    /// Total number of crates
    #[serde(rename = "totalCrates")]
    pub total_crates: i64,
    /// The newests crate in the registry
    #[serde(rename = "cratesNewest")]
    pub crates_newest: Vec<CrateLink>,
    /// The most downloaded crates in the registry
    #[serde(rename = "cratesMostDownloaded")]
    pub crates_most_downloaded: Vec<CrateLink>,
    /// the last updated crates in the registry
    #[serde(rename = "cratesLastUpdated")]
    pub crates_last_updated: Vec<CrateLink>,
}

/// A link to a crate
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct CrateLink {
    /// The name of the crate
    pub name: String,
    /// The crate's version
    pub version: String,
}
