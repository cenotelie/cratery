/*******************************************************************************
 * Copyright (c) 2024 Cénotélie Opérations SAS (cenotelie.fr)
 ******************************************************************************/

//! API for maintenance jobs

use cenotelie_lib_apierror::ApiError;
use log::info;
use sqlx::SqliteConnection;

use crate::app::Application;
use crate::index::Index;
use crate::model::config::Configuration;
use crate::transaction::in_transaction;

/// Publish readme files on S3
#[allow(unused)]
pub async fn publish_readme_files(
    connection: &mut SqliteConnection,
    configuration: &Configuration,
    index: &Index,
) -> Result<(), ApiError> {
    let crates = in_transaction(connection, |transaction| async move {
        let app = Application::new(transaction);
        let result = app.search("", None).await?;
        Ok::<_, ApiError>(result.crates)
    })
    .await?;
    let keys = crate::storage::get_objects_in_bucket(configuration, Some("crates")).await?;
    for crate_data in crates {
        let versions = index.get_crate_data(&crate_data.name).await?;
        for version in versions {
            let readme_key = format!("crates/{}/{}/readme", crate_data.name, version.vers);
            if keys.contains(&readme_key) {
                continue;
            }
            let data = crate::storage::download_crate(configuration, &crate_data.name, &version.vers).await?;
            let readme = crate::storage::extract_readme(&data)?;
            crate::storage::store_crate_readme(configuration, &crate_data.name, &version.vers, readme).await?;
            info!("extracted README for {}/{}", crate_data.name, version.vers);
        }
    }
    Ok(())
}
