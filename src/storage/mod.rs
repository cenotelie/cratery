/*******************************************************************************
 * Copyright (c) 2024 Cénotélie Opérations SAS (cenotelie.fr)
 ******************************************************************************/

//! Storage implementations

pub mod fs;
pub mod s3;

use std::future::Future;
use std::io::Read;
use std::path::Path;
use std::time::Duration;

use flate2::bufread::GzDecoder;
use tar::Archive;

use crate::model::config::{Configuration, StorageConfig};
use crate::model::objects::CrateMetadata;
use crate::utils::apierror::{error_backend_failure, specialize, ApiError};

/// Backing storage implementations
pub trait Storage {
    /// Stores the data for a crate
    fn store_crate(&self, metadata: &CrateMetadata, content: Vec<u8>) -> impl Future<Output = Result<(), ApiError>> + Send;

    /// Stores the README for a crate
    fn store_crate_readme(
        &self,
        name: &str,
        version: &str,
        content: Vec<u8>,
    ) -> impl Future<Output = Result<(), ApiError>> + Send;

    /// Downloads a crate
    fn download_crate(&self, name: &str, version: &str) -> impl Future<Output = Result<Vec<u8>, ApiError>> + Send;

    /// Downloads the last metadata for a crate
    fn download_crate_metadata(
        &self,
        name: &str,
        version: &str,
    ) -> impl Future<Output = Result<Option<CrateMetadata>, ApiError>> + Send;

    /// Downloads the last README for a crate
    fn download_crate_readme(&self, name: &str, version: &str) -> impl Future<Output = Result<Vec<u8>, ApiError>> + Send;

    /// Stores a documentation file
    fn store_doc_file(&self, path: &str, file: &Path) -> impl Future<Output = Result<(), ApiError>> + Send;

    /// Stores a documentation file
    fn store_doc_data(&self, path: &str, content: Vec<u8>) -> impl Future<Output = Result<(), ApiError>> + Send;

    /// Gets the content of a documentation file
    fn download_doc_file(&self, path: &str) -> impl Future<Output = Result<Vec<u8>, ApiError>> + Send;
}

/// Gets the backing storage for the documentation
pub fn get_storage(config: &Configuration) -> impl Storage + '_ {
    StorageImpl { config }
}

/// The storage implementation
/// Use poor-man dispatch because we cannot use dyn for Storage
struct StorageImpl<'config> {
    /// The configuration
    config: &'config Configuration,
}

impl<'config> StorageImpl<'config> {
    /// Runs a future with a timeout
    async fn with_timeout<R, FUT>(&self, future: FUT) -> Result<R, ApiError>
    where
        FUT: Future<Output = Result<R, ApiError>>,
    {
        tokio::time::timeout(Duration::from_millis(self.config.storage_timeout), future)
            .await
            .map_err(|_| {
                specialize(
                    error_backend_failure(),
                    String::from("Timeout when interacting with the storage layer"),
                )
            })?
    }
}

impl<'config> Storage for StorageImpl<'config> {
    async fn store_crate(&self, metadata: &CrateMetadata, content: Vec<u8>) -> Result<(), ApiError> {
        match &self.config.storage {
            StorageConfig::FileSystem => {
                self.with_timeout(fs::FsStorage::new(&self.config.data_dir).store_crate(metadata, content))
                    .await
            }
            StorageConfig::S3 { params, bucket } => {
                self.with_timeout(s3::S3Storage::new(params, bucket).store_crate(metadata, content))
                    .await
            }
        }
    }

    async fn store_crate_readme(&self, name: &str, version: &str, content: Vec<u8>) -> Result<(), ApiError> {
        match &self.config.storage {
            StorageConfig::FileSystem => {
                self.with_timeout(fs::FsStorage::new(&self.config.data_dir).store_crate_readme(name, version, content))
                    .await
            }
            StorageConfig::S3 { params, bucket } => {
                self.with_timeout(s3::S3Storage::new(params, bucket).store_crate_readme(name, version, content))
                    .await
            }
        }
    }

    async fn download_crate(&self, name: &str, version: &str) -> Result<Vec<u8>, ApiError> {
        match &self.config.storage {
            StorageConfig::FileSystem => {
                self.with_timeout(fs::FsStorage::new(&self.config.data_dir).download_crate(name, version))
                    .await
            }
            StorageConfig::S3 { params, bucket } => {
                self.with_timeout(s3::S3Storage::new(params, bucket).download_crate(name, version))
                    .await
            }
        }
    }

    async fn download_crate_metadata(&self, name: &str, version: &str) -> Result<Option<CrateMetadata>, ApiError> {
        match &self.config.storage {
            StorageConfig::FileSystem => {
                self.with_timeout(fs::FsStorage::new(&self.config.data_dir).download_crate_metadata(name, version))
                    .await
            }
            StorageConfig::S3 { params, bucket } => {
                self.with_timeout(s3::S3Storage::new(params, bucket).download_crate_metadata(name, version))
                    .await
            }
        }
    }

    async fn download_crate_readme(&self, name: &str, version: &str) -> Result<Vec<u8>, ApiError> {
        match &self.config.storage {
            StorageConfig::FileSystem => {
                self.with_timeout(fs::FsStorage::new(&self.config.data_dir).download_crate_readme(name, version))
                    .await
            }
            StorageConfig::S3 { params, bucket } => {
                self.with_timeout(s3::S3Storage::new(params, bucket).download_crate_readme(name, version))
                    .await
            }
        }
    }

    /// Stores a documentation file
    async fn store_doc_file(&self, path: &str, file: &Path) -> Result<(), ApiError> {
        match &self.config.storage {
            StorageConfig::FileSystem => {
                self.with_timeout(fs::FsStorage::new(&self.config.data_dir).store_doc_file(path, file))
                    .await
            }
            StorageConfig::S3 { params, bucket } => {
                self.with_timeout(s3::S3Storage::new(params, bucket).store_doc_file(path, file))
                    .await
            }
        }
    }

    /// Stores a documentation file
    async fn store_doc_data(&self, path: &str, content: Vec<u8>) -> Result<(), ApiError> {
        match &self.config.storage {
            StorageConfig::FileSystem => {
                self.with_timeout(fs::FsStorage::new(&self.config.data_dir).store_doc_data(path, content))
                    .await
            }
            StorageConfig::S3 { params, bucket } => {
                self.with_timeout(s3::S3Storage::new(params, bucket).store_doc_data(path, content))
                    .await
            }
        }
    }

    /// Gets the content of a documentation file
    async fn download_doc_file(&self, path: &str) -> Result<Vec<u8>, ApiError> {
        match &self.config.storage {
            StorageConfig::FileSystem => {
                self.with_timeout(fs::FsStorage::new(&self.config.data_dir).download_doc_file(path))
                    .await
            }
            StorageConfig::S3 { params, bucket } => {
                self.with_timeout(s3::S3Storage::new(params, bucket).download_doc_file(path))
                    .await
            }
        }
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
