/*******************************************************************************
 * Copyright (c) 2024 Cénotélie Opérations SAS (cenotelie.fr)
 ******************************************************************************/

//! Module for definition of API objects

use std::collections::HashMap;
use std::io::Cursor;

use byteorder::{LittleEndian, ReadBytesExt};
use chrono::NaiveDateTime;
use data_encoding::HEXLOWER;
use ring::digest::{Context, SHA256};
use serde_derive::{Deserialize, Serialize};

use super::cargo::{CrateMetadata, Dependency, RegistryUser};
use crate::utils::apierror::ApiError;

/// Represents the possible access for an authenticated user
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct AuthenticatedUser {
    /// The principal (email of the user)
    pub principal: String,
    /// Whether a crate can be uploaded
    #[serde(rename = "canWrite")]
    pub can_write: bool,
    /// Whether administration can be done
    #[serde(rename = "canAdmin")]
    pub can_admin: bool,
}

/// A token for a registry user
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct RegistryUserToken {
    /// The unique identifier
    pub id: i64,
    /// The token name
    pub name: String,
    /// The last time the token was used
    #[serde(rename = "lastUsed")]
    pub last_used: NaiveDateTime,
    /// Whether a crate can be uploaded using this token
    #[serde(rename = "canWrite")]
    pub can_write: bool,
    /// Whether administration can be done using this token through the API
    #[serde(rename = "canAdmin")]
    pub can_admin: bool,
}

/// A token for a registry user
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct RegistryUserTokenWithSecret {
    /// The unique identifier
    pub id: i64,
    /// The token name
    pub name: String,
    /// The value for the token
    pub secret: String,
    /// The last time the token was used
    #[serde(rename = "lastUsed")]
    pub last_used: NaiveDateTime,
    /// Whether a crate can be uploaded using this token
    #[serde(rename = "canWrite")]
    pub can_write: bool,
    /// Whether administration can be done using this token through the API
    #[serde(rename = "canAdmin")]
    pub can_admin: bool,
}

/// The metadata for a crate inside the index
#[derive(Default, Debug, Clone, Serialize, Deserialize)]
pub struct CrateMetadataIndex {
    /// The name of the package
    pub name: String,
    /// The version of the package this row is describing.
    /// This must be a valid version number according to the Semantic
    /// Versioning 2.0.0 spec at [https://semver.org/](https://semver.org/).
    pub vers: String,
    /// Array of direct dependencies of the package
    pub deps: Vec<DependencyIndex>,
    /// A SHA256 checksum of the `.crate` file.
    pub cksum: String,
    /// Set of features defined for the package.
    /// Each feature maps to an array of features or dependencies it enables.
    pub features: HashMap<String, Vec<String>>,
    /// Boolean of whether or not this version has been yanked.
    pub yanked: bool,
    /// The `links` string value from the package's manifest, or null if not
    /// specified. This field is optional and defaults to null.
    pub links: Option<String>,
}

/// A dependency for a crate in the index
#[derive(Default, Debug, Clone, Serialize, Deserialize)]
pub struct DependencyIndex {
    /// Name of the dependency.
    /// If the dependency is renamed from the original package name,
    /// this is the original name. The new package name is stored in
    /// the `package` field.
    pub name: String,
    /// The semver requirement for this dependency.
    /// This must be a valid version requirement defined at
    /// [https://github.com/steveklabnik/semver#requirements](https://github.com/steveklabnik/semver#requirements).
    pub req: String,
    /// Array of features (as strings) enabled for this dependency
    pub features: Vec<String>,
    /// Boolean of whether or not this is an optional dependency
    pub optional: bool,
    /// Boolean of whether or not default features are enabled
    pub default_features: bool,
    /// The target platform for the dependency.
    /// null if not a target dependency.
    /// Otherwise, a string such as "cfg(windows)".
    pub target: Option<String>,
    /// The dependency kind.
    /// "dev", "build", or "normal".
    pub kind: String,
    /// The URL of the index of the registry where this dependency is
    /// from as a string. If not specified or null, it is assumed the
    /// dependency is in the current registry.
    pub registry: Option<String>,
    /// If the dependency is renamed, this is a string of the new
    /// package name. If not specified or null, this dependency is not
    /// renamed.
    pub package: Option<String>,
}

impl From<&Dependency> for DependencyIndex {
    fn from(dep: &Dependency) -> Self {
        Self {
            name: dep.name.clone(),
            req: dep.version_req.clone(),
            features: dep.features.clone(),
            optional: dep.optional,
            default_features: dep.default_features,
            target: dep.target.clone(),
            kind: dep.kind.clone(),
            registry: dep.registry.clone(),
            package: dep.explicit_name_in_toml.clone(),
        }
    }
}

/// Gets the last info for a crate
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CrateInfo {
    /// The last metadata, if any
    pub metadata: Option<CrateMetadata>,
    /// Gets the versions in the index
    pub versions: Vec<CrateInfoVersion>,
}

/// The data for a crate version
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CrateInfoVersion {
    /// The data from the index
    pub index: CrateMetadataIndex,
    /// The upload date time
    pub upload: NaiveDateTime,
    /// The user that uploaded the version
    #[serde(rename = "uploadedBy")]
    pub uploaded_by: RegistryUser,
    /// Whether documentation was generated for this version
    #[serde(rename = "hasDocs")]
    pub has_docs: bool,
    /// Whether the documentation generation was attempted
    #[serde(rename = "docGenAttempted")]
    pub doc_gen_attempted: bool,
    /// The number of times this version was downloaded
    #[serde(rename = "downloadCount")]
    pub download_count: i64,
}

/// The upload data for publishing a crate
pub struct CrateUploadData {
    /// The metadata
    pub metadata: CrateMetadata,
    /// The content of the .crate package
    pub content: Vec<u8>,
}

impl CrateUploadData {
    /// Deserialize the content of an input payload
    #[allow(clippy::cast_possible_truncation)]
    pub fn new(buffer: &[u8]) -> Result<CrateUploadData, ApiError> {
        let mut cursor = Cursor::new(buffer);
        // read the metadata
        let metadata_length = u64::from(cursor.read_u32::<LittleEndian>()?);
        let metadata_buffer = &buffer[4..((4 + metadata_length) as usize)];
        let metadata = serde_json::from_slice(metadata_buffer)?;
        // read the content
        cursor.set_position(4 + metadata_length);
        let content_length = cursor.read_u32::<LittleEndian>()? as usize;
        let mut content = vec![0_u8; content_length];
        content.copy_from_slice(&buffer[((4 + metadata_length + 4) as usize)..]);
        Ok(CrateUploadData { metadata, content })
    }

    /// Builds the metadata to be index for this version
    pub fn build_index_data(&self) -> CrateMetadataIndex {
        let cksum = sha256(&self.content);
        CrateMetadataIndex {
            name: self.metadata.name.clone(),
            vers: self.metadata.vers.clone(),
            deps: self.metadata.deps.iter().map(DependencyIndex::from).collect(),
            cksum,
            features: self.metadata.features.clone(),
            yanked: false,
            links: self.metadata.links.clone(),
        }
    }
}

/// Computes the SHA256 digest of bytes
pub fn sha256(buffer: &[u8]) -> String {
    let mut context = Context::new(&SHA256);
    context.update(buffer);
    let digest = context.finish();
    HEXLOWER.encode(digest.as_ref())
}

/// Represents a documentation generation job
#[derive(Debug, Clone)]
pub struct DocsGenerationJob {
    /// The name of the target crate
    pub crate_name: String,
    /// The version of the target crate
    pub crate_version: String,
}
