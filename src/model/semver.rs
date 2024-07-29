/*******************************************************************************
 * Copyright (c) 2024 Cénotélie Opérations SAS (cenotelie.fr)
 ******************************************************************************/

//! Data types for semantic versioning

use std::fmt::Display;
use std::str::FromStr;

use serde_derive::{Deserialize, Serialize};

mod semver_version_serializer {
    //! Serialize/Deserialize support for `semver::Version`

    use serde::de::Error;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    /// Serialize a `semver::Version`
    ///
    /// # Errors
    ///
    /// Return a deserialization error from serde.
    pub fn serialize<S: Serializer>(data: &semver::Version, serializer: S) -> Result<S::Ok, S::Error> {
        data.to_string().serialize(serializer)
    }

    /// Deserialize a chrono `semver::Version`
    ///
    /// # Errors
    ///
    /// Return a deserialization error from serde.
    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<semver::Version, D::Error> {
        let text: String = Deserialize::deserialize(deserializer)?;
        text.parse().map_err(D::Error::custom)
    }
}

mod semver_version_req_serializer {
    //! Serialize/Deserialize support for `semver::VersionReq`

    use serde::de::Error;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    /// Serialize a `semver::VersionReq`
    ///
    /// # Errors
    ///
    /// Return a deserialization error from serde.
    pub fn serialize<S: Serializer>(data: &semver::VersionReq, serializer: S) -> Result<S::Ok, S::Error> {
        data.to_string().serialize(serializer)
    }

    /// Deserialize a chrono `semver::VersionReq`
    ///
    /// # Errors
    ///
    /// Return a deserialization error from serde.
    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<semver::VersionReq, D::Error> {
        let text: String = Deserialize::deserialize(deserializer)?;
        text.parse().map_err(D::Error::custom)
    }
}

/// The representation of a version number according to semver
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[repr(transparent)]
#[serde(transparent)]
pub struct SemverVersion(#[serde(with = "semver_version_serializer")] pub semver::Version);

impl From<semver::Version> for SemverVersion {
    fn from(value: semver::Version) -> Self {
        Self(value)
    }
}

impl FromStr for SemverVersion {
    type Err = <semver::Version as FromStr>::Err;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self(semver::Version::from_str(s)?))
    }
}

impl Display for SemverVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

/// The representation of the requirement for a version  according to semver
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[repr(transparent)]
#[serde(transparent)]
pub struct SemverVersionReq(#[serde(with = "semver_version_req_serializer")] pub semver::VersionReq);

impl From<semver::VersionReq> for SemverVersionReq {
    fn from(value: semver::VersionReq) -> Self {
        Self(value)
    }
}

impl FromStr for SemverVersionReq {
    type Err = <semver::VersionReq as FromStr>::Err;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self(semver::VersionReq::from_str(s)?))
    }
}

impl Display for SemverVersionReq {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}
