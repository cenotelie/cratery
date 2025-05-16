/*******************************************************************************
 * Copyright (c) 2024 Cénotélie Opérations SAS (cenotelie.fr)
 ******************************************************************************/

//! Service implementations

use std::{io, path::PathBuf, sync::Arc};

use thiserror::Error;

use crate::model::config::{Configuration, WriteAuthConfigError};
use crate::model::errors::MissingEnvVar;
use crate::model::worker::WorkersManager;
use crate::utils::apierror::{ApiError, AsStatusCode};
use crate::utils::db::RwSqlitePool;

pub mod database;
pub mod deps;
pub mod docs;
pub mod emails;
pub mod index;
pub mod rustsec;
pub mod storage;

#[derive(Debug, Error)]
pub enum ConfigurationError {
    #[error("failed to get configuration")]
    GetConfiguration(#[source] MissingEnvVar),

    #[error("failed to write configuration")]
    WriteConfiguration(#[source] WriteAuthConfigError),

    #[error("failed to create temp dir '{path}'")]
    CreateTempDir {
        #[source]
        source: io::Error,
        path: PathBuf,
    },
}
impl AsStatusCode for ConfigurationError {}

/// Factory responsible for building services
#[expect(async_fn_in_trait)]
pub trait ServiceProvider {
    /// Gets the configuration
    async fn get_configuration() -> Result<Configuration, ConfigurationError>;

    /// Gets the backing storage for the documentation
    fn get_storage(config: &Configuration) -> Arc<dyn storage::Storage + Send + Sync>;

    /// Gets the index service
    async fn get_index(config: &Configuration, expect_empty: bool) -> Result<Arc<dyn index::Index + Send + Sync>, ApiError>;

    /// Gets the rustsec service
    fn get_rustsec(config: &Configuration) -> Arc<dyn rustsec::RustSecChecker + Send + Sync>;

    /// Gets the dependencies checker service
    fn get_deps_checker(
        configuration: Arc<Configuration>,
        service_index: Arc<dyn index::Index + Send + Sync>,
        service_rustsec: Arc<dyn rustsec::RustSecChecker + Send + Sync>,
    ) -> Arc<dyn deps::DepsChecker + Send + Sync>;

    /// Gets the email sender service
    fn get_email_sender(config: Arc<Configuration>) -> Arc<dyn emails::EmailSender + Send + Sync>;

    /// Gets the documentation generation service
    fn get_docs_generator(
        configuration: Arc<Configuration>,
        service_db_pool: RwSqlitePool,
        service_storage: Arc<dyn storage::Storage + Send + Sync>,
        worker_nodes: WorkersManager,
    ) -> Arc<dyn docs::DocsGenerator + Send + Sync>;
}

/// Provides the standard implementations for services
pub struct StandardServiceProvider;

impl ServiceProvider for StandardServiceProvider {
    /// Gets the configuration
    async fn get_configuration() -> Result<Configuration, ConfigurationError> {
        let configuration = Configuration::from_env()
            .await
            .map_err(ConfigurationError::GetConfiguration)?;
        configuration
            .write_auth_config()
            .await
            .map_err(ConfigurationError::WriteConfiguration)?;
        Ok(configuration)
    }

    /// Gets the backing storage for the documentation
    fn get_storage(config: &Configuration) -> Arc<dyn storage::Storage + Send + Sync> {
        storage::get_service(config)
    }

    /// Gets the index service
    async fn get_index(config: &Configuration, expect_empty: bool) -> Result<Arc<dyn index::Index + Send + Sync>, ApiError> {
        index::get_service(config, expect_empty).await
    }

    /// Gets the rustsec service
    fn get_rustsec(config: &Configuration) -> Arc<dyn rustsec::RustSecChecker + Send + Sync> {
        rustsec::get_service(config)
    }

    /// Gets the dependencies checker service
    fn get_deps_checker(
        configuration: Arc<Configuration>,
        service_index: Arc<dyn index::Index + Send + Sync>,
        service_rustsec: Arc<dyn rustsec::RustSecChecker + Send + Sync>,
    ) -> Arc<dyn deps::DepsChecker + Send + Sync> {
        deps::get_service(configuration, service_index, service_rustsec)
    }

    /// Gets the email sender service
    fn get_email_sender(config: Arc<Configuration>) -> Arc<dyn emails::EmailSender + Send + Sync> {
        emails::get_service(config)
    }

    /// Gets the documentation generation service
    fn get_docs_generator(
        configuration: Arc<Configuration>,
        service_db_pool: RwSqlitePool,
        service_storage: Arc<dyn storage::Storage + Send + Sync>,
        worker_nodes: WorkersManager,
    ) -> Arc<dyn docs::DocsGenerator + Send + Sync> {
        docs::get_service(configuration, service_db_pool, service_storage, worker_nodes)
    }
}
