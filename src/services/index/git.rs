/*******************************************************************************
 * Copyright (c) 2024 Cénotélie Opérations SAS (cenotelie.fr)
 ******************************************************************************/

//! Implementation of an index using a local git repository

use std::path::{Path, PathBuf};

use futures::future::BoxFuture;
use log::{error, info};
use thiserror::Error;
use tokio::fs::{File, OpenOptions, create_dir_all};
use tokio::io::{self, AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::Mutex;

use super::{Index, IndexError, build_package_file_path};
use crate::model::cargo::IndexCrateMetadata;
use crate::model::config::IndexConfig;
use crate::utils::apierror::{ApiError, AsStatusCode};
use crate::utils::{CommandError, FaillibleFuture, execute_at_location, execute_git};

#[derive(Debug, Error)]
pub enum GitIndexError {
    #[error("missing key file: '{key_filename}'")]
    MissingKeyFile { key_filename: PathBuf },

    #[error("fail to create dir : '{location}'")]
    CreateDirAll {
        #[source]
        source: io::Error,
        location: PathBuf,
    },

    #[error("fail to read dir : '{location}'")]
    ReadDir {
        #[source]
        source: io::Error,
        location: PathBuf,
    },

    #[error("failed to open `{path}` for reading")]
    OpenReadFile {
        #[source]
        source: io::Error,
        path: PathBuf,
    },

    #[error("failed to open `{path}` for writing")]
    OpenWriteFile {
        #[source]
        source: io::Error,
        path: PathBuf,
    },

    #[error("error during read first entry in : '{location}'")]
    ReadNextEntry {
        #[source]
        source: io::Error,
        location: PathBuf,
    },

    #[error("failed init repository ad '{location}'")]
    CommandInit {
        #[source]
        src: CommandError,
        location: PathBuf,
    },

    #[error("failed to clone '{remote}'")]
    CommandClone {
        #[source]
        source: CommandError,
        remote: String,
    },

    #[error("failed to pull origin master")]
    CommandPull(#[source] CommandError),

    #[error("failed to configure user")]
    ConfigureUser(#[source] CommandError),

    #[error("failed to add remote origin '{remote}' user")]
    AddRemoteOrigin {
        #[source]
        src: CommandError,
        remote: String,
    },

    #[error("failed to add files to git repository index")]
    CommandAdd(#[source] CommandError),

    #[error("failed to commit indexed files in git ")]
    CommandCommit(#[source] CommandError),

    #[error("failed to update server info of git repository")]
    UpdateServerInfo(#[source] CommandError),

    #[error("failed to push origin master")]
    PushOriginMaster(#[source] CommandError),

    #[error("fail to serialize index public config")]
    PublicConfigSerialization(#[source] serde_json::Error),

    #[error("fail to write git index public config")]
    WritePublicConfig(#[source] io::Error),

    #[error("failed to read `{path}`")]
    ReadFile {
        #[source]
        source: io::Error,
        path: PathBuf,
    },

    #[error("failed to read line {line_idx} in file `{path}`")]
    ReadNextLine {
        #[source]
        source: io::Error,
        path: PathBuf,
        line_idx: usize,
    },

    #[error("failed to write into file `{path}`")]
    WriteAll {
        #[source]
        source: io::Error,
        path: PathBuf,
    },

    #[error("failed to flush file `{path}`")]
    Flush {
        #[source]
        source: io::Error,
        path: PathBuf,
    },

    #[error("failed to sync_all file `{path}`")]
    SyncAll {
        #[source]
        source: io::Error,
        path: PathBuf,
    },

    #[error("failed to deserialise line `{line_idx}` : \n{line}")]
    DeserialiseLine {
        #[source]
        source: serde_json::Error,
        line: String,
        line_idx: usize,
    },

    #[error("failed to serialise package version : `{version}`")]
    SerializeVersion {
        #[source]
        source: serde_json::Error,
        version: String,
    },
}
impl AsStatusCode for GitIndexError {}

/// Manages the index on git
pub struct GitIndex {
    inner: Mutex<GitIndexImpl>,
}

impl GitIndex {
    /// When the application is launched
    pub async fn new(config: IndexConfig, expect_empty: bool) -> Result<Self, GitIndexError> {
        let inner = GitIndexImpl::new(config, expect_empty).await?;
        Ok(Self {
            inner: Mutex::new(inner),
        })
    }
}

impl Index for GitIndex {
    fn get_index_file<'a>(&'a self, file_path: &'a Path) -> FaillibleFuture<'a, Option<PathBuf>> {
        Box::pin(async move { Ok(self.inner.lock().await.get_index_file(file_path)) })
    }

    fn get_upload_pack_info_refs(&self) -> FaillibleFuture<'_, Vec<u8>> {
        Box::pin(async move { self.inner.lock().await.get_upload_pack_info_refs().await })
    }

    fn get_upload_pack_for<'a>(&'a self, input: &'a [u8]) -> FaillibleFuture<'a, Vec<u8>> {
        Box::pin(async move { self.inner.lock().await.get_upload_pack_for(input).await })
    }

    fn publish_crate_version<'a>(&'a self, metadata: &'a IndexCrateMetadata) -> FaillibleFuture<'a, ()> {
        Box::pin(async move { self.inner.lock().await.publish_crate_version(metadata).await })
    }

    fn remove_crate_version<'a>(&'a self, package: &'a str, version: &'a str) -> BoxFuture<'a, Result<(), IndexError>> {
        Box::pin(async move { self.inner.lock().await.remove_crate_version(package, version).await })
    }

    fn get_crate_data<'a>(&'a self, package: &'a str) -> BoxFuture<'a, Result<Vec<IndexCrateMetadata>, IndexError>> {
        Box::pin(async move { self.inner.lock().await.get_crate_data(package).await })
    }
}

/// Manages the index on git
struct GitIndexImpl {
    /// The configuration
    config: IndexConfig,
}

impl GitIndexImpl {
    /// When the application is launched
    async fn new(config: IndexConfig, expect_empty: bool) -> Result<Self, GitIndexError> {
        let index = Self { config };

        // check for the SSH key
        if let Some(file_name) = &index.config.remote_ssh_key_file_name {
            let mut key_filename = PathBuf::from(&index.config.home_dir);
            key_filename.push(".ssh");
            key_filename.push(file_name);
            if !key_filename.exists() {
                return Err(GitIndexError::MissingKeyFile { key_filename });
            }
        }

        // check that the git folder exists
        let location = PathBuf::from(&index.config.location);
        if !location.exists() {
            tokio::fs::create_dir_all(&location)
                .await
                .map_err(|source| GitIndexError::CreateDirAll {
                    source,
                    location: location.clone(),
                })?;
        }

        let mut content = tokio::fs::read_dir(&location)
            .await
            .map_err(|source| GitIndexError::ReadDir {
                source,
                location: location.clone(),
            })?;
        if content
            .next_entry()
            .await
            .map_err(|source| GitIndexError::ReadNextEntry {
                source,
                location: location.clone(),
            })?
            .is_none()
        {
            // the folder is empty
            info!("index: initializing on empty index");
            index.initialize_on_empty(&location, expect_empty).await?;
        } else if index.config.remote_origin.is_some() {
            // attempt to pull changes
            info!("index: pulling changes from origin");
            execute_git(&location, &["pull", "origin", "master"])
                .await
                .map_err(GitIndexError::CommandPull)?;
        }
        index.configure_user(&location).await.map_err(GitIndexError::ConfigureUser)?;
        Ok(index)
    }

    /// Initializes the index at the specified location when found empty
    async fn initialize_on_empty(&self, location: &Path, expect_empty: bool) -> Result<(), GitIndexError> {
        if let Some(remote_origin) = &self.config.remote_origin {
            // attempts to clone
            info!("index: cloning from {remote_origin}");
            match execute_git(location, &["clone", remote_origin, "."]).await {
                Ok(()) => {
                    self.configure_user(location).await.map_err(GitIndexError::ConfigureUser)?;
                    // cloned and (re-)configured the git user
                    return Ok(());
                }
                Err(error) => {
                    // failed to clone
                    if expect_empty {
                        // this could be normal if we expected an empty index
                        // fallback to creating an empty index
                        info!("index: clone failed on empty database, this could be normal: {error}");
                    } else {
                        // we expected to successfully clone because the database is not empty
                        // so we have some packages in the database, but not in the index ... not good
                        error!("index: clone unexpectedly failed: {error}");
                        return Err(GitIndexError::CommandClone {
                            source: error,
                            remote: remote_origin.clone(),
                        });
                    }
                }
            }
        }

        // initializes an empty index
        self.initialize_new_index(location).await
    }

    /// Initializes an empty index at the specified location
    async fn initialize_new_index(&self, location: &Path) -> Result<(), GitIndexError> {
        // initialise an empty repo
        info!("index: initializing empty index");
        execute_git(location, &["init"])
            .await
            .map_err(|src| GitIndexError::CommandInit {
                src,
                location: location.to_path_buf(),
            })?;
        self.configure_user(location).await.map_err(GitIndexError::ConfigureUser)?;
        if let Some(remote_origin) = &self.config.remote_origin {
            execute_git(location, &["remote", "add", "origin", remote_origin])
                .await
                .map_err(|src| GitIndexError::AddRemoteOrigin {
                    src,
                    remote: remote_origin.clone(),
                })?;
        }
        // write the index configuration
        {
            let index_config = serde_json::to_vec(&self.config.public).map_err(GitIndexError::PublicConfigSerialization)?;
            let mut file_name = location.to_path_buf();
            file_name.push("config.json");
            write_file(&file_name, &index_config)
                .await
                .map_err(GitIndexError::WritePublicConfig)?;
        }
        // commit the configuration
        execute_git(location, &["add", "."])
            .await
            .map_err(GitIndexError::CommandAdd)?;
        execute_git(location, &["commit", "-m", "Add initial configuration"])
            .await
            .map_err(GitIndexError::CommandCommit)?;
        execute_git(location, &["update-server-info"])
            .await
            .map_err(GitIndexError::UpdateServerInfo)?;
        if let (Some(remote_origin), true) = (self.config.remote_origin.as_ref(), self.config.remote_push_changes) {
            info!("index: pushing to {remote_origin}");
            execute_git(location, &["push", "origin", "master"])
                .await
                .map_err(GitIndexError::PushOriginMaster)?;
        }
        Ok(())
    }

    /// Configures the git user
    async fn configure_user(&self, location: &Path) -> Result<(), CommandError> {
        execute_git(location, &["config", "user.name", &self.config.user_name]).await?;
        execute_git(location, &["config", "user.email", &self.config.user_email]).await?;
        Ok(())
    }

    /// Gets the full path to a file in the bare git repository
    fn get_index_file(&self, file_path: &Path) -> Option<PathBuf> {
        let mut full_path = PathBuf::from(&self.config.location);
        if file_path.iter().nth(1).is_some_and(|elem| elem == ".git") {
            // exclude .git folder
            return None;
        }
        for elem in file_path.iter().skip(1) {
            full_path.push(elem);
        }
        if full_path.exists() { Some(full_path) } else { None }
    }

    /// Gets the upload pack advertisement for /info/refs
    async fn get_upload_pack_info_refs(&self) -> Result<Vec<u8>, ApiError> {
        let location = PathBuf::from(&self.config.location);
        let mut data = execute_at_location(&location, "git-upload-pack", &["--http-backend-info-refs", ".git"], &[]).await?;
        let mut response = String::from("001e# service=git-upload-pack\n0000").into_bytes();
        response.append(&mut data);
        // response.append(&mut String::from("\n0000").into_bytes());
        Ok(response)
    }

    /// Gets the response for an upload pack request
    async fn get_upload_pack_for(&self, input: &[u8]) -> Result<Vec<u8>, ApiError> {
        let location = PathBuf::from(&self.config.location);
        execute_at_location(&location, "git-upload-pack", &["--stateless-rpc", ".git"], input)
            .await
            .map_err(ApiError::from)
    }

    /// Publish a new version for a crate
    async fn publish_crate_version(&self, metadata: &IndexCrateMetadata) -> Result<(), ApiError> {
        let file_name = build_package_file_path(PathBuf::from(&self.config.location), &metadata.name);
        create_dir_all(file_name.parent().unwrap()).await?;
        let buffer = serde_json::to_vec(metadata)?;
        // write to package file
        // append the metadata at the end
        let mut file = OpenOptions::new().create(true).append(true).open(file_name).await?;
        file.write_all(&buffer).await?;
        file.write_all(&[0x0A]).await?; // add line end
        file.flush().await?;
        file.sync_all().await?;
        // commit and update
        let message = format!("Publish {}:{}", &metadata.name, &metadata.vers);
        self.commit_changes(&message).await?;
        Ok(())
    }

    /// Completely removes a version from the registry
    async fn remove_crate_version(&self, package: &str, version: &str) -> Result<(), IndexError> {
        let file_name = build_package_file_path(PathBuf::from(&self.config.location), package);
        create_dir_all(file_name.parent().unwrap())
            .await
            .map_err(|source| GitIndexError::CreateDirAll {
                source,
                location: file_name.parent().unwrap().to_path_buf(),
            })?;
        // get the existing versions
        let mut versions = {
            // expect the file to be present
            let file = OpenOptions::new()
                .read(true)
                .open(&file_name)
                .await
                .map_err(|source| GitIndexError::OpenReadFile {
                    source,
                    path: file_name.clone(),
                })?;
            let reader = BufReader::new(file);
            let mut lines = reader.lines();
            let mut versions = Vec::new();
            let mut line_idx = 0;
            while let Some(line) = lines.next_line().await.map_err(|source| GitIndexError::ReadNextLine {
                source,
                path: file_name.clone(),
                line_idx,
            })? {
                let index_crate_metadata = serde_json::from_str::<IndexCrateMetadata>(&line)
                    .map_err(|source| GitIndexError::DeserialiseLine { source, line, line_idx })?;
                versions.push(index_crate_metadata);
                line_idx += 1;
            }
            versions
        };
        // remove the version of interest
        versions.retain(|v| v.vers != version);
        // write back
        {
            let mut file = OpenOptions::new()
                .write(true)
                .truncate(true)
                .open(&file_name)
                .await
                .map_err(|source| GitIndexError::OpenWriteFile {
                    source,
                    path: file_name.clone(),
                })?;
            for version in versions {
                let buffer = serde_json::to_vec(&version).map_err(|source| GitIndexError::SerializeVersion {
                    source,
                    version: format!("{version:#?}"),
                })?;
                file.write_all(&buffer).await.map_err(|source| GitIndexError::WriteAll {
                    source,
                    path: file_name.clone(),
                })?;
                file.write_all(&[0x0A]).await.map_err(|source| GitIndexError::WriteAll {
                    source,
                    path: file_name.clone(),
                })?; // add line end
            }
            file.flush().await.map_err(|source| GitIndexError::Flush {
                source,
                path: file_name.clone(),
            })?;
            file.sync_all().await.map_err(|source| GitIndexError::SyncAll {
                source,
                path: file_name.clone(),
            })?;
        }
        // commit and update
        let message = format!("Removed {package}:{version}");
        self.commit_changes(&message).await?;
        Ok(())
    }

    /// Commits the local changes to the index
    async fn commit_changes(&self, message: &str) -> Result<(), GitIndexError> {
        let location = PathBuf::from(&self.config.location);
        execute_git(&location, &["add", "."])
            .await
            .map_err(GitIndexError::CommandAdd)?;
        execute_git(&location, &["commit", "-m", message])
            .await
            .map_err(GitIndexError::CommandCommit)?;
        execute_git(&location, &["update-server-info"])
            .await
            .map_err(GitIndexError::UpdateServerInfo)?;
        if let (Some(_), true) = (self.config.remote_origin.as_ref(), self.config.remote_push_changes) {
            execute_git(&location, &["push", "origin", "master"])
                .await
                .map_err(GitIndexError::PushOriginMaster)?;
        }
        Ok(())
    }

    ///  Gets the data for a crate
    async fn get_crate_data(&self, package: &str) -> Result<Vec<IndexCrateMetadata>, IndexError> {
        let file_name = build_package_file_path(PathBuf::from(&self.config.location), package);
        if !file_name.exists() {
            return Err(IndexError::PackageNotInRegistry { package: package.into() });
        }
        let file = File::open(&file_name).await.map_err(|source| GitIndexError::OpenReadFile {
            source,
            path: file_name.clone(),
        })?;
        let mut reader = BufReader::new(file).lines();
        let mut results = Vec::new();
        let mut line_idx = 0;
        while let Some(line) = reader.next_line().await.map_err(|source| GitIndexError::ReadNextLine {
            source,
            path: file_name.clone(),
            line_idx,
        })? {
            let data =
                serde_json::from_str(&line).map_err(|source| GitIndexError::DeserialiseLine { source, line, line_idx })?;
            results.push(data);
            line_idx += 1;
        }
        Ok(results)
    }
}

async fn write_file(path: impl AsRef<Path>, index_config: &[u8]) -> Result<(), io::Error> {
    let mut file = File::create(path).await?;
    file.write_all(index_config).await?;
    file.flush().await?;
    file.sync_all().await
}
