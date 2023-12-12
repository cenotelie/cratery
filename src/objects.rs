//! Module for definition of API objects

use std::io::Cursor;
use std::{collections::HashMap, env::VarError};

use byteorder::{LittleEndian, ReadBytesExt};
use cenotelie_lib_apierror::{error_invalid_request, specialize, ApiError};
use cenotelie_lib_s3::S3Params;
use chrono::NaiveDateTime;
use data_encoding::HEXLOWER;
use ring::digest::{Context, SHA256};
use serde_derive::{Deserialize, Serialize};

/// The object representing the application version
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct AppVersion {
    /// The changeset that was used to build the app
    pub commit: String,
    /// The version tag, if any
    pub tag: String,
}

/// A configuration for the registry
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Configuration {
    /// Identifier for the license
    #[serde(rename = "licenseId")]
    pub license_id: String,
    /// Web domain for the license
    #[serde(rename = "licenseWebDomain")]
    pub licence_web_domain: String,
    /// The data directory
    #[serde(rename = "dataDir")]
    pub data_dir: String,
    /// The configuration for the index
    #[serde(rename = "indexConfig")]
    pub index_config: IndexConfig,
    /// The root uri to publish to
    pub uri: String,
    /// The parameters to connect to S3
    pub s3: S3Params,
    /// The name of the s3 bucket to use
    pub bucket: String,
    /// The uri of the OAuth login page
    #[serde(rename = "oauthLoginUri")]
    pub oauth_login_uri: String,
    /// The uri of the OAuth token API endpoint
    #[serde(rename = "oauthTokenUri")]
    pub oauth_token_uri: String,
    /// The uri of the OAuth userinfo API endpoint
    #[serde(rename = "oauthCallbackUri")]
    pub oauth_callback_uri: String,
    /// The uri of the OAuth userinfo API endpoint
    #[serde(rename = "oauthUserInfoUri")]
    pub oauth_userinfo_uri: String,
    /// The identifier of the client to use
    #[serde(rename = "oauthClientId")]
    pub oauth_client_id: String,
    /// The secret for the client to use
    #[serde(rename = "oauthClientSecret")]
    pub oauth_client_secret: String,
    /// The secret for the client to use
    #[serde(rename = "oauthClientScope")]
    pub oauth_client_scope: String,
}

impl Configuration {
    /// Gets the configuration from environment variables
    ///
    /// # Errors
    ///
    /// Return a `VarError` when an expected environment variable is not present
    pub fn from_env() -> Result<Self, VarError> {
        let data_dir = std::env::var("REGISTRY_DATA_DIR")?;
        let licence_web_domain = std::env::var("REGISTRY_LICENSE_WEB_DOMAIN")?;
        let uri = std::env::var("REGISTRY_PUBLIC_URI")?.replace("?domain", &licence_web_domain);
        let index_config = IndexConfig {
            location: format!("{}/index", data_dir),
            remote_origin: std::env::var("REGISTRY_GIT_REMOTE").ok(),
            remote_ssh_key_file_name: std::env::var("REGISTRY_GIT_REMOTE_SSH_KEY_FILENAME").ok(),
            user_name: std::env::var("REGISTRY_GIT_USER_NAME")?,
            user_email: std::env::var("REGISTRY_GIT_USER_EMAIL")?,
            public: IndexPublicConfig {
                dl: format!("{}/api/v1/crates", uri),
                api: uri.clone(),
            },
        };
        Ok(Self {
            license_id: std::env::var("REGISTRY_LICENSE_ID")?,
            licence_web_domain: licence_web_domain.clone(),
            data_dir,
            index_config,
            uri,
            s3: S3Params {
                uri: std::env::var("REGISTRY_S3_URI")?,
                region: std::env::var("REGISTRY_S3_REGION")?,
                service: std::env::var("REGISTRY_S3_SERVICE").ok(),
                access_key: std::env::var("REGISTRY_S3_ACCESS_KEY")?,
                secret_key: std::env::var("REGISTRY_S3_SECRET_KEY")?,
            },
            bucket: std::env::var("REGISTRY_S3_BUCKET")?,
            oauth_login_uri: std::env::var("REGISTRY_OAUTH_LOGIN_URI")?,
            oauth_token_uri: std::env::var("REGISTRY_OAUTH_TOKEN_URI")?,
            oauth_callback_uri: std::env::var("REGISTRY_OAUTH_CALLBACK_URI")?.replace("?domain", &licence_web_domain),
            oauth_userinfo_uri: std::env::var("REGISTRY_OAUTH_USERINFO_URI")?,
            oauth_client_id: std::env::var("REGISTRY_OAUTH_CLIENT_ID")?,
            oauth_client_secret: std::env::var("REGISTRY_OAUTH_CLIENT_SECRET")?,
            oauth_client_scope: std::env::var("REGISTRY_OAUTH_CLIENT_SCOPE")?,
        })
    }

    /// Gets the corresponding database url
    pub fn get_database_url(&self) -> String {
        format!("sqlite://{}/registry.db", self.data_dir)
    }

    /// Gets the corresponding index git config
    pub fn get_index_git_config(&self) -> IndexConfig {
        self.index_config.clone()
    }
}

/// An OAuth access token
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct OAuthToken {
    /// The access token
    pub access_token: String,
    /// The type of token
    pub token_type: String,
    /// The expiration time
    pub expires_in: Option<i64>,
    /// The refresh token
    pub refresh_token: Option<String>,
    /// The grant scope
    pub scope: Option<String>,
}

/// The configuration in the index
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct IndexConfig {
    /// The location in the file system
    pub location: String,
    /// URI for the origin git remote to sync with
    #[serde(rename = "remoteOrigin")]
    pub remote_origin: Option<String>,
    /// The name of the file for the SSH key for the remote
    #[serde(rename = "remoteSshKeyFileName")]
    pub remote_ssh_key_file_name: Option<String>,
    /// The user name to use for commits
    #[serde(rename = "userName")]
    pub user_name: String,
    /// The user email to use for commits
    #[serde(rename = "userEmail")]
    pub user_email: String,
    /// The public configuration
    pub public: IndexPublicConfig,
}

/// The configuration in the index
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct IndexPublicConfig {
    /// The root URI to download crates
    pub dl: String,
    /// The API root URI
    pub api: String,
}

/// A user for the registry
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct RegistryUser {
    /// The unique identifier
    pub id: i64,
    /// Whether this is an active user
    #[serde(rename = "isActive")]
    pub is_active: bool,
    /// The login
    pub login: String,
    /// The user's name
    pub name: String,
    /// The roles for the user
    pub roles: String,
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
