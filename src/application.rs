/*******************************************************************************
 * Copyright (c) 2024 Cénotélie Opérations SAS (cenotelie.fr)
 ******************************************************************************/

//! Main application

use std::sync::Arc;

use futures::channel::mpsc::UnboundedSender;
use futures::lock::Mutex;
use futures::SinkExt;
use log::info;
use sqlx::sqlite::SqlitePoolOptions;
use sqlx::{Pool, Sqlite};
use tokio::task::JoinHandle;

use crate::model::config::Configuration;
use crate::model::objects::DocsGenerationJob;
use crate::services::database::Database;
use crate::services::index::Index;
use crate::utils::apierror::ApiError;

/// The state of this application for axum
pub struct Application {
    /// The configuration
    pub configuration: Arc<Configuration>,
    /// The database connection
    pub db_pool: Pool<Sqlite>,
    /// A mutex for synchronisation on git commands
    pub index: Mutex<Index>,
    /// Sender of documentation generation jobs
    pub docs_worker_sender: UnboundedSender<DocsGenerationJob>,
}

/// The empty database
const DB_EMPTY: &[u8] = include_bytes!("empty.db");

impl Application {
    /// Creates a new application
    pub async fn launch() -> Result<(Arc<Self>, JoinHandle<()>), ApiError> {
        // load configuration
        let configuration = Arc::new(Configuration::from_env()?);
        // write the auth data
        configuration.write_auth_config().await?;

        // connection pool to the database
        let db_filename = configuration.get_database_filename();
        if tokio::fs::metadata(&db_filename).await.is_err() {
            // write the file
            info!("db file is inaccessible => attempt to create an empty one");
            tokio::fs::write(&db_filename, DB_EMPTY).await?;
        }
        let db_pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_lazy(&configuration.get_database_url())?;
        // migrate the database, if appropriate
        crate::migrations::migrate_to_last(&mut *db_pool.acquire().await?).await?;

        // prepare the index
        let index = Index::on_launch(configuration.get_index_git_config()).await?;

        // docs worker
        let (docs_worker_sender, docs_worker) =
            crate::services::docs::create_docs_worker(configuration.clone(), db_pool.clone());
        // check undocumented packages
        {
            let mut docs_worker_sender = docs_worker_sender.clone();
            let mut connection = db_pool.acquire().await?;
            crate::utils::db::in_transaction(&mut connection, |transaction| async move {
                let app = Database::new(transaction);
                let jobs = app.get_undocumented_packages().await?;
                for job in jobs {
                    docs_worker_sender.send(job).await?;
                }
                Ok::<_, ApiError>(())
            })
            .await?;
        }

        Ok((
            Arc::new(Self {
                configuration,
                db_pool,
                index: Mutex::new(index),
                docs_worker_sender,
            }),
            docs_worker,
        ))
    }
}
