/*******************************************************************************
 * Copyright (c) 2024 Cénotélie Opérations SAS (cenotelie.fr)
 ******************************************************************************/

//! Data types around CVEs

use semver::Version;
use serde_derive::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdvisorySeverity {
    #[serde(rename = "type")]
    pub type_value: String,
    pub score: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdvisoryAffectedPackage {
    pub ecosystem: String,
    pub name: String,
    pub purl: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdvisoryAffectedRangeEvent {
    introduced: Option<String>,
    fixed: Option<String>,
    last_affected: Option<String>,
    limit: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdvisoryAffectedRange {
    #[serde(rename = "type")]
    pub type_value: String,
    pub repo: Option<String>,
    pub events: Vec<AdvisoryAffectedRangeEvent>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdvisoryAffected {
    pub package: AdvisoryAffectedPackage,
    #[serde(default)]
    pub severity: Vec<AdvisorySeverity>,
    #[serde(default)]
    pub ranges: Vec<AdvisoryAffectedRange>,
    #[serde(default)]
    pub versions: Vec<String>,
    pub ecosystem_specific: Option<serde_json::Value>,
    pub database_specific: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdvisoryReference {
    #[serde(rename = "type")]
    pub type_value: String,
    pub url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdvisoryCredit {
    pub name: String,
    pub contact: Vec<String>,
    #[serde(rename = "type")]
    pub type_value: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Advisory {
    pub schema_version: Option<String>,
    pub id: String,
    pub modified: String,
    pub published: String,
    #[serde(default)]
    pub withdrawn: String,
    #[serde(default)]
    pub aliases: Vec<String>,
    #[serde(default)]
    pub related: Vec<String>,
    #[serde(default)]
    pub summary: String,
    #[serde(default)]
    pub detail: String,
    #[serde(default)]
    pub severity: Vec<AdvisorySeverity>,
    #[serde(default)]
    pub affected: Vec<AdvisoryAffected>,
    #[serde(default)]
    pub references: Vec<AdvisoryReference>,
    #[serde(default)]
    pub credits: Vec<AdvisoryCredit>,
    pub database_specific: Option<serde_json::Value>,
}

/// A range of affected versions
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SimpleAdvisoryRange {
    /// Minimal affected version
    pub introduced: Version,
    /// The minimal fixing version
    pub fixed: Option<Version>,
    /// The last affected version
    pub last_affected: Option<Version>,
}

impl SimpleAdvisoryRange {
    /// Gets whether the specified version is affected by this range
    #[must_use]
    pub fn affects(&self, version: &Version) -> bool {
        if let Some(fixed) = self.fixed.as_ref() {
            version >= &self.introduced && version < fixed
        } else if let Some(last_affected) = self.last_affected.as_ref() {
            version >= &self.introduced && version <= last_affected
        } else {
            version >= &self.introduced
        }
    }
}

/// A simplified advisory against a crate to be used in services
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SimpleAdvisory {
    /// The affected package
    pub package: String,
    /// The identifier for the advisory
    pub id: String,
    /// Datetime on initial publication
    pub published: String,
    /// Datetime on last modification
    pub modified: String,
    /// The summary for the advisory
    pub summary: String,
    /// The affected ranges
    pub ranges: Vec<SimpleAdvisoryRange>,
    /// The affected versions
    pub versions: Vec<Version>,
}

impl SimpleAdvisory {
    /// Gets whether the specified version is affected by this advisory
    #[must_use]
    pub fn affects(&self, version: &Version) -> bool {
        if self.versions.iter().any(|v| v == version) {
            return true;
        }
        self.ranges.iter().any(|range| range.affects(version))
    }
}

impl TryFrom<Advisory> for SimpleAdvisory {
    type Error = ();

    fn try_from(advisory: Advisory) -> Result<Self, Self::Error> {
        if !advisory.withdrawn.is_empty() {
            return Err(());
        }
        let affected = advisory
            .affected
            .into_iter()
            .find(|affected| affected.package.ecosystem == "crates.io")
            .ok_or(())?;
        let ranges = affected
            .ranges
            .into_iter()
            .filter(|range| range.type_value == "SEMVER")
            .map(|range| {
                let introduced = range.events.iter().find_map(|event| event.introduced.as_ref()).ok_or(())?;
                let fixed = range.events.iter().find_map(|event| event.fixed.as_ref());
                let last_affected = range.events.iter().find_map(|event| event.last_affected.as_ref());
                Ok::<_, ()>(SimpleAdvisoryRange {
                    introduced: introduced.parse().map_err(|_| ())?,
                    fixed: fixed.map(|v| v.parse().map_err(|_| ())).transpose()?,
                    last_affected: last_affected.map(|v| v.parse().map_err(|_| ())).transpose()?,
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        let versions = affected
            .versions
            .iter()
            .map(|v| v.parse())
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| ())?;
        Ok(Self {
            package: affected.package.name,
            id: advisory.id,
            published: advisory.published,
            modified: advisory.modified,
            summary: advisory.summary,
            ranges,
            versions,
        })
    }
}
