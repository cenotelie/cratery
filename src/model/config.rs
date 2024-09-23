/*******************************************************************************
 * Copyright (c) 2024 Cénotélie Opérations SAS (cenotelie.fr)
 ******************************************************************************/

//! Module for configuration management

use std::net::{IpAddr, Ipv4Addr};
use std::path::PathBuf;
use std::process::Stdio;
use std::str::FromStr;

use axum::http::Uri;
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use serde_derive::{Deserialize, Serialize};
use tokio::fs::File;
use tokio::io::{AsyncWriteExt, BufWriter};
use tokio::process::Command;

use crate::model::errors::MissingEnvVar;
use crate::utils::apierror::ApiError;
use crate::utils::token::generate_token;

/// Gets the value for an environment variable
pub fn get_var<T: AsRef<str>>(name: T) -> Result<String, MissingEnvVar> {
    let key = name.as_ref();
    std::env::var(key).map_err(|original| MissingEnvVar {
        original,
        var_name: key.to_string(),
    })
}

/// The protocol to use for an external registry
#[derive(Debug, Serialize, Deserialize, Copy, Clone, PartialEq, Eq)]
pub enum ExternalRegistryProtocol {
    /// The git protocol
    Git,
    /// The sparse protocol
    Sparse,
}

/// The configuration for an external registry
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ExternalRegistry {
    /// The name for the registry
    pub name: String,
    /// The URI to the registry's index
    pub index: String,
    /// The protocol to use
    pub protocol: ExternalRegistryProtocol,
    /// The root uri to docs for packages in this registry
    #[serde(rename = "docsRoot")]
    pub docs_root: String,
    /// The login to connect to the registry
    pub login: String,
    /// The token for authentication
    pub token: String,
}

impl ExternalRegistry {
    /// Loads the configuration for a registry from the environment
    fn from_env(reg_index: usize) -> Result<Option<ExternalRegistry>, MissingEnvVar> {
        if let Ok(name) = get_var(format!("REGISTRY_EXTERNAL_{reg_index}_NAME")) {
            let mut index = get_var(format!("REGISTRY_EXTERNAL_{reg_index}_INDEX"))?;
            let protocol = if let Some(rest) = index.strip_prefix("sparse+") {
                index = rest.to_string();
                ExternalRegistryProtocol::Sparse
            } else {
                ExternalRegistryProtocol::Git
            };
            let docs_root = get_var(format!("REGISTRY_EXTERNAL_{reg_index}_DOCS"))?;
            let login = get_var(format!("REGISTRY_EXTERNAL_{reg_index}_LOGIN"))?;
            let token = get_var(format!("REGISTRY_EXTERNAL_{reg_index}_TOKEN"))?;
            Ok(Some(ExternalRegistry {
                name,
                index,
                protocol,
                docs_root,
                login,
                token,
            }))
        } else {
            Ok(None)
        }
    }
}

/// The specification of the storage system to use
#[derive(Debug, Serialize, Deserialize, Clone)]
pub enum StorageConfig {
    /// The file system
    FileSystem,
    /// An S3 bucket
    S3 {
        /// The parameters to connect to S3
        params: S3Params,
        /// The name of the s3 bucket to use
        bucket: String,
    },
}

impl StorageConfig {
    /// Loads the configuration for a registry from the environment
    fn from_env() -> Result<StorageConfig, MissingEnvVar> {
        let storage_kind = get_var("REGISTRY_STORAGE")?;
        Ok(match storage_kind.as_str() {
            "s3" | "S3" => StorageConfig::S3 {
                params: S3Params {
                    endpoint: get_var("REGISTRY_S3_URI")?,
                    region: get_var("REGISTRY_S3_REGION")?,
                    access_key: get_var("REGISTRY_S3_ACCESS_KEY")?,
                    secret_key: get_var("REGISTRY_S3_SECRET_KEY")?,
                    root: get_var("REGISTRY_S3_ROOT").unwrap_or_default(),
                },
                bucket: get_var("REGISTRY_S3_BUCKET")?,
            },
            "" | "fs" | "FS" | "filesystem" | "FileSystem" => StorageConfig::FileSystem,
            _ => panic!("invalid REGISTRY_STORAGE"),
        })
    }
}

