//! Manage the storage of crates' data

use cenotelie_lib_apierror::ApiError;
use cenotelie_lib_s3 as s3;

use crate::objects::Configuration;

/// Stores the data for a crate
pub async fn store_crate(config: &Configuration, name: &str, version: &str, content: Vec<u8>) -> Result<(), ApiError> {
    let buckets = s3::list_all_buckets(&config.s3).await?;
    if buckets.into_iter().all(|b| b != config.bucket) {
        // bucket does not exist => create it
        s3::create_bucket(&config.s3, &config.bucket).await?;
    }
    let object_key = format!("{name}/{version}");
    s3::upload_object_raw(&config.s3, &config.bucket, &object_key, None, content).await?;
    Ok(())
}

/// Downloads a crate
pub async fn download_crate(config: &Configuration, name: &str, version: &str) -> Result<Vec<u8>, ApiError> {
    let object_key = format!("{name}/{version}");
    let data = s3::get_object(&config.s3, &config.bucket, &object_key).await?;
    Ok(data)
}
