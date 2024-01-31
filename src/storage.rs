//! Manage the storage of crates' data

use std::io::Read;

use cenotelie_lib_apierror::ApiError;
use cenotelie_lib_s3 as s3;
use flate2::bufread::GzDecoder;
use tar::Archive;

use crate::objects::{Configuration, CrateMetadata};

/// Stores the data for a crate
pub async fn store_crate(config: &Configuration, metadata: &CrateMetadata, content: Vec<u8>) -> Result<(), ApiError> {
    let readme = extract_readme(&content)?;
    let buckets = s3::list_all_buckets(&config.s3).await?;
    if buckets.into_iter().all(|b| b != config.bucket) {
        // bucket does not exist => create it
        s3::create_bucket(&config.s3, &config.bucket).await?;
    }
    s3::upload_object_raw(
        &config.s3,
        &config.bucket,
        &format!("{}/{}", metadata.name, metadata.vers),
        None,
        content,
    )
    .await?;
    // version data
    s3::upload_object_raw(
        &config.s3,
        &config.bucket,
        &format!("{}/{}/metadata", metadata.name, metadata.vers),
        None,
        serde_json::to_vec(metadata)?,
    )
    .await?;
    s3::upload_object_raw(
        &config.s3,
        &config.bucket,
        &format!("{}/{}/readme", metadata.name, metadata.vers),
        None,
        readme,
    )
    .await?;
    Ok(())
}

/// Stores the README for a crate
pub async fn store_crate_readme(config: &Configuration, name: &str, version: &str, content: Vec<u8>) -> Result<(), ApiError> {
    let object_key = format!("{name}/{version}/readme");
    s3::upload_object_raw(&config.s3, &config.bucket, &object_key, None, content).await?;
    Ok(())
}

/// Downloads a crate
pub async fn download_crate(config: &Configuration, name: &str, version: &str) -> Result<Vec<u8>, ApiError> {
    let object_key = format!("{name}/{version}");
    let data = s3::get_object(&config.s3, &config.bucket, &object_key).await?;
    Ok(data)
}

/// Downloads the last metadata for a crate
pub async fn download_crate_metadata(
    config: &Configuration,
    name: &str,
    version: &str,
) -> Result<Option<CrateMetadata>, ApiError> {
    let object_key = format!("{name}/{version}/metadata");
    if let Ok(data) = s3::get_object(&config.s3, &config.bucket, &object_key).await {
        Ok(Some(serde_json::from_slice(&data)?))
    } else {
        Ok(None)
    }
}

/// Downloads the last README for a crate
pub async fn download_crate_readme(config: &Configuration, name: &str, version: &str) -> Result<Vec<u8>, ApiError> {
    let object_key = format!("{name}/{version}/readme");
    let data = s3::get_object(&config.s3, &config.bucket, &object_key).await?;
    Ok(data)
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

/// Gets the keys for all the objects in the bucket
pub async fn get_objects_in_bucket(config: &Configuration) -> Result<Vec<String>, ApiError> {
    let result = s3::list_objects(&config.s3, &config.bucket, None, None).await?;
    Ok(result.into_iter().map(|o| o.key).collect())
}