/// The S3 parameters
#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct S3Params {
    /// Endpoint base URI for the S3 service
    pub endpoint: String,
    /// The region to target
    pub region: String,
    /// The account access key
    #[serde(rename = "accessKey")]
    pub access_key: String,
    /// The account secret key
    #[serde(rename = "secretKey")]
    pub secret_key: String,
    /// The prefix to use for the keys
    pub root: String,
}

/// The configuration in the index
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct IndexConfig {
    /// The home directory where the .cargo, .git are expected to be located
    #[serde(rename = "homeDir")]
    pub home_dir: String,
    /// The location in the file system
    pub location: String,
    /// Whether to allow the git protocol for clients fetching the index
    #[serde(rename = "allowProtocolGit")]
    pub allow_protocol_git: bool,
    /// Whether to allow the sparse protocol for clients fetching the index
    #[serde(rename = "allowProtocolSparse")]
    pub allow_protocol_sparse: bool,
    /// URI for the origin git remote to sync with
    #[serde(rename = "remoteOrigin")]
    pub remote_origin: Option<String>,
    /// The name of the file for the SSH key for the remote
    #[serde(rename = "remoteSshKeyFileName")]
    pub remote_ssh_key_file_name: Option<String>,
    /// Do automatically push index changes to the remote
    #[serde(rename = "remotePushChanges")]
    pub remote_push_changes: bool,
    /// The user name to use for commits
    #[serde(rename = "userName")]
    pub user_name: String,
    /// The user email to use for commits
    #[serde(rename = "userEmail")]
    pub user_email: String,
    /// The public configuration
    pub public: IndexPublicConfig,
}

impl IndexConfig {
    /// Loads the configuration for a registry from the environment
    fn from_env(home_dir: &str, data_dir: &str, web_public_uri: &str) -> Result<IndexConfig, MissingEnvVar> {
        Ok(IndexConfig {
            home_dir: home_dir.to_string(),
            location: format!("{data_dir}/index"),
            allow_protocol_git: get_var("REGISTRY_INDEX_PROTOCOL_GIT").map(|v| v == "true").unwrap_or(false),
            allow_protocol_sparse: get_var("REGISTRY_INDEX_PROTOCOL_SPARSE").map(|v| v == "true").unwrap_or(true),
            remote_origin: get_var("REGISTRY_GIT_REMOTE").ok(),
            remote_ssh_key_file_name: get_var("REGISTRY_GIT_REMOTE_SSH_KEY_FILENAME").ok(),
            remote_push_changes: get_var("REGISTRY_GIT_REMOTE_PUSH_CHANGES")
                .is_ok_and(|value| value == "1" || value.eq_ignore_ascii_case("true")),
            user_name: get_var("REGISTRY_GIT_USER_NAME")?,
            user_email: get_var("REGISTRY_GIT_USER_EMAIL")?,
            public: IndexPublicConfig {
                dl: format!("{web_public_uri}/api/v1/crates"),
                api: web_public_uri.to_string(),
                auth_required: true,
            },
        })
    }
}

/// The configuration in the index
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct IndexPublicConfig {
    /// The root URI to download crates
    pub dl: String,
    /// The API root URI
    pub api: String,
    /// Whether authentication is always required
    #[serde(rename = "auth-required")]
    pub auth_required: bool,
}

/// The SMTP configuration to use to send emails
#[derive(Debug, Default, Serialize, Deserialize, Clone)]
pub struct SmtpConfig {
    /// The host for sending mails
    pub host: String,
    /// The port for sending mails
    pub port: u16,
    /// The login to connect to the SMTP host
    pub login: String,
    /// The password to connect to the SMTP host
    pub password: String,
}

