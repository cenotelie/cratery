/*******************************************************************************
 * Copyright (c) 2024 Cénotélie Opérations SAS (cenotelie.fr)
 ******************************************************************************/

//! Storage implementations for crates data and documentation

use std::io::Read;
use std::path::Path;

use flate2::bufread::GzDecoder;
use opendal::layers::LoggingLayer;
use opendal::Operator;
use tar::Archive;

use crate::model::cargo::CrateMetadata;
use crate::model::config::{Configuration, StorageConfig};
use crate::utils::apierror::ApiError;

/// Backing storage
pub struct Storage {
    opendal_operator: Operator,
}

impl From<&Configuration> for Storage {
    fn from(config: &Configuration) -> Self {
        let opendal_operator = match &config.storage {
            StorageConfig::FileSystem => {
                let builder = opendal::services::Fs::default().root(&config.data_dir);

                opendal::Operator::new(builder)
                    .unwrap()
                    .layer(LoggingLayer::default())
                    .finish()
            }
            StorageConfig::S3 { params, bucket } => {
                let builder = opendal::services::S3::default()
                    .bucket(bucket)
                    .region(&params.region)
                    .endpoint(&params.uri)
                    .access_key_id(&params.access_key)
                    .secret_access_key(&params.secret_key);

                opendal::Operator::new(builder)
                    .unwrap()
                    .layer(LoggingLayer::default())
                    .finish()
            }
        };

        Storage { opendal_operator }
    }
}

impl Storage {
    /// Stores the data for a crate
    pub async fn store_crate(&self, metadata: &CrateMetadata, content: Vec<u8>) -> Result<(), ApiError> {
        let readme = extract_readme(&content)?;
        let metadata_json = serde_json::to_vec(metadata)?;
        let name = &metadata.name;
        let version = &metadata.vers;

        self.write_to_file(&Self::data_path(name, version), content).await?;

        self.write_to_file(&Self::metadata_path(name, version), metadata_json).await?;

        self.write_to_file(&Self::readme_path(name, version), readme).await?;

        Ok(())
    }

    /// Downloads a crate
    pub async fn download_crate(&self, name: &str, version: &str) -> Result<Vec<u8>, ApiError> {
        self.read_from_file(&Self::data_path(name, version)).await
    }

    /// Downloads the last metadata for a crate
    pub async fn download_crate_metadata(&self, name: &str, version: &str) -> Result<Option<CrateMetadata>, ApiError> {
        if let Ok(data) = self.read_from_file(&Self::metadata_path(name, version)).await {
            Ok(Some(serde_json::from_slice(&data)?))
        } else {
            Ok(None)
        }
    }

    /// Downloads the last README for a crate
    pub async fn download_crate_readme(&self, name: &str, version: &str) -> Result<Vec<u8>, ApiError> {
        self.read_from_file(&Self::readme_path(name, version)).await
    }

    /// Stores a documentation file
    pub async fn store_doc_file(&self, path: &str, file: &Path) -> Result<(), ApiError> {
        let content = tokio::fs::read(file).await?;
        self.write_to_file(&format!("/docs/{path}"), content).await?;
        Ok(())
    }

    /// Stores a documentation file
    pub async fn store_doc_data(&self, path: &str, content: Vec<u8>) -> Result<(), ApiError> {
        self.write_to_file(&format!("docs/{path}"), content).await?;
        Ok(())
    }

    /// Gets the content of a documentation file
    pub async fn download_doc_file(&self, path: &str) -> Result<Vec<u8>, ApiError> {
        self.read_from_file(&format!("docs/{path}")).await
    }

    /// Write to a file
    pub async fn write_to_file(&self, path: &str, content: Vec<u8>) -> Result<(), ApiError> {
        self.opendal_operator.write(path, content).await?;
        Ok(())
    }

    /// Reads from a file
    async fn read_from_file(&self, path: &str) -> Result<Vec<u8>, ApiError> {
        let buffer = self.opendal_operator.read(path).await?;

        Ok(buffer.to_vec())
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
}

/// Extract the content of the README from the
pub fn extract_readme(crate_content: &[u8]) -> Result<Vec<u8>, ApiError> {
    let decoder = GzDecoder::new(crate_content);
    let mut archive = Archive::new(decoder);
    let mut buffer = Vec::new();

    archive
        .entries()?
        .find(|entry| {
            entry.as_ref().is_ok_and(|entry| {
                entry.header().path().is_ok_and(|path| {
                    path.file_name()
                        .is_some_and(|file_name| file_name.to_string_lossy().contains("README"))
                })
            })
        })
        .transpose()?
        .map(|mut entry| entry.read_to_end(&mut buffer))
        .transpose()?;

    Ok(buffer)
}
