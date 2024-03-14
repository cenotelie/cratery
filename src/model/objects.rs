/*******************************************************************************
 * Copyright (c) 2024 Cénotélie Opérations SAS (cenotelie.fr)
 ******************************************************************************/

//! Module for definition of API objects

use std::collections::HashMap;
use std::io::Cursor;

use byteorder::{LittleEndian, ReadBytesExt};
use cenotelie_lib_apierror::{error_invalid_request, specialize, ApiError};
use chrono::NaiveDateTime;
use data_encoding::HEXLOWER;
use ring::digest::{Context, SHA256};
use serde_derive::{Deserialize, Serialize};

/// A user for the registry
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct RegistryUser {
    /// The unique identifier
    pub id: i64,
    /// Whether this is an active user
    #[serde(rename = "isActive")]
    pub is_active: bool,
    /// The email, unique for each user
    pub email: String,
    /// The login to be used for token authentication
    pub login: String,
    /// The user's name
    pub name: String,
    /// The roles for the user
    pub roles: String,
}

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

/// A crate to appear in search results
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct SearchResultCrate {
    /// Name of the crate
    pub name: String,
    /// The highest version available
    pub max_version: String,
    /// Textual description of the crate
    pub description: String,
}

/// The metadata of the search results
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct SearchResultsMeta {
    /// Total number of results available on the server
    pub total: usize,
}

/// The search results for crates
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct SearchResults {
    /// The crates
    pub crates: Vec<SearchResultCrate>,
    /// The metadata
    pub meta: SearchResultsMeta,
}

/// A set of errors as a response for the web API
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ApiResponseErrors {
    /// The individual errors
    pub errors: Vec<ApiResponseError>,
}

/// An error response for the web API
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ApiResponseError {
    /// The details for the error
    pub detail: String,
}

impl From<ApiError> for ApiResponseErrors {
    fn from(err: ApiError) -> Self {
        ApiResponseErrors {
            errors: vec![ApiResponseError { detail: err.to_string() }],
        }
    }
}

/// The metadata for a crate
#[derive(Default, Debug, Clone, Serialize, Deserialize)]
pub struct CrateMetadata {
    /// The name of the package
    pub name: String,
    /// The version of the package being published
    pub vers: String,
    /// Array of direct dependencies of the package
    pub deps: Vec<Dependency>,
    /// Set of features defined for the package.
    /// Each feature maps to an array of features or dependencies it enables.
    /// Cargo does not impose limitations on feature names, but crates.io
    /// requires alphanumeric ASCII, `_` or `-` characters.
    pub features: HashMap<String, Vec<String>>,
    /// List of strings of the authors.
    /// May be empty.
    pub authors: Vec<String>,
    /// Description field from the manifest.
    /// May be null. crates.io requires at least some content.
    pub description: Option<String>,
    /// String of the URL to the website for this package's documentation.
    /// May be null.
    pub documentation: Option<String>,
    /// String of the URL to the website for this package's home page.
    /// May be null.
    pub homepage: Option<String>,
    /// String of the content of the README file.
    /// May be null.
    pub readme: Option<String>,
    /// String of a relative path to a README file in the crate.
    /// May be null.
    pub readme_file: Option<String>,
    /// Array of strings of keywords for the package.
    pub keywords: Vec<String>,
    /// Array of strings of categories for the package.
    pub categories: Vec<String>,
    /// String of the license for the package.
    /// May be null. crates.io requires either `license` or `license_file` to be set.
    pub license: Option<String>,
    /// String of a relative path to a license file in the crate.
    /// May be null.
    pub license_file: Option<String>,
    /// String of the URL to the website for the source repository of this package.
    /// May be null.
    pub repository: String,
    /// Optional object of "status" badges. Each value is an object of
    /// arbitrary string to string mappings.
    /// crates.io has special interpretation of the format of the badges.
    pub badges: HashMap<String, serde_json::Value>,
    /// The `links` string value from the package's manifest, or null if not
    /// specified. This field is optional and defaults to null.
    pub links: Option<String>,
}