impl SmtpConfig {
    /// Loads the configuration for a registry from the environment
    fn from_env() -> Result<Self, MissingEnvVar> {
        Ok(Self {
            host: get_var("REGISTRY_EMAIL_SMTP_HOST")?,
            port: get_var("REGISTRY_EMAIL_SMTP_PORT")
                .map(|s| s.parse().expect("invalid REGISTRY_EMAIL_SMTP_PORT"))
                .unwrap_or(465),
            login: get_var("REGISTRY_EMAIL_SMTP_LOGIN")?,
            password: get_var("REGISTRY_EMAIL_SMTP_PASSWORD")?,
        })
    }
}

#[derive(Debug, Default, Serialize, Deserialize, Clone)]
pub struct EmailConfig {
    /// The SMTP configuration to use to send emails
    pub smtp: SmtpConfig,
    /// The address to use a sender for mails
    pub sender: String,
    /// The address to always CC for mails
    pub cc: String,
}

impl EmailConfig {
    /// Loads the configuration for a registry from the environment
    fn from_env() -> Result<Self, MissingEnvVar> {
        Ok(Self {
            smtp: SmtpConfig::from_env()?,
            sender: get_var("REGISTRY_EMAIL_SENDER")?,
            cc: get_var("REGISTRY_EMAIL_CC").unwrap_or_default(),
        })
    }
}

/// A configuration for the registry
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Configuration {
    /// The log level to use
    #[serde(rename = "logLevel")]
    pub log_level: String,
    /// The datetime format to use when logging
    #[serde(rename = "logDatetimeFormat")]
    pub log_datetime_format: String,
    /// The IP to bind for the web server
    #[serde(rename = "webListenOnIp")]
    pub web_listenon_ip: IpAddr,
    /// The port to bind for the web server
    #[serde(rename = "webListenOnPort")]
    pub web_listenon_port: u16,
    /// The root uri from which the application is served
    #[serde(rename = "webPublicUri")]
    pub web_public_uri: String,
    /// The domain for the application
    #[serde(rename = "webDomain")]
    pub web_domain: String,
    /// The maximum size for the body of incoming requests
    #[serde(rename = "webBodyLimit")]
    pub web_body_limit: usize,
    /// The path to the local resources to serve as the web app
    #[serde(rename = "webHotReloadPath")]
    pub web_hot_reload_path: Option<String>,
    /// The home directory where the .cargo, .git are expected to be located
    #[serde(rename = "homeDir")]
    pub home_dir: String,
    /// The data directory
    #[serde(rename = "dataDir")]
    pub data_dir: String,
    /// The configuration for the index
    #[serde(rename = "indexConfig")]
    pub index: IndexConfig,
    /// The configuration for the storage
    pub storage: StorageConfig,
    /// Timeout (in milli-seconds) to use when interacting with the storage
    #[serde(rename = "storageTimeout")]
    pub storage_timeout: u64,
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
    /// Path to the email field in the JSON blob returned at the userinfo URI
    #[serde(rename = "oauthUserInfoPathEmail")]
    pub oauth_userinfo_path_email: String,
    /// Path to the full name field in the JSON blob returned at the userinfo URI
    #[serde(rename = "oauthUserInfoPathFullName")]
    pub oauth_userinfo_path_fullname: String,
    /// The identifier of the client to use
    #[serde(rename = "oauthClientId")]
    pub oauth_client_id: String,
    /// The secret for the client to use
    #[serde(rename = "oauthClientSecret")]
    pub oauth_client_secret: String,
    /// The secret for the client to use
    #[serde(rename = "oauthClientScope")]
    pub oauth_client_scope: String,
    /// The known external registries that require authentication
    #[serde(rename = "externalRegistries")]
    pub external_registries: Vec<ExternalRegistry>,
    /// Flag to mock the documentation generation
    #[serde(rename = "docsGenMock")]
    pub docs_gen_mock: bool,
    /// Number of seconds between each check
    #[serde(rename = "depsCheckPeriod")]
    pub deps_check_period: u64,
    /// Number of milliseconds after which the local data about an external registry are deemed stale and must be pulled again
    #[serde(rename = "depsStaleRegistry")]
    pub deps_stale_registry: u64,
    /// Number of minutes after which the saved analysis for a crate becomes stale
    /// A negative number deactivates background analysis of crates
    #[serde(rename = "depsStaleAnalysis")]
    pub deps_stale_analysis: i64,
    /// Whether to send a notification by email to the owners of a crate when some of its dependencies become outdated
    #[serde(rename = "depsNotifyOutdated")]
    pub deps_notify_outdated: bool,
    /// Whether to send a notification by email to the owners of a crate when CVEs are discovered in its dependencies
    #[serde(rename = "depsNotifyCVEs")]
    pub deps_notify_cves: bool,
    /// The configuration for sending emails
    pub email: EmailConfig,
    /// The name to use for the local registry in cargo and git config
    #[serde(rename = "selfLocalName")]
    pub self_local_name: String,
    /// The login to the service account for self authentication
    #[serde(rename = "selfServiceLogin")]
    pub self_service_login: String,
    /// The token to the service account for self authentication
    #[serde(rename = "selfServiceToken")]
    pub self_service_token: String,
    /// The version of the locally installed toolchain
    #[serde(rename = "selfToolchainVersion")]
    pub self_toolchain_version: String,
    /// The host target of the locally installed toolchain
    #[serde(rename = "selfToolchainHost")]
    pub self_toolchain_host: String,
    /// The known built-in targets in rustc
    #[serde(rename = "selfBuiltinTargets")]
    pub self_builtin_targets: Vec<String>,
}

