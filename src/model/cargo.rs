/*******************************************************************************
 * Copyright (c) 2024 Cénotélie Opérations SAS (cenotelie.fr)
 ******************************************************************************/

//! Data model for the Cargo web API

use std::collections::HashMap;
use std::io::Cursor;
use std::str::FromStr;

use byteorder::{LittleEndian, ReadBytesExt};
use data_encoding::HEXLOWER;
use ring::digest::{Context, SHA256};
use serde_derive::{Deserialize, Serialize};

use crate::utils::apierror::{error_invalid_request, specialize, ApiError};

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

/// The query for adding/removing owners to a crate
#[derive(Default, Debug, Serialize, Deserialize, Clone)]
pub struct OwnersChangeQuery {
    /// The login of the users
    pub users: Vec<String>,
}

/// A user for the registry
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct RegistryUser {
    /// The unique identifier
    /// Expected for Cargo
    pub id: i64,
    /// Whether this is an active user
    #[serde(rename = "isActive")]
    pub is_active: bool,
    /// The email, unique for each user
    pub email: String,
    /// The login to be used for token authentication
    /// Expected for Cargo
    pub login: String,
    /// The user's name
    /// Expected for Cargo
    pub name: String,
    /// The roles for the user
    pub roles: String,
}

/// The metadata for a crate
#[derive(Default, Debug, Clone, Serialize, Deserialize)]
pub struct CrateMetadata {
    /// The name of the package
    pub name: String,
    /// The version of the package being published
    pub vers: String,
    /// Array of direct dependencies of the package
    pub deps: Vec<CrateMetadataDependency>,
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
    pub repository: Option<String>,
    /// Optional object of "status" badges. Each value is an object of
    /// arbitrary string to string mappings.
    /// crates.io has special interpretation of the format of the badges.
    pub badges: HashMap<String, serde_json::Value>,
    /// The `links` string value from the package's manifest, or null if not
    /// specified. This field is optional and defaults to null.
    pub links: Option<String>,
    /// The minimal supported Rust version (optional)
    /// This must be a valid version requirement without an operator (e.g. no `=`)
    pub rust_version: Option<String>,
}

