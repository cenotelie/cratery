/*******************************************************************************
 * Copyright (c) 2024 Cénotélie Opérations SAS (cenotelie.fr)
 ******************************************************************************/

//! Storage backend that use the file system

use std::path::{Path, PathBuf};

use cenotelie_lib_apierror::{error_not_found, ApiError};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::Storage;
use crate::model::objects::CrateMetadata;

/// An storage implementation that uses the file system
pub struct FsStorage<'config> {
    /// The data directory in the configuration
    data_dir: &'config str,
}

impl<'config> FsStorage<'config> {
    pub fn new(data_dir: &'config str) -> FsStorage<'config> {
        Self { data_dir }
    }

    /// Write to a file
    async fn write_to_file(&self, path: &str, content: &[u8]) -> Result<(), ApiError> {
        let full_path = PathBuf::from(format!("{}/{path}", self.data_dir));
        tokio::fs::create_dir_all(full_path.parent().unwrap()).await?;
        let file = tokio::fs::File::create(&full_path).await?;
        let mut writer = tokio::io::BufWriter::new(file);
        writer.write_all(content).await?;
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
        let file = tokio::fs::File::open(&full_path).await?;
        let mut reader = tokio::io::BufReader::new(file);
        let mut buffer = Vec::new();
        reader.read_to_end(&mut buffer).await?;
        Ok(buffer)
    }
}

impl<'config> Storage for FsStorage<'config> {
    /// Stores the data for a crate
    async fn store_crate(&self, metadata: &CrateMetadata, content: Vec<u8>) -> Result<(), ApiError> {
        let readme = super::extract_readme(&content)?;
        let metadata_json = serde_json::to_vec(metadata)?;
        self.write_to_file(&format!("crates/{}/{}", metadata.name, metadata.vers), &content)
            .await?;
        self.write_to_file(
            &format!("crates/{}/{}/metadata", metadata.name, metadata.vers),
            &metadata_json,
        )
        .await?;
        self.write_to_file(&format!("crates/{}/{}/readme", metadata.name, metadata.vers), &readme)
            .await?;
        Ok(())
    }

    /// Stores the README for a crate
    async fn store_crate_readme(&self, name: &str, version: &str, content: Vec<u8>) -> Result<(), ApiError> {
        self.write_to_file(&format!("crates/{name}/{version}/readme"), &content)
            .await?;
        Ok(())
    }

    /// Downloads a crate
    async fn download_crate(&self, name: &str, version: &str) -> Result<Vec<u8>, ApiError> {
        self.read_from_file(&format!("{name}/{version}")).await
    }

    /// Downloads the last metadata for a crate
    async fn download_crate_metadata(&self, name: &str, version: &str) -> Result<Option<CrateMetadata>, ApiError> {
        if let Ok(data) = self.read_from_file(&format!("crates/{name}/{version}/metadata")).await {
            Ok(Some(serde_json::from_slice(&data)?))
        } else {
            Ok(None)
        }
    }

    /// Downloads the last README for a crate
    async fn download_crate_readme(&self, name: &str, version: &str) -> Result<Vec<u8>, ApiError> {
        self.read_from_file(&format!("crates/{name}/{version}/readme")).await
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
