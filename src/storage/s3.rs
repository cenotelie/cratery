/*******************************************************************************
 * Copyright (c) 2024 Cénotélie Opérations SAS (cenotelie.fr)
 ******************************************************************************/

//! Storage backend that use s3

use std::path::Path;

use cenotelie_lib_apierror::ApiError;
use cenotelie_lib_s3 as s3;

use crate::model::{
    config::{Configuration, StorageConfig},
    objects::CrateMetadata,
};

use super::Storage;

/// An storage implementation that uses s3
pub struct S3Storage<'config> {
    /// The S3 connection parameters
    params: &'config s3::S3Params,
    /// The bucket to use
    bucket: &'config str,
}

impl<'config> S3Storage<'config> {
    pub fn new(params: &'config s3::S3Params, bucket: &'config str) -> S3Storage<'config> {
        Self { params, bucket }
    }
}

impl<'config> Storage for S3Storage<'config> {
    /// Stores the data for a crate
    async fn store_crate(&self, metadata: &CrateMetadata, content: Vec<u8>) -> Result<(), ApiError> {
        let readme = super::extract_readme(&content)?;
        let buckets = s3::list_all_buckets(self.params).await?;
        if buckets.into_iter().all(|b| b != self.bucket) {
            // bucket does not exist => create it
            s3::create_bucket(self.params, self.bucket).await?;
        }
        s3::upload_object_raw(
            self.params,
            self.bucket,
            &format!("crates/{}/{}", metadata.name, metadata.vers),
            None,
            content,
        )
        .await?;
        // version data
        s3::upload_object_raw(
            self.params,
            self.bucket,
            &format!("crates/{}/{}/metadata", metadata.name, metadata.vers),
            None,
            serde_json::to_vec(metadata)?,
        )
        .await?;
        s3::upload_object_raw(
            self.params,
            self.bucket,
            &format!("crates/{}/{}/readme", metadata.name, metadata.vers),
            None,
            readme,
        )
        .await?;
        Ok(())
    }

    /// Stores the README for a crate
    async fn store_crate_readme(&self, name: &str, version: &str, content: Vec<u8>) -> Result<(), ApiError> {
        let object_key = format!("crates/{name}/{version}/readme");
        s3::upload_object_raw(self.params, self.bucket, &object_key, None, content).await?;
        Ok(())
    }

    /// Downloads a crate
    async fn download_crate(&self, name: &str, version: &str) -> Result<Vec<u8>, ApiError> {
        let object_key = format!("{name}/{version}");
        let data = s3::get_object(self.params, self.bucket, &object_key).await?;
        Ok(data)
    }

    /// Downloads the last metadata for a crate
    async fn download_crate_metadata(&self, name: &str, version: &str) -> Result<Option<CrateMetadata>, ApiError> {
        let object_key = format!("crates/{name}/{version}/metadata");
        if let Ok(data) = s3::get_object(self.params, self.bucket, &object_key).await {
            Ok(Some(serde_json::from_slice(&data)?))
        } else {
            Ok(None)
        }
    }

    /// Downloads the last README for a crate
    async fn download_crate_readme(&self, name: &str, version: &str) -> Result<Vec<u8>, ApiError> {
        let object_key = format!("crates/{name}/{version}/readme");
        let data = s3::get_object(self.params, self.bucket, &object_key).await?;
        Ok(data)
    }

    /// Stores a documentation file
    async fn store_doc_file(&self, path: &str, file: &Path) -> Result<(), ApiError> {
        let object_key = format!("docs/{path}");
        s3::upload_object_file(self.params, self.bucket, &object_key, None, file).await?;
        Ok(())
    }

    /// Gets the content of a documentation file
    async fn download_doc_file(&self, path: &str) -> Result<Vec<u8>, ApiError> {
        let object_key = format!("docs/{path}");
        let data = s3::get_object(self.params, self.bucket, &object_key).await?;
        Ok(data)
    }
}

/// Gets the keys for all the objects in the bucket
pub async fn get_objects_in_bucket(config: &Configuration, prefix: Option<&str>) -> Result<Vec<String>, ApiError> {
    let StorageConfig::S3 { params, bucket } = &config.storage else {
        return Ok(Vec::new());
    };
    let result = s3::list_objects(params, bucket, prefix, None).await?;
    Ok(result.into_iter().map(|o| o.key).collect())
}