impl CrateMetadata {
    /// Validate the crate's metadata
    pub fn validate(&self) -> Result<CrateUploadResult, ApiError> {
        self.validate_name()?;
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
}

/// Creates a validation error
pub fn validation_error(details: &str) -> Result<(), ApiError> {
    Err(specialize(error_invalid_request(), details.to_string()))
}

/// The kind of dependency
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DependencyKind {
    /// A normal dependency
    #[default]
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

/// A dependency for a crate
#[derive(Default, Debug, Clone, Serialize, Deserialize)]
pub struct CrateMetadataDependency {
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
    pub kind: DependencyKind,
    /// The URL of the index of the registry where this dependency is
    /// from as a string. If not specified or null, it is assumed the
    /// dependency is in the current registry.
    pub registry: Option<String>,
    /// If the dependency is renamed, this is a string of the new
    /// package name. If not specified or null, this dependency is not
    /// renamed.
    pub explicit_name_in_toml: Option<String>,
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
    pub fn build_index_data(&self) -> IndexCrateMetadata {
        let cksum = sha256(&self.content);
        IndexCrateMetadata {
            name: self.metadata.name.clone(),
            vers: self.metadata.vers.clone(),
            deps: self.metadata.deps.iter().map(IndexCrateDependency::from).collect(),
            cksum,
            features: HashMap::new(),
            yanked: false,
            links: self.metadata.links.clone(),
            v: Some(2),
            features2: Some(self.metadata.features.clone()),
            rust_version: self.metadata.rust_version.clone(),
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

/// The metadata for a crate inside the index
#[derive(Default, Debug, Clone, Serialize, Deserialize)]
pub struct IndexCrateMetadata {
    /// The name of the package
    pub name: String,
    /// The version of the package this row is describing.
    /// This must be a valid version number according to the Semantic
    /// Versioning 2.0.0 spec at [https://semver.org/](https://semver.org/).
    pub vers: String,
    /// Array of direct dependencies of the package
    pub deps: Vec<IndexCrateDependency>,
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
    /// An unsigned 32-bit integer value indicating the schema version of this
    /// entry.
    ///
    /// If this not specified, it should be interpreted as the default of 1.
    ///
    /// Cargo (starting with version 1.51) will ignore versions it does not
    /// recognize. This provides a method to safely introduce changes to index
    /// entries and allow older versions of cargo to ignore newer entries it
    /// doesn't understand. Versions older than 1.51 ignore this field, and
    /// thus may misinterpret the meaning of the index entry.
    ///
    /// The current values are:
    ///
    /// * 1: The schema as documented here, not including newer additions.
    ///      This is honored in Rust version 1.51 and newer.
    /// * 2: The addition of the `features2` field.
    ///      This is honored in Rust version 1.60 and newer.
    pub v: Option<u32>,
    /// This optional field contains features with new, extended syntax.
    /// Specifically, namespaced features (`dep:`) and weak dependencies
    /// (`pkg?/feat`).
    ///
    /// This is separated from `features` because versions older than 1.19
    /// will fail to load due to not being able to parse the new syntax, even
    /// with a `Cargo.lock` file.
    ///
    /// Cargo will merge any values listed here with the "features" field.
    ///
    /// If this field is included, the "v" field should be set to at least 2.
    ///
    /// Registries are not required to use this field for extended feature
    /// syntax, they are allowed to include those in the "features" field.
    /// Using this is only necessary if the registry wants to support cargo
    /// versions older than 1.19, which in practice is only crates.io since
    /// those older versions do not support other registries.
    pub features2: Option<HashMap<String, Vec<String>>>,
    /// The minimal supported Rust version (optional)
    /// This must be a valid version requirement without an operator (e.g. no `=`)
    pub rust_version: Option<String>,
}

impl IndexCrateMetadata {
    /// Gets the value associated to a requested feature
    pub fn get_feature(&self, feature: &str) -> Option<&[String]> {
        self.features2
            .as_ref()
            .and_then(|r| r.get(feature))
            .or_else(|| self.features.get(feature))
            .map(Vec::as_slice)
    }
}

/// A dependency for a crate in the index
#[derive(Default, Debug, Clone, Serialize, Deserialize)]
pub struct IndexCrateDependency {
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
    pub kind: DependencyKind,
    /// The URL of the index of the registry where this dependency is
    /// from as a string. If not specified or null, it is assumed the
    /// dependency is in the current registry.
    pub registry: Option<String>,
    /// If the dependency is renamed, this is a string of the new
    /// package name. If not specified or null, this dependency is not
    /// renamed.
    pub package: Option<String>,
}

impl IndexCrateDependency {
    /// Gets the crate name for this dependency
    pub fn get_name(&self) -> &str {
        self.package.as_deref().unwrap_or(&self.name)
    }

    /// Gets whether this dependency is active, for the specified targets and features
    pub fn is_active_for(&self, active_targets: &[String], active_features: &[&str]) -> bool {
        let is_in_targets = match self.target.as_ref() {
            None => true,
            Some(target_spec) => {
                if let Some(rest) = target_spec.strip_prefix("cfg(") {
                    let _cfg_spec = &rest[..rest.len() - 1];
                    // FIXME
                    false
                } else {
                    active_targets.contains(target_spec)
                }
            }
        };
        if !is_in_targets {
            // not for an active target
            return false;
        }
        if !self.optional {
            // not optional
            return true;
        }
        let name = self.get_name();
        active_features.iter().any(|feature| {
            feature.strip_prefix("dep:").is_some_and(|suffix| name == suffix)
                || feature.find('/').is_some_and(|i| &feature[..i] == name)
        })
    }
}

impl From<&CrateMetadataDependency> for IndexCrateDependency {
    fn from(dep: &CrateMetadataDependency) -> Self {
        Self {
            name: dep.explicit_name_in_toml.as_ref().unwrap_or(&dep.name).clone(),
            req: dep.version_req.clone(),
            features: dep.features.clone(),
            optional: dep.optional,
            default_features: dep.default_features,
            target: dep.target.clone(),
            kind: dep.kind,
            registry: dep.registry.clone(),
            package: if dep.explicit_name_in_toml.is_some() {
                Some(dep.name.clone())
            } else {
                None
            },
        }
    }
}
