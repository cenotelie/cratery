/*******************************************************************************
 * Copyright (c) 2024 Cénotélie Opérations SAS (cenotelie.fr)
 ******************************************************************************/

//! Storage backend that use the file system

use std::path::{Path, PathBuf};

use super::Storage;
use crate::model::cargo::CrateMetadata;
use crate::utils::apierror::{error_not_found, ApiError};

/// An storage implementation that uses the file system
pub struct FsStorage<'config> {
    /// The data directory in the configuration
    data_dir: &'config str,
}

impl<'config> FsStorage<'config> {
    pub fn new(data_dir: &'config str) -> FsStorage<'config> {
        Self { data_dir }
    }

    fn crate_file_key(name: &str, version: &str, filename: &str) -> String {
        format!("crates/{name}/{version}/{filename}")
    }

    fn data_path(name: &str, version: &str) -> String {
        Self::crate_file_key(name, version, "data")
    }

    fn metadata_path(name: &str, version: &str) -> String {
        Self::crate_file_key(name, version, "metadata")
    }

    fn readme_path(name: &str, version: &str) -> String {
        Self::crate_file_key(name, version, "readme")
    }

    /// Write to a file
    async fn write_to_file(&self, path: &str, content: &[u8]) -> Result<(), ApiError> {
        let full_path = PathBuf::from(format!("{}/{path}", self.data_dir));
        tokio::fs::create_dir_all(full_path.parent().unwrap()).await?;
        tokio::fs::write(full_path, content).await?;
        Ok(())
    }

    /// Reads from a file
    async fn read_from_file(&self, path: &str) -> Result<Vec<u8>, ApiError> {
        let full_path = PathBuf::from(format!("{}/{path}", self.data_dir));
        for component in full_path.components() {
            let Some(part) = component.as_os_str().to_str() else {
                return Err(error_not_found());
            };
            if part == ".." {
                // forbid parent folder
                return Err(error_not_found());
            }
        }

        Ok(tokio::fs::read(full_path).await?)
    }
}

impl<'config> Storage for FsStorage<'config> {
    /// Stores the data for a crate
    async fn store_crate(&self, metadata: &CrateMetadata, content: Vec<u8>) -> Result<(), ApiError> {
        let readme = super::extract_readme(&content)?;
        let metadata_json = serde_json::to_vec(metadata)?;
        let name = &metadata.name;
        let version = &metadata.vers;

        self.write_to_file(&Self::data_path(name, version), &content).await?;
        self.write_to_file(&Self::metadata_path(name, version), &metadata_json)
            .await?;
        self.write_to_file(&Self::readme_path(name, version), &readme).await?;
        Ok(())
    }

    /// Downloads a crate
    async fn download_crate(&self, name: &str, version: &str) -> Result<Vec<u8>, ApiError> {
        self.read_from_file(&Self::data_path(name, version)).await
    }

    /// Downloads the last metadata for a crate
    async fn download_crate_metadata(&self, name: &str, version: &str) -> Result<Option<CrateMetadata>, ApiError> {
        if let Ok(data) = self.read_from_file(&Self::metadata_path(name, version)).await {
            Ok(Some(serde_json::from_slice(&data)?))
        } else {
            Ok(None)
        }
    }

    /// Downloads the last README for a crate
    async fn download_crate_readme(&self, name: &str, version: &str) -> Result<Vec<u8>, ApiError> {
        self.read_from_file(&Self::readme_path(name, version)).await
    }

    /// Stores a documentation file
    async fn store_doc_file(&self, path: &str, file: &Path) -> Result<(), ApiError> {
        let full_path = PathBuf::from(format!("{}/docs/{path}", self.data_dir));
        tokio::fs::create_dir_all(full_path.parent().unwrap()).await?;
        tokio::fs::copy(file, &full_path).await?;
        Ok(())
    }

    /// Stores a documentation file
    async fn store_doc_data(&self, path: &str, content: Vec<u8>) -> Result<(), ApiError> {
        self.write_to_file(&format!("docs/{path}"), &content).await?;
        Ok(())
    }

    /// Gets the content of a documentation file
    async fn download_doc_file(&self, path: &str) -> Result<Vec<u8>, ApiError> {
        self.read_from_file(&format!("docs/{path}")).await
    }
}
