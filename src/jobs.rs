//! API for maintenance jobs

use cenotelie_lib_apierror::ApiError;
use log::info;
use sqlx::SqliteConnection;

use crate::{api::Application, objects::Configuration, transaction::in_transaction};

/// Publish readme files on S3
#[allow(unused)]
pub async fn publish_readme_files(connection: &mut SqliteConnection, configuration: &Configuration) -> Result<(), ApiError> {
    let crates = in_transaction(connection, |transaction| async move {
        let app = Application::new(transaction);
        let result = app.search("", None).await?;
        Ok::<_, ApiError>(result.crates)
    })
    .await?;
    for crate_data in crates {
        let data = crate::storage::download_crate(configuration, &crate_data.name, &crate_data.max_version).await?;
        let readme = crate::storage::extract_readme(&data)?;
        crate::storage::store_crate_readme(configuration, &crate_data.name, readme).await?;
        info!("extracted README for {}", crate_data.name);
    }
    Ok(())
}
