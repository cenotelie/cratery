/*******************************************************************************
 * Copyright (c) 2024 Cénotélie Opérations SAS (cenotelie.fr)
 ******************************************************************************/

//! Storage implementations for crates data and documentation

pub mod fs;
pub mod s3;

use std::future::Future;
use std::io::Read;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use flate2::bufread::GzDecoder;
use tar::Archive;

use crate::model::cargo::CrateMetadata;
use crate::model::config::{Configuration, StorageConfig};
use crate::utils::apierror::{error_backend_failure, specialize, ApiError};
use crate::utils::FaillibleFuture;

/// Backing storage implementations
pub trait Storage {
    /// Stores the data for a crate
    fn store_crate<'a>(&'a self, metadata: &'a CrateMetadata, content: Vec<u8>) -> FaillibleFuture<'a, ()>;

    /// Downloads a crate
    fn download_crate<'a>(&'a self, name: &'a str, version: &'a str) -> FaillibleFuture<'a, Vec<u8>>;

    /// Downloads the last metadata for a crate
    fn download_crate_metadata<'a>(&'a self, name: &'a str, version: &'a str) -> FaillibleFuture<'a, Option<CrateMetadata>>;

    /// Downloads the last README for a crate
    fn download_crate_readme<'a>(&'a self, name: &'a str, version: &'a str) -> FaillibleFuture<'a, Vec<u8>>;

    /// Stores a documentation file
    fn store_doc_file<'a>(&'a self, path: &'a str, file: &'a Path) -> FaillibleFuture<'a, ()>;

    /// Stores a documentation file
    fn store_doc_data<'a>(&'a self, path: &'a str, content: Vec<u8>) -> FaillibleFuture<'a, ()>;

    /// Gets the content of a documentation file
    fn download_doc_file<'a>(&'a self, path: &'a str) -> FaillibleFuture<'a, Vec<u8>>;
}

/// Gets the backing storage for the documentation
pub fn get_storage(config: &Configuration) -> Arc<dyn Storage + Send + Sync> {
    Arc::new(StorageWithTimeout {
        inner: match &config.storage {
            StorageConfig::FileSystem => Box::new(fs::FsStorage::new(config.data_dir.clone())),
            StorageConfig::S3 { params, bucket } => Box::new(s3::S3Storage::new(params.clone(), bucket.clone())),
        },
        timeout: config.storage_timeout,
    })
}

/// A wrapper storage that adds a timeout when interacting with the wrappee
struct StorageWithTimeout {
    /// The wrappee
    inner: Box<dyn Storage + Send + Sync>,
    /// Timeout (in milli-seconds) to use when interacting with the storage
    timeout: u64,
}

impl StorageWithTimeout {
    /// Runs a future with a timeout
    async fn with_timeout<R, FUT>(&self, future: FUT) -> Result<R, ApiError>
    where
        FUT: Future<Output = Result<R, ApiError>>,
    {
        tokio::time::timeout(Duration::from_millis(self.timeout), future)
            .await
            .map_err(|_| {
                specialize(
                    error_backend_failure(),
                    String::from("Timeout when interacting with the storage layer"),
                )
            })?
    }
}

impl Storage for StorageWithTimeout {
    fn store_crate<'a>(&'a self, metadata: &'a CrateMetadata, content: Vec<u8>) -> FaillibleFuture<'a, ()> {
        Box::pin(async move { self.with_timeout(self.inner.store_crate(metadata, content)).await })
    }

    fn download_crate<'a>(&'a self, name: &'a str, version: &'a str) -> FaillibleFuture<'a, Vec<u8>> {
        Box::pin(async move { self.with_timeout(self.inner.download_crate(name, version)).await })
    }

    fn download_crate_metadata<'a>(&'a self, name: &'a str, version: &'a str) -> FaillibleFuture<'a, Option<CrateMetadata>> {
        Box::pin(async move { self.with_timeout(self.inner.download_crate_metadata(name, version)).await })
    }

    fn download_crate_readme<'a>(&'a self, name: &'a str, version: &'a str) -> FaillibleFuture<'a, Vec<u8>> {
        Box::pin(async move { self.with_timeout(self.inner.download_crate_readme(name, version)).await })
    }

    fn store_doc_file<'a>(&'a self, path: &'a str, file: &'a Path) -> FaillibleFuture<'a, ()> {
        Box::pin(async move { self.with_timeout(self.inner.store_doc_file(path, file)).await })
    }

    fn store_doc_data<'a>(&'a self, path: &'a str, content: Vec<u8>) -> FaillibleFuture<'a, ()> {
        Box::pin(async move { self.with_timeout(self.inner.store_doc_data(path, content)).await })
    }

    fn download_doc_file<'a>(&'a self, path: &'a str) -> FaillibleFuture<'a, Vec<u8>> {
        Box::pin(async move { self.with_timeout(self.inner.download_doc_file(path)).await })
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