impl Default for Configuration {
    fn default() -> Self {
        Self {
            log_level: String::from("INFO"),
            log_datetime_format: String::from("[%Y-%m-%d %H:%M:%S]"),
            web_listenon_ip: IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0)),
            web_listenon_port: 80,
            web_public_uri: String::from("http://localhost"),
            web_domain: String::from("localhost"),
            web_body_limit: 10 * 1024 * 1024,
            web_hot_reload_path: None,
            home_dir: String::from("/home/cratery"),
            data_dir: String::from("/data"),
            index: IndexConfig {
                home_dir: String::from("/home/cratery"),
                location: String::from("/data/index"),
                allow_protocol_git: true,
                allow_protocol_sparse: true,
                remote_origin: None,
                remote_ssh_key_file_name: None,
                remote_push_changes: false,
                user_name: String::from("Cratery"),
                user_email: String::from("cratery@localhost"),
                public: IndexPublicConfig {
                    dl: String::from("http://localhost/api/v1/crates"),
                    api: String::from("http://localhost"),
                    auth_required: true,
                },
            },
            storage: StorageConfig::FileSystem,
            storage_timeout: 3000,
            oauth_login_uri: String::new(),
            oauth_token_uri: String::new(),
            oauth_callback_uri: String::new(),
            oauth_userinfo_uri: String::new(),
            oauth_userinfo_path_email: String::from("email"),
            oauth_userinfo_path_fullname: String::from("fullName"),
            oauth_client_id: String::new(),
            oauth_client_secret: String::new(),
            oauth_client_scope: String::new(),
            external_registries: Vec::new(),
            docs_gen_mock: true,
            deps_check_period: 60,
            deps_stale_registry: 60 * 1000,
            deps_stale_analysis: 24 * 60,
            deps_notify_outdated: false,
            deps_notify_cves: false,
            email: EmailConfig::default(),
            self_local_name: String::from("localhost"),
            self_service_login: String::new(),
            self_service_token: String::new(),
            self_toolchain_version: String::new(),
            self_toolchain_host: String::new(),
            self_builtin_targets: Vec::new(),
        }
    }
}

