/*******************************************************************************
 * Copyright (c) 2024 Cénotélie Opérations SAS (cenotelie.fr)
 ******************************************************************************/

//! Service implementations

use std::sync::Arc;

use crate::model::config::Configuration;
use crate::utils::apierror::ApiError;
use crate::utils::db::RwSqlitePool;

pub mod database;
pub mod deps;
pub mod docs;
pub mod emails;
pub mod index;
pub mod rustsec;
pub mod storage;

/// Factory responsible for building services
#[allow(async_fn_in_trait)]
pub trait ServiceProvider {
    /// Gets the configuration
    async fn get_configuration() -> Result<Configuration, ApiError>;

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
    ) -> Arc<dyn docs::DocsGenerator + Send + Sync>;
}

/// Provides the standard implementations for services
pub struct StandardServiceProvider;

impl ServiceProvider for StandardServiceProvider {
    /// Gets the configuration
    async fn get_configuration() -> Result<Configuration, ApiError> {
        let configuration = Configuration::from_env().await?;
        configuration.write_auth_config().await?;
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
    ) -> Arc<dyn docs::DocsGenerator + Send + Sync> {
        docs::get_service(configuration, service_db_pool, service_storage)
    }
}