impl CrateMetadata {
    /// Validate the crate's metadata
    pub fn validate(&self) -> Result<CrateUploadResult, ApiError> {
        self.validate_name()?;
        self.validate_kind()?;
        Ok(CrateUploadResult::default())
    }

    /// Validates the package name
    fn validate_name(&self) -> Result<(), ApiError> {
        if self.name.is_empty() {
            return validation_error("Name must not be empty");
        }
        if self.name.len() > 64 {
            return validation_error("Name must not exceed 64 characters");
        }
        for (i, c) in self.name.chars().enumerate() {
            match (i, c) {
                (0, c) if !c.is_ascii_alphabetic() => {
                    return validation_error("Name must start with an ASCII letter");
                }
                (_, c) if !c.is_ascii_alphanumeric() && c != '-' && c != '_' => {
                    return validation_error("Name must only contain alphanumeric, -, _");
                }
                _ => { /* this is ok */ }
            }
        }
        Ok(())
    }

    /// Validate the kind field
    fn validate_kind(&self) -> Result<(), ApiError> {
        for dep in &self.deps {
            if dep.kind != "dev" && dep.kind != "build" && dep.kind != "normal" {
                return validation_error("kind for dependency must be either [normal, dev, build]");
            }
        }
        Ok(())
    }
}

/// Creates a validation error
pub fn validation_error(details: &str) -> Result<(), ApiError> {
    Err(specialize(error_invalid_request(), details.to_string()))
}

/// A dependency for a crate
#[derive(Default, Debug, Clone, Serialize, Deserialize)]
pub struct Dependency {
    /// Name of the dependency.
    /// If the dependency is renamed from the original package name,
    /// this is the original name. The new package name is stored in
    /// the `explicit_name_in_toml` field.
    pub name: String,
    /// The semver requirement for this dependency
    pub version_req: String,
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
    pub explicit_name_in_toml: Option<String>,
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

/// The result for the upload fo a crate
#[derive(Default, Debug, Serialize, Deserialize, Clone)]
pub struct CrateUploadResult {
    /// The warnings
    pub warnings: CrateUploadWarnings,
}

/// The warnings for the upload of a crate
#[derive(Default, Debug, Serialize, Deserialize, Clone)]
pub struct CrateUploadWarnings {
    /// Array of strings of categories that are invalid and ignored
    pub invalid_categories: Vec<String>,
    /// Array of strings of badge names that are invalid and ignored
    pub invalid_badges: Vec<String>,
    /// Array of strings of arbitrary warnings to display to the user
    pub other: Vec<String>,
}

/// The result for a yank operation
#[derive(Default, Debug, Serialize, Deserialize, Clone)]
pub struct YesNoResult {
    /// The value for the result
    pub ok: bool,
}

impl YesNoResult {
    /// Creates a new instance
    pub fn new() -> YesNoResult {
        YesNoResult { ok: true }
    }
}

/// The result for a yank operation
#[derive(Default, Debug, Serialize, Deserialize, Clone)]
pub struct YesNoMsgResult {
    /// The value for the result
    pub ok: bool,
    /// A string message that will be displayed
    pub msg: String,
}

impl YesNoMsgResult {
    /// Creates a new instance
    pub fn new(msg: String) -> YesNoMsgResult {
        YesNoMsgResult { ok: true, msg }
    }
}

/// The result when querying for owners
#[derive(Default, Debug, Serialize, Deserialize, Clone)]
pub struct OwnersQueryResult {
    /// The list of owners
    pub users: Vec<RegistryUser>,
}

/// The query for adding owners to a crate
#[derive(Default, Debug, Serialize, Deserialize, Clone)]
pub struct OwnersAddQuery {
    /// The login of the users
    pub users: Vec<String>,
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