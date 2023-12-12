//! Docs generation and management

use std::sync::Arc;

use cenotelie_lib_apierror::ApiError;
use futures::{channel::mpsc::UnboundedSender, StreamExt};
use log::error;
use sqlx::{Pool, Sqlite};
use tokio::task::JoinHandle;

use crate::{
    api::Application,
    objects::{Configuration, DocsGenerationJob},
    storage,
    transaction::in_transaction,
};

/// Creates a worker for the generation of documentation
pub fn create_docs_worker(
    configuration: Arc<Configuration>,
    pool: Pool<Sqlite>,
) -> (UnboundedSender<DocsGenerationJob>, JoinHandle<()>) {
    let (sender, mut receiver) = futures::channel::mpsc::unbounded();
    let handle = tokio::spawn(async move {
        while let Some(job) = receiver.next().await {
            if let Err(e) = docs_worker_job(&configuration, &pool, job).await {
                error!("{e}");
            }
        }
    });
    (sender, handle)
}

/// Executes a documentation generation job
async fn docs_worker_job(configuration: &Configuration, pool: &Pool<Sqlite>, job: DocsGenerationJob) -> Result<(), ApiError> {
    // // get the content
    // let content = storage::download_crate(&configuration, &job.crate_name, &job.crate_version).await?;
    // // extract to a temp folder
    // extract_content(&job.crate_name, &job.crate_version, &content).await?;

    // // set the package as documented
    // let mut connection = pool.acquire().await?;
    // in_transaction(&mut connection, |transaction| async move {
    //     let app = Application::new(transaction);
    //     app.set_package_documented(&job.crate_name, &job.crate_version).await
    // })
    // .await?;
    Ok(())
}

/// Generates and upload the documentation for a crate
async fn extract_content(name: &str, version: &str, content: &[u8]) -> Result<(), ApiError> {
    Ok(())
}
