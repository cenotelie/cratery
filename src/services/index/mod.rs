/*******************************************************************************
 * Copyright (c) 2024 Cénotélie Opérations SAS (cenotelie.fr)
 ******************************************************************************/

//! API for index manipulation

mod git;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::model::cargo::IndexCrateMetadata;
use crate::model::config::Configuration;
use crate::utils::apierror::ApiError;
use crate::utils::FaillibleFuture;

/// Index implementations
pub trait Index {
    /// Gets the full path to a file in the index
    fn get_index_file<'a>(&'a self, file_path: &'a Path) -> FaillibleFuture<'a, Option<PathBuf>>;

    /// Gets the upload pack advertisement for /info/refs
    fn get_upload_pack_info_refs(&self) -> FaillibleFuture<'_, Vec<u8>>;

    /// Gets the response for a upload pack request
    fn get_upload_pack_for<'a>(&'a self, input: &'a [u8]) -> FaillibleFuture<'a, Vec<u8>>;

    /// Publish a new version for a crate
    fn publish_crate_version<'a>(&'a self, metadata: &'a IndexCrateMetadata, is_overwriting: bool) -> FaillibleFuture<'a, ()>;

    ///  Gets the data for a crate
    fn get_crate_data<'a>(&'a self, package: &'a str) -> FaillibleFuture<'a, Vec<IndexCrateMetadata>>;
}

/// Gets path elements for a package in the file system
#[must_use]
pub fn package_file_path(lowercase: &str) -> (&str, Option<&str>) {
    match lowercase.len() {
        0 => panic!("Empty name is not possible"),
        1 => ("1", None),
        2 => ("2", None),
        3 => ("3", Some(&lowercase[..1])),
        _ => (&lowercase[0..2], Some(&lowercase[2..4])),
    }
}

/// Produce the path elements that contains the metadata for the crate
#[must_use]
pub fn build_package_file_path(mut root: PathBuf, name: &str) -> PathBuf {
    let lowercase = name.to_ascii_lowercase();
    let (first, second) = package_file_path(&lowercase);

    root.push(first);
    if let Some(second) = second {
        root.push(second);
    }
    root.push(lowercase);

    root
}

/// Gets the index service
pub async fn get_service(config: &Configuration, expect_empty: bool) -> Result<Arc<dyn Index + Send + Sync>, ApiError> {
    let index = git::GitIndex::new(config.get_index_git_config(), expect_empty).await?;
    Ok(Arc::new(index))
}
