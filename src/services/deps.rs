/*******************************************************************************
 * Copyright (c) 2024 Cénotélie Opérations SAS (cenotelie.fr)
 ******************************************************************************/

//! Service to fetch data about dependency crates

use crate::model::deps::DependencyInfo;
use crate::services::index::Index;
use crate::utils::apierror::{error_not_found, ApiError};

/// Service to check the dependencies of a crate
pub struct DependencyChecker {}

impl DependencyChecker {
    /// Checks the dependencies of a local crate
    pub async fn check_crate(&self, index: &Index, package: &str, version: &str) -> Result<Vec<DependencyInfo>, ApiError> {
        let metadata = index.get_crate_data(package).await?;
        let metadata = metadata.iter().find(|meta| meta.vers == version).ok_or_else(error_not_found)?;


        Ok(Vec::new())
    }
}
