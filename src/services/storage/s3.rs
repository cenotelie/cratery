/*******************************************************************************
 * Copyright (c) 2024 Cénotélie Opérations SAS (cenotelie.fr)
 ******************************************************************************/

//! Storage backend that use S3

use std::path::Path;

use super::Storage;
use crate::model::cargo::CrateMetadata;
use crate::utils::apierror::ApiError;
use crate::utils::s3::S3Params;

/// An storage implementation that uses S3
pub struct S3Storage<'config> {
    /// The S3 connection parameters
    params: &'config S3Params,
    /// The bucket to use
    bucket: &'config str,
}

impl<'config> S3Storage<'config> {
    pub fn new(params: &'config S3Params, bucket: &'config str) -> S3Storage<'config> {
        Self { params, bucket }
    }
}

impl<'config> Storage for S3Storage<'config> {
    /// Stores the data for a crate
    async fn store_crate(&self, metadata: &CrateMetadata, content: Vec<u8>) -> Result<(), ApiError> {
        let readme = super::extract_readme(&content)?;
        let buckets = crate::utils::s3::list_all_buckets(self.params).await?;
        if buckets.into_iter().all(|b| b != self.bucket) {
            // bucket does not exist => create it
            crate::utils::s3::create_bucket(self.params, self.bucket).await?;
        }
        crate::utils::s3::upload_object_raw(
            self.params,
            self.bucket,
            &format!("crates/{}/{}", metadata.name, metadata.vers),
            content,
        )
        .await?;
        // version data
        crate::utils::s3::upload_object_raw(
            self.params,
            self.bucket,
            &format!("crates/{}/{}/metadata", metadata.name, metadata.vers),
            serde_json::to_vec(metadata)?,
        )
        .await?;
        crate::utils::s3::upload_object_raw(
            self.params,
            self.bucket,
            &format!("crates/{}/{}/readme", metadata.name, metadata.vers),
            readme,
        )
        .await?;
        Ok(())
    }

    /// Stores the README for a crate
    async fn store_crate_readme(&self, name: &str, version: &str, content: Vec<u8>) -> Result<(), ApiError> {
        let object_key = format!("crates/{name}/{version}/readme");
        crate::utils::s3::upload_object_raw(self.params, self.bucket, &object_key, content).await?;
        Ok(())
    }

    /// Downloads a crate
    async fn download_crate(&self, name: &str, version: &str) -> Result<Vec<u8>, ApiError> {
        let object_key = format!("crates/{name}/{version}");
        let data = crate::utils::s3::get_object(self.params, self.bucket, &object_key).await?;
        Ok(data)
    }

    /// Downloads the last metadata for a crate
    async fn download_crate_metadata(&self, name: &str, version: &str) -> Result<Option<CrateMetadata>, ApiError> {
        let object_key = format!("crates/{name}/{version}/metadata");
        if let Ok(data) = crate::utils::s3::get_object(self.params, self.bucket, &object_key).await {
            Ok(Some(serde_json::from_slice(&data)?))
        } else {
            Ok(None)
        }
    }

    /// Downloads the last README for a crate
    async fn download_crate_readme(&self, name: &str, version: &str) -> Result<Vec<u8>, ApiError> {
        let object_key = format!("crates/{name}/{version}/readme");
        let data = crate::utils::s3::get_object(self.params, self.bucket, &object_key).await?;
        Ok(data)
    }

    /// Stores a documentation file
    async fn store_doc_file(&self, path: &str, file: &Path) -> Result<(), ApiError> {
        let object_key = format!("docs/{path}");
        crate::utils::s3::upload_object_file(self.params, self.bucket, &object_key, file).await?;
        Ok(())
    }

    /// Stores a documentation file
    async fn store_doc_data(&self, path: &str, content: Vec<u8>) -> Result<(), ApiError> {
        let object_key = format!("docs/{path}");
        crate::utils::s3::upload_object_raw(self.params, self.bucket, &object_key, content).await?;
        Ok(())
    }

    /// Gets the content of a documentation file
    async fn download_doc_file(&self, path: &str) -> Result<Vec<u8>, ApiError> {
        let object_key = format!("docs/{path}");
        let data = crate::utils::s3::get_object(self.params, self.bucket, &object_key).await?;
        Ok(data)
    }
}
