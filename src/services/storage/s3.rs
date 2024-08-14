/*******************************************************************************
 * Copyright (c) 2024 Cénotélie Opérations SAS (cenotelie.fr)
 ******************************************************************************/

//! Storage backend that use S3

use std::path::Path;

use super::Storage;
use crate::model::cargo::CrateMetadata;
use crate::utils::s3::S3Params;
use crate::utils::FaillibleFuture;

/// Storage implementation that uses S3
pub struct S3Storage {
    /// The S3 connection parameters
    params: S3Params,
    /// The bucket to use
    bucket: String,
}

impl S3Storage {
    /// Create a new storage implementation using S3 as a backing system
    pub fn new(params: S3Params, bucket: String) -> S3Storage {
        Self { params, bucket }
    }

    fn crate_file_key(name: &str, version: &str, filename: &str) -> String {
        format!("crates/{name}/{version}/{filename}")
    }

    fn data_key(name: &str, version: &str) -> String {
        Self::crate_file_key(name, version, "data")
    }

    fn metadata_key(name: &str, version: &str) -> String {
        Self::crate_file_key(name, version, "metadata")
    }

    fn readme_key(name: &str, version: &str) -> String {
        Self::crate_file_key(name, version, "readme")
    }
}

impl Storage for S3Storage {
    /// Stores the data for a crate
    fn store_crate<'a>(&'a self, metadata: &'a CrateMetadata, content: Vec<u8>) -> FaillibleFuture<'a, ()> {
        Box::pin(async move {
            let readme = super::extract_readme(&content)?;
            let buckets = crate::utils::s3::list_all_buckets(&self.params).await?;
            let name = &metadata.name;
            let version = &metadata.vers;

            if buckets.into_iter().all(|b| b != self.bucket) {
                // bucket does not exist => create it
                crate::utils::s3::create_bucket(&self.params, &self.bucket).await?;
            }
            crate::utils::s3::upload_object_raw(&self.params, &self.bucket, &Self::data_key(name, version), content).await?;
            // version data
            crate::utils::s3::upload_object_raw(
                &self.params,
                &self.bucket,
                &Self::metadata_key(name, version),
                serde_json::to_vec(metadata)?,
            )
            .await?;
            crate::utils::s3::upload_object_raw(&self.params, &self.bucket, &Self::readme_key(name, version), readme).await?;
            Ok(())
        })
    }

    /// Downloads a crate
    fn download_crate<'a>(&'a self, name: &'a str, version: &'a str) -> FaillibleFuture<'a, Vec<u8>> {
        Box::pin(async move {
            let object_key = Self::data_key(name, version);
            let data = crate::utils::s3::get_object(&self.params, &self.bucket, &object_key).await?;
            Ok(data)
        })
    }

    /// Downloads the last metadata for a crate
    fn download_crate_metadata<'a>(&'a self, name: &'a str, version: &'a str) -> FaillibleFuture<'a, Option<CrateMetadata>> {
        Box::pin(async move {
            let object_key = Self::metadata_key(name, version);
            if let Ok(data) = crate::utils::s3::get_object(&self.params, &self.bucket, &object_key).await {
                Ok(Some(serde_json::from_slice(&data)?))
            } else {
                Ok(None)
            }
        })
    }

    /// Downloads the last README for a crate
    fn download_crate_readme<'a>(&'a self, name: &'a str, version: &'a str) -> FaillibleFuture<'a, Vec<u8>> {
        Box::pin(async move {
            let object_key = Self::readme_key(name, version);
            let data = crate::utils::s3::get_object(&self.params, &self.bucket, &object_key).await?;
            Ok(data)
        })
    }

    /// Stores a documentation file
    fn store_doc_file<'a>(&'a self, path: &'a str, file: &'a Path) -> FaillibleFuture<'a, ()> {
        Box::pin(async move {
            let object_key = format!("docs/{path}");
            crate::utils::s3::upload_object_file(&self.params, &self.bucket, &object_key, file).await?;
            Ok(())
        })
    }

    /// Stores a documentation file
    fn store_doc_data<'a>(&'a self, path: &'a str, content: Vec<u8>) -> FaillibleFuture<'a, ()> {
        Box::pin(async move {
            let object_key = format!("docs/{path}");
            crate::utils::s3::upload_object_raw(&self.params, &self.bucket, &object_key, content).await?;
            Ok(())
        })
    }

    /// Gets the content of a documentation file
    fn download_doc_file<'a>(&'a self, path: &'a str) -> FaillibleFuture<'a, Vec<u8>> {
        Box::pin(async move {
            let object_key = format!("docs/{path}");
            let data = crate::utils::s3::get_object(&self.params, &self.bucket, &object_key).await?;
            Ok(data)
        })
    }
}
