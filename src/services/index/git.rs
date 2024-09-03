/*******************************************************************************
 * Copyright (c) 2024 Cénotélie Opérations SAS (cenotelie.fr)
 ******************************************************************************/

//! Implementation of an index using a local git repository

use std::path::{Path, PathBuf};

use log::info;
use tokio::fs::{create_dir_all, File, OpenOptions};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::Mutex;

use super::{build_package_file_path, Index};
use crate::model::cargo::IndexCrateMetadata;
use crate::model::config::IndexConfig;
use crate::utils::apierror::{error_backend_failure, error_not_found, specialize, ApiError};
use crate::utils::{execute_at_location, execute_git, FaillibleFuture};

/// Manages the index on git
pub struct GitIndex {
    inner: Mutex<GitIndexImpl>,
}

impl GitIndex {
    /// When the application is launched
    pub async fn new(config: IndexConfig) -> Result<Self, ApiError> {
        let inner = GitIndexImpl::new(config).await?;
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

    fn get_crate_data<'a>(&'a self, package: &'a str) -> FaillibleFuture<'a, Vec<IndexCrateMetadata>> {
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
    async fn new(config: IndexConfig) -> Result<Self, ApiError> {
        let index = Self { config };

        // check for the SSH key
        if let Some(file_name) = &index.config.remote_ssh_key_file_name {
            let mut key_filename = PathBuf::from(&index.config.home_dir);
            key_filename.push(".ssh");
            key_filename.push(file_name);
            if !key_filename.exists() {
                return Err(specialize(
                    error_backend_failure(),
                    format!("Missing key file: {key_filename:?}"),
                ));
            }
        }

        // check that the git folder exists
        let location = PathBuf::from(&index.config.location);
        if !location.exists() {
            tokio::fs::create_dir_all(&location).await?;
        }

        let mut content = tokio::fs::read_dir(&location).await?;
        if content.next_entry().await?.is_none() {
            // the folder is empty
            info!("index: initializing on empty index");
            index.initialize_index(location).await?;
        } else if index.config.remote_origin.is_some() {
            // attempt to pull changes
            info!("index: pulling changes from origin");
            execute_git(&location, &["pull", "origin", "master"]).await?;
        }
        Ok(index)
    }

    /// Initializes the index at the specified location
    async fn initialize_index(&self, location: PathBuf) -> Result<(), ApiError> {
        if let Some(remote_origin) = &self.config.remote_origin {
            // attempts to clone
            info!("index: cloning from {remote_origin}");
            if execute_git(&location, &["clone", remote_origin, "."]).await.is_ok() {
                return Ok(());
            }
            info!("index: clone failed!");
        }

        // initializes an empty index
        self.initialize_empty_index(location).await?;
        Ok(())
    }

    /// Initializes an empty index at the specified location
    async fn initialize_empty_index(&self, location: PathBuf) -> Result<(), ApiError> {
        // initialise an empty repo
        info!("index: initializing empty index");
        execute_git(&location, &["init"]).await?;
        execute_git(&location, &["config", "user.name", &self.config.user_name]).await?;
        execute_git(&location, &["config", "user.email", &self.config.user_email]).await?;
        if let Some(remote_origin) = &self.config.remote_origin {
            execute_git(&location, &["remote", "add", "origin", remote_origin]).await?;
        }
        // write the index configuration
        {
            let index_config = serde_json::to_vec(&self.config.public)?;
            let mut file_name = location.clone();
            file_name.push("config.json");
            let mut file = File::create(&file_name).await?;
            file.write_all(&index_config).await?;
            file.flush().await?;
            file.sync_all().await?;
        }
        // commit the configuration
        execute_git(&location, &["add", "."]).await?;
        execute_git(&location, &["commit", "-m", "Add initial configuration"]).await?;
        execute_git(&location, &["update-server-info"]).await?;
        if let (Some(remote_origin), true) = (self.config.remote_origin.as_ref(), self.config.remote_push_changes) {
            info!("index: pushing to {remote_origin}");
            execute_git(&location, &["push", "origin", "master"]).await?;
        }
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
        if full_path.exists() {
            Some(full_path)
        } else {
            None
        }
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
        execute_at_location(&location, "git-upload-pack", &["--stateless-rpc", ".git"], input).await
    }

    /// Publish a new version for a crate
    async fn publish_crate_version(&self, metadata: &IndexCrateMetadata) -> Result<(), ApiError> {
        let file_name = build_package_file_path(PathBuf::from(&self.config.location), &metadata.name);
        create_dir_all(file_name.parent().unwrap()).await?;
        let buffer = serde_json::to_vec(metadata)?;
        // write to package file
        {
            let mut file = OpenOptions::new().create(true).append(true).open(file_name).await?;
            file.write_all(&buffer).await?;
            file.write_all(&[0x0A]).await?; // add line end
            file.flush().await?;
            file.sync_all().await?;
        }
        // commit and update
        let location = PathBuf::from(&self.config.location);
        let message = format!("Publish {}:{}", &metadata.name, &metadata.vers);
        execute_git(&location, &["add", "."]).await?;
        execute_git(&location, &["commit", "-m", &message]).await?;
        execute_git(&location, &["update-server-info"]).await?;
        if let (Some(_), true) = (self.config.remote_origin.as_ref(), self.config.remote_push_changes) {
            execute_git(&location, &["push", "origin", "master"]).await?;
        }
        Ok(())
    }

    ///  Gets the data for a crate
    async fn get_crate_data(&self, package: &str) -> Result<Vec<IndexCrateMetadata>, ApiError> {
        let file_name = build_package_file_path(PathBuf::from(&self.config.location), package);
        if !file_name.exists() {
            return Err(specialize(
                error_not_found(),
                format!("package {package} is not in this registry"),
            ));
        }
        let file = File::open(&file_name).await?;
        let mut reader = BufReader::new(file).lines();
        let mut results = Vec::new();
        while let Some(line) = reader.next_line().await? {
            let data = serde_json::from_str(&line)?;
            results.push(data);
        }
        Ok(results)
    }
}
