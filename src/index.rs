/*******************************************************************************
 * Copyright (c) 2024 Cénotélie Opérations SAS (cenotelie.fr)
 ******************************************************************************/

//! API for index manipulation

use std::path::{Path, PathBuf};
use std::process::Stdio;

use log::info;
use tokio::fs::{self, create_dir_all, File, OpenOptions};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;

use crate::model::config::IndexConfig;
use crate::model::objects::CrateMetadataIndex;
use crate::utils::apierror::{error_backend_failure, error_not_found, specialize, ApiError};

/// Manages the index on git
pub struct Index {
    /// The configuration
    config: IndexConfig,
}

impl Index {
    /// When the application is launched
    pub async fn on_launch(config: IndexConfig) -> Result<Index, ApiError> {
        let index = Index { config };

        // check for the SSH key
        if let Some(file_name) = &index.config.remote_ssh_key_file_name {
            let mut key_filename = PathBuf::from(std::env::var("HOME")?);
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
            fs::create_dir_all(&location).await?;
        }

        let mut content = fs::read_dir(&location).await?;
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

    /// Intializes the index at the specified location
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

    /// Intializes an empty index at the specified location
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
    pub fn get_index_file(&self, file_path: &Path) -> Option<PathBuf> {
        self.get_index_file_base(file_path)
            .or_else(|| self.get_index_file_git(file_path))
    }

    /// Gets the full path to a file in the checked out git repository
    fn get_index_file_base(&self, file_path: &Path) -> Option<PathBuf> {
        let mut full_path = PathBuf::from(&self.config.location);
        for elem in file_path.iter().skip(1) {
            full_path.push(elem);
        }
        if full_path.exists() {
            Some(full_path)
        } else {
            None
        }
    }

    /// Gets the full path to a file in the bare git repository
    fn get_index_file_git(&self, file_path: &Path) -> Option<PathBuf> {
        let mut full_path = PathBuf::from(&self.config.location);
        full_path.push(".git");
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
    pub async fn get_upload_pack_info_refs(&self) -> Result<Vec<u8>, ApiError> {
        let location = PathBuf::from(&self.config.location);
        let mut data = execute_at_location(&location, "git-upload-pack", &["--http-backend-info-refs", ".git"], &[]).await?;
        let mut response = String::from("001e# service=git-upload-pack\n0000").into_bytes();
        response.append(&mut data);
        // response.append(&mut String::from("\n0000").into_bytes());
        Ok(response)
    }

    /// Gets the response for a upload pack request
    pub async fn get_upload_pack_for(&self, input: &[u8]) -> Result<Vec<u8>, ApiError> {
        let location = PathBuf::from(&self.config.location);
        execute_at_location(&location, "git-upload-pack", &["--stateless-rpc", ".git"], input).await
    }

    /// Publish a new version for a crate
    pub async fn publish_crate_version(&self, metadata: &CrateMetadataIndex) -> Result<(), ApiError> {
        let file_name = self.file_for_package(&metadata.name);
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
    pub async fn get_crate_data(&self, package: &str) -> Result<Vec<CrateMetadataIndex>, ApiError> {
        let file_name = self.file_for_package(package);
        if !file_name.exists() {
            return Err(error_not_found());
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

    /// Produce the path that contains the metadata for the crate
    fn file_for_package(&self, name: &str) -> PathBuf {
        let lowercase = name.to_ascii_lowercase();
        let mut result = PathBuf::from(&self.config.location);
        match lowercase.len() {
            0 => panic!("Empty name is not possible"),
            1 | 2 => {
                result.push("1");
            }
            3 => {
                result.push("3");
                // safe because this should be ASCII
                result.push(&lowercase[0..1]);
            }
            _ => {
                // safe because this should be ASCII
                result.push(&lowercase[0..2]);
                result.push(&lowercase[2..4]);
            }
        }
        result.push(lowercase);
        result
    }
}

/// Execute a git command
async fn execute_git(location: &Path, args: &[&str]) -> Result<(), ApiError> {
    execute_at_location(location, "git", args, &[]).await.map(|_| ())
}

/// Execute a git command
async fn execute_at_location(location: &Path, command: &str, args: &[&str], input: &[u8]) -> Result<Vec<u8>, ApiError> {
    let mut child = Command::new(command)
        .current_dir(location)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()?;
    child.stdin.as_mut().unwrap().write_all(input).await?;
    let output = child.wait_with_output().await?;
    if output.status.success() {
        Ok(output.stdout)
    } else {
        Err(specialize(error_backend_failure(), String::from_utf8(output.stdout)?))
    }
}