impl Configuration {
    /// Gets the configuration from environment variables
    ///
    /// # Errors
    ///
    /// Return a `VarError` when an expected environment variable is not present
    pub async fn from_env() -> Result<Self, MissingEnvVar> {
        let home_dir = get_var("REGISTRY_HOME_DIR")
            .or(get_var("HOME"))
            .unwrap_or_else(|_| String::from("/home/cratery"));
        let data_dir = get_var("REGISTRY_DATA_DIR")?;
        let web_public_uri = get_var("REGISTRY_WEB_PUBLIC_URI")?;
        let web_domain = Uri::from_str(&web_public_uri)
            .expect("invalid REGISTRY_WEB_PUBLIC_URI")
            .host()
            .unwrap_or_default()
            .to_string();
        let self_local_name = match get_var("REGISTRY_SELF_LOCAL_NAME") {
            Ok(value) => value,
            Err(_) => match web_domain.rfind('.') {
                Some(index) => web_domain[index..].to_string(),
                None => web_domain.clone(),
            },
        };
        let index = IndexConfig::from_env(&home_dir, &data_dir, &web_public_uri)?;
        let storage = StorageConfig::from_env()?;
        let deps_notify_outdated = get_var("REGISTRY_DEPS_NOTIFY_OUTDATED").map(|v| v == "true").unwrap_or(false);
        let deps_notify_cves = get_var("REGISTRY_DEPS_NOTIFY_CVES").map(|v| v == "true").unwrap_or(false);
        let email = if deps_notify_outdated || deps_notify_cves {
            EmailConfig::from_env()?
        } else {
            EmailConfig::default()
        };
        let mut external_registries = Vec::new();
        let mut external_registry_index = 1;
        while let Some(registry) = ExternalRegistry::from_env(external_registry_index)? {
            external_registries.push(registry);
            external_registry_index += 1;
        }
        Ok(Self {
            log_level: get_var("REGISTRY_LOG_LEVEL").unwrap_or_else(|_| String::from("INFO")),
            log_datetime_format: get_var("REGISTRY_LOG_DATE_TIME_FORMAT")
                .unwrap_or_else(|_| String::from("[%Y-%m-%d %H:%M:%S]")),
            web_listenon_ip: get_var("REGISTRY_WEB_LISTENON_IP").map_or_else(
                |_| IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0)),
                |s| IpAddr::from_str(&s).expect("invalid REGISTRY_WEB_LISTENON_IP"),
            ),
            web_listenon_port: get_var("REGISTRY_WEB_LISTENON_PORT")
                .map(|s| s.parse().expect("invalid REGISTRY_WEB_LISTENON_PORT"))
                .unwrap_or(80),
            web_domain,
            web_public_uri,
            web_body_limit: get_var("REGISTRY_WEB_BODY_LIMIT")
                .map(|s| s.parse().expect("invalid REGISTRY_WEB_BODY_LIMIT"))
                .unwrap_or(10 * 1024 * 1024),
            web_hot_reload_path: get_var("REGISTRY_WEB_HOT_RELOAD_PATH").ok(),
            home_dir,
            data_dir,
            index,
            storage,
            storage_timeout: get_var("REGISTRY_STORAGE_TIMEOUT")
                .map(|s| s.parse().expect("invalid REGISTRY_STORAGE_TIMEOUT"))
                .unwrap_or(3000),
            oauth_login_uri: get_var("REGISTRY_OAUTH_LOGIN_URI")?,
            oauth_token_uri: get_var("REGISTRY_OAUTH_TOKEN_URI")?,
            oauth_callback_uri: get_var("REGISTRY_OAUTH_CALLBACK_URI")?,
            oauth_userinfo_uri: get_var("REGISTRY_OAUTH_USERINFO_URI")?,
            oauth_userinfo_path_email: get_var("REGISTRY_OAUTH_USERINFO_PATH_EMAIL").unwrap_or_else(|_| String::from("email")),
            oauth_userinfo_path_fullname: get_var("REGISTRY_OAUTH_USERINFO_PATH_FULLNAME")
                .unwrap_or_else(|_| String::from("name")),
            oauth_client_id: get_var("REGISTRY_OAUTH_CLIENT_ID")?,
            oauth_client_secret: get_var("REGISTRY_OAUTH_CLIENT_SECRET")?,
            oauth_client_scope: get_var("REGISTRY_OAUTH_CLIENT_SCOPE")?,
            docs_gen_mock: get_var("REGISTRY_DOCS_GEN_MOCK").map(|v| v == "true").unwrap_or(false),
            deps_check_period: get_var("REGISTRY_DEPS_CHECK_PERIOD")
                .map(|s| s.parse().expect("invalid REGISTRY_DEPS_CHECK_PERIOD"))
                .unwrap_or(60), // 1 minute
            deps_stale_registry: get_var("REGISTRY_DEPS_STALE_REGISTRY")
                .map(|s| s.parse().expect("invalid REGISTRY_DEPS_STALE_REGISTRY"))
                .unwrap_or(60 * 1000), // 1 minute
            deps_stale_analysis: get_var("REGISTRY_DEPS_STALE_ANALYSIS")
                .map(|s| s.parse().expect("invalid REGISTRY_DEPS_STALE_ANALYSIS"))
                .unwrap_or(24 * 60), // 24 hours
            deps_notify_outdated,
            deps_notify_cves,
            email,
            self_local_name,
            self_service_login: generate_token(16),
            self_service_token: generate_token(64),
            self_toolchain_version: get_rustc_version().await,
            self_toolchain_host: get_rustc_host().await,
            self_builtin_targets: get_builtin_targets().await,
            external_registries,
        })
    }

    /// Gets the path to a file in the home folder
    #[must_use]
    pub fn get_home_path_for(&self, path: &[&str]) -> PathBuf {
        let mut result = PathBuf::from(&self.home_dir);
        for e in path {
            result.push(e);
        }
        result
    }

    /// Gets the name of the file for the database
    #[must_use]
    pub fn get_database_filename(&self) -> String {
        format!("{}/registry.db", self.data_dir)
    }

    /// Gets the corresponding database url
    #[must_use]
    pub fn get_database_url(&self) -> String {
        format!("sqlite://{}/registry.db", self.data_dir)
    }

    /// Gets the corresponding index git config
    #[must_use]
    pub fn get_index_git_config(&self) -> IndexConfig {
        self.index.clone()
    }

    /// Write the configuration for authenticating to registries
    ///
    /// # Errors
    ///
    /// Return an error when writing fail
    pub async fn write_auth_config(&self) -> Result<(), ApiError> {
        if self.index.allow_protocol_git {
            self.write_auth_config_git_config().await?;
            self.write_auth_config_git_credentials().await?;
        }
        self.write_auth_config_cargo_config().await?;
        self.write_auth_config_cargo_credentials().await?;
        Ok(())
    }

    /// Write the configuration for authenticating to registries
    async fn write_auth_config_git_config(&self) -> Result<(), ApiError> {
        let file = File::create(self.get_home_path_for(&[".gitconfig"])).await?;
        let mut writer = BufWriter::new(file);
        writer.write_all("[credential]\n    helper = store\n".as_bytes()).await?;
        writer.flush().await?;
        Ok(())
    }

    /// Write the configuration for authenticating to registries
    async fn write_auth_config_git_credentials(&self) -> Result<(), ApiError> {
        let file = File::create(self.get_home_path_for(&[".git-credentials"])).await?;
        let mut writer = BufWriter::new(file);
        let index = self.web_public_uri.find('/').unwrap() + 2;
        writer
            .write_all(
                format!(
                    "{}{}:{}@{}\n",
                    &self.web_public_uri[..index],
                    self.self_service_login,
                    self.self_service_token,
                    &self.web_public_uri[index..]
                )
                .as_bytes(),
            )
            .await?;
        for registry in &self.external_registries {
            let index = registry.index.find('/').unwrap() + 2;
            writer
                .write_all(
                    format!(
                        "{}{}:{}@{}",
                        &registry.index[..index],
                        registry.login,
                        registry.token,
                        &registry.index[index..]
                    )
                    .as_bytes(),
                )
                .await?;
        }
        writer.flush().await?;
        Ok(())
    }

    /// Write the configuration for authenticating to registries
    async fn write_auth_config_cargo_config(&self) -> Result<(), ApiError> {
        let file = File::create(self.get_home_path_for(&[".cargo", "config.toml"])).await?;
        let mut writer = BufWriter::new(file);
        writer.write_all("[registry]\n".as_bytes()).await?;
        writer
            .write_all("global-credential-providers = [\"cargo:token\"]\n".as_bytes())
            .await?;
        writer.write_all("\n".as_bytes()).await?;
        writer.write_all("[registries]\n".as_bytes()).await?;
        if self.index.allow_protocol_git {
            writer
                .write_all(format!("{} = {{ index = \"{}\" }}\n", self.self_local_name, self.web_public_uri).as_bytes())
                .await?;
            if self.index.allow_protocol_sparse {
                // both git and sparse
                writer
                    .write_all(
                        format!(
                            "{}sparse = {{ index = \"sparse+{}/\" }}\n",
                            self.self_local_name, self.web_public_uri
                        )
                        .as_bytes(),
                    )
                    .await?;
            }
        } else if self.index.allow_protocol_sparse {
            // sparse only
            writer
                .write_all(
                    format!(
                        "{} = {{ index = \"sparse+{}/\" }}\n",
                        self.self_local_name, self.web_public_uri
                    )
                    .as_bytes(),
                )
                .await?;
        }
        for registry in &self.external_registries {
            writer
                .write_all(format!("{} = {{ index = \"{}\" }}\n", registry.name, registry.index).as_bytes())
                .await?;
        }
        writer.flush().await?;
        Ok(())
    }

    /// Write the configuration for authenticating to registries
    async fn write_auth_config_cargo_credentials(&self) -> Result<(), ApiError> {
        let file = File::create(self.get_home_path_for(&[".cargo", "credentials.toml"])).await?;
        let mut writer = BufWriter::new(file);
        writer
            .write_all(format!("[registries.{}]\n", self.self_local_name).as_bytes())
            .await?;
        writer
            .write_all(
                format!(
                    "token = \"Basic {}\"\n",
                    STANDARD.encode(format!("{}:{}", self.self_service_login, self.self_service_token))
                )
                .as_bytes(),
            )
            .await?;
        if self.index.allow_protocol_git && self.index.allow_protocol_sparse {
            // add credential for specialized sparse registry
            writer
                .write_all(format!("[registries.{}sparse]\n", self.self_local_name).as_bytes())
                .await?;
            writer
                .write_all(
                    format!(
                        "token = \"Basic {}\"\n",
                        STANDARD.encode(format!("{}:{}", self.self_service_login, self.self_service_token))
                    )
                    .as_bytes(),
                )
                .await?;
        }
        for registry in &self.external_registries {
            writer
                .write_all(format!("[registries.{}]\n", registry.name).as_bytes())
                .await?;
            writer
                .write_all(
                    format!(
                        "token = \"Basic {}\"\n",
                        STANDARD.encode(format!("{}:{}", registry.login, registry.token))
                    )
                    .as_bytes(),
                )
                .await?;
        }
        writer.flush().await?;
        Ok(())
    }
}

/// Gets the rustc version
async fn get_rustc_version() -> String {
    let child = Command::new("rustc")
        .args(["+stable", "--version"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let output = child.wait_with_output().await.unwrap();
    let output = String::from_utf8(output.stdout).unwrap();
    output.split_ascii_whitespace().nth(1).unwrap().to_string()
}

async fn get_rustc_host() -> String {
    let child = Command::new("rustc")
        .args(["+stable", "-vV"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let output = child.wait_with_output().await.unwrap();
    let output = String::from_utf8(output.stdout).unwrap();
    output
        .lines()
        .find_map(|line| line.strip_prefix("host: ").map(str::to_string))
        .unwrap()
}

async fn get_builtin_targets() -> Vec<String> {
    let child = Command::new("rustc")
        .args(["+stable", "--print", "target-list"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let output = child.wait_with_output().await.unwrap();
    let output = String::from_utf8(output.stdout).unwrap();
    output.lines().map(str::to_string).collect()
}
