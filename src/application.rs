/*******************************************************************************
 * Copyright (c) 2024 Cénotélie Opérations SAS (cenotelie.fr)
 ******************************************************************************/

//! Main application

use std::ops::Deref;
use std::sync::Arc;

use log::{error, info};
use sqlx::Sqlite;
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};

use crate::model::auth::{Authentication, RegistryUserToken, RegistryUserTokenWithSecret, TokenUsageEvent};
use crate::model::cargo::{
    CrateUploadData, CrateUploadResult, OwnersQueryResult, RegistryUser, SearchResults, YesNoMsgResult, YesNoResult,
};
use crate::model::config::Configuration;
use crate::model::deps::DepsAnalysis;
use crate::model::docs::{DocGenJob, DocGenJobUpdate, DocGenTrigger};
use crate::model::packages::CrateInfo;
use crate::model::stats::{DownloadStats, GlobalStats};
use crate::model::{CrateVersion, JobCrate, RegistryInformation};
use crate::services::database::Database;
use crate::services::deps::DepsChecker;
use crate::services::docs::DocsGenerator;
use crate::services::emails::EmailSender;
use crate::services::index::Index;
use crate::services::rustsec::RustSecChecker;
use crate::services::storage::Storage;
use crate::utils::apierror::{error_invalid_request, error_unauthorized, specialize, ApiError};
use crate::utils::axum::auth::{AuthData, Token};
use crate::utils::db::{in_transaction, AppTransaction, RwSqlitePool};

/// The state of this application for axum
pub struct Application {
    /// The configuration
    pub configuration: Arc<Configuration>,
    /// The database pool
    service_db_pool: RwSqlitePool,
    /// The storage layer
    service_storage: Arc<dyn Storage + Send + Sync>,
    /// Service to index the metadata of crates
    service_index: Arc<dyn Index + Send + Sync>,
    /// The `RustSec` checker service
    #[allow(dead_code)]
    service_rustsec: Arc<dyn RustSecChecker + Send + Sync>,
    /// Service to check the dependencies of a crate
    service_deps_checker: Arc<dyn DepsChecker + Send + Sync>,
    /// The service to send emails
    #[allow(dead_code)]
    service_email_sender: Arc<dyn EmailSender + Send + Sync>,
    /// The service to generator documentation
    service_docs_generator: Arc<dyn DocsGenerator + Send + Sync>,
    /// The sender to use to notify about the usage of a token
    token_usage_update: UnboundedSender<TokenUsageEvent>,
}

/// The empty database
const DB_EMPTY: &[u8] = include_bytes!("empty.db");

impl Application {
    /// Creates a new application
    pub async fn launch() -> Result<Arc<Self>, ApiError> {
        // load configuration
        let configuration = Arc::new(Configuration::from_env().await?);
        // write the auth data
        configuration.write_auth_config().await?;

        // connection pool to the database
        let db_filename = configuration.get_database_filename();
        if tokio::fs::metadata(&db_filename).await.is_err() {
            // write the file
            info!("db file is inaccessible => attempt to create an empty one");
            tokio::fs::write(&db_filename, DB_EMPTY).await?;
        }
        let service_db_pool = RwSqlitePool::new(&configuration.get_database_url())?;
        // migrate the database, if appropriate
        crate::migrations::migrate_to_last(&mut *service_db_pool.acquire_write("migrate_to_last").await?).await?;

        let service_storage = crate::services::storage::get_storage(&configuration.deref().clone());
        let service_index = crate::services::index::get_index(&configuration).await?;
        let service_rustsec = crate::services::rustsec::get_rustsec(&configuration);
        let service_deps_checker =
            crate::services::deps::get_deps_checker(configuration.clone(), service_index.clone(), service_rustsec.clone());
        let service_email_sender = crate::services::emails::get_deps_checker(configuration.clone());
        let service_docs_generator =
            crate::services::docs::get_docs_generator(configuration.clone(), service_db_pool.clone(), service_storage.clone());

        // check undocumented packages
        let job_specs = {
            let mut connection = service_db_pool.acquire_read().await?;
            in_transaction(&mut connection, |transaction| async move {
                let app = Database::new(transaction);
                let jobs = app.get_undocumented_crates().await?;
                Ok::<_, ApiError>(jobs)
            })
            .await
        }?;
        for spec in &job_specs {
            service_docs_generator.queue(spec, &DocGenTrigger::MissingOnLaunch).await?;
        }

        // deps worker
        crate::services::deps::create_deps_worker(
            configuration.clone(),
            service_deps_checker.clone(),
            service_email_sender.clone(),
            service_db_pool.clone(),
        );

        let (token_usage_update, token_usage_receiver) = unbounded_channel();

        let this = Arc::new(Self {
            configuration,
            service_db_pool,
            service_storage,
            service_index,
            service_rustsec,
            service_deps_checker,
            service_email_sender,
            service_docs_generator,
            token_usage_update,
        });

        let _handle = {
            let app = this.clone();
            tokio::spawn(async move {
                app.token_usage_worker(token_usage_receiver).await;
            })
        };

        Ok(this)
    }

    /// Gets the storage service
    pub fn get_service_storage(&self) -> Arc<dyn Storage + Send + Sync> {
        self.service_storage.clone()
    }

    /// Gets the index service
    pub fn get_service_index(&self) -> &(dyn Index + Send + Sync) {
        self.service_index.as_ref()
    }

    /// Creates the application with transaction
    pub fn with_transaction<'a, 'c>(&'a self, transaction: AppTransaction<'c>) -> ApplicationWithTransaction<'a, 'c> {
        ApplicationWithTransaction {
            application: self,
            database: Database { transaction },
        }
    }

    /// The worker to handle the update of token usage
    async fn token_usage_worker(&self, mut receiver: UnboundedReceiver<TokenUsageEvent>) {
        const BUFFER_SIZE: usize = 16;
        let mut events = Vec::with_capacity(BUFFER_SIZE);
        loop {
            let count = receiver.recv_many(&mut events, BUFFER_SIZE).await;
            if count == 0 {
                break;
            }
            if let Err(e) = self.token_usage_worker_on_events(&events).await {
                error!("{e}");
                if let Some(backtrace) = e.backtrace {
                    error!("{backtrace}");
                }
            }
        }
    }

    /// Handles a set of events
    async fn token_usage_worker_on_events(&self, events: &[TokenUsageEvent]) -> Result<(), ApiError> {
        let mut connection = self.service_db_pool.acquire_write("update_token_last_usage").await?;
        in_transaction(&mut connection, |transaction| async move {
            let app = self.with_transaction(transaction);
            for event in events {
                app.database.update_token_last_usage(event).await?;
            }
            Ok::<_, ApiError>(())
        })
        .await?;
        Ok(())
    }

    /// Attempts the authentication of a user
    pub async fn authenticate(&self, auth_data: &AuthData) -> Result<Authentication, ApiError> {
        let mut connection = self.service_db_pool.acquire_read().await?;
        in_transaction(&mut connection, |transaction| async move {
            self.with_transaction(transaction).authenticate(auth_data).await
        })
        .await
    }

    /// Gets the registry configuration
    pub fn get_registry_information(&self) -> RegistryInformation {
        RegistryInformation {
            registry_name: self.configuration.self_local_name.clone(),
        }
    }

    /// Gets the data about the current user
    pub async fn get_current_user(&self, auth_data: &AuthData) -> Result<RegistryUser, ApiError> {
        let mut connection = self.service_db_pool.acquire_read().await?;
        in_transaction(&mut connection, |transaction| async move {
            let app = self.with_transaction(transaction);
            let principal = app.authenticate(auth_data).await?;
            app.database.get_current_user(&principal).await
        })
        .await
    }

    /// Attempts to login using an OAuth code
    pub async fn login_with_oauth_code(&self, code: &str) -> Result<RegistryUser, ApiError> {
        let mut connection = self.service_db_pool.acquire_write("login_with_oauth_code").await?;
        in_transaction(&mut connection, |transaction| async move {
            let app = self.with_transaction(transaction);
            app.database.login_with_oauth_code(&self.configuration, code).await
        })
        .await
    }

    /// Gets the known users
    pub async fn get_users(&self, auth_data: &AuthData) -> Result<Vec<RegistryUser>, ApiError> {
        let mut connection = self.service_db_pool.acquire_read().await?;
        in_transaction(&mut connection, |transaction| async move {
            let app = self.with_transaction(transaction);
            let principal = app.authenticate(auth_data).await?;
            app.database.get_users(&principal).await
        })
        .await
    }

    /// Updates the information of a user
    pub async fn update_user(&self, auth_data: &AuthData, target: &RegistryUser) -> Result<RegistryUser, ApiError> {
        let mut connection = self.service_db_pool.acquire_write("update_user").await?;
        in_transaction(&mut connection, |transaction| async move {
            let app = self.with_transaction(transaction);
            let principal = app.authenticate(auth_data).await?;
            app.database.update_user(&principal, target).await
        })
        .await
    }

    /// Attempts to deactivate a user
    pub async fn deactivate_user(&self, auth_data: &AuthData, target: &str) -> Result<(), ApiError> {
        let mut connection = self.service_db_pool.acquire_write("deactivate_user").await?;
        in_transaction(&mut connection, |transaction| async move {
            let app = self.with_transaction(transaction);
            let principal = app.authenticate(auth_data).await?;
            app.database.deactivate_user(&principal, target).await
        })
        .await
    }

    /// Attempts to re-activate a user
    pub async fn reactivate_user(&self, auth_data: &AuthData, target: &str) -> Result<(), ApiError> {
        let mut connection = self.service_db_pool.acquire_write("reactivate_user").await?;
        in_transaction(&mut connection, |transaction| async move {
            let app = self.with_transaction(transaction);
            let principal = app.authenticate(auth_data).await?;
            app.database.reactivate_user(&principal, target).await
        })
        .await
    }

    /// Attempts to delete a user
    pub async fn delete_user(&self, auth_data: &AuthData, target: &str) -> Result<(), ApiError> {
        let mut connection = self.service_db_pool.acquire_write("delete_user").await?;
        in_transaction(&mut connection, |transaction| async move {
            let app = self.with_transaction(transaction);
            let principal = app.authenticate(auth_data).await?;
            app.database.delete_user(&principal, target).await
        })
        .await
    }

    /// Gets the tokens for a user
    pub async fn get_tokens(&self, auth_data: &AuthData) -> Result<Vec<RegistryUserToken>, ApiError> {
        let mut connection = self.service_db_pool.acquire_read().await?;
        in_transaction(&mut connection, |transaction| async move {
            let app = self.with_transaction(transaction);
            let principal = app.authenticate(auth_data).await?;
            app.database.get_tokens(&principal).await
        })
        .await
    }

    /// Creates a token for the current user
    pub async fn create_token(
        &self,
        auth_data: &AuthData,
        name: &str,
        can_write: bool,
        can_admin: bool,
    ) -> Result<RegistryUserTokenWithSecret, ApiError> {
        let mut connection = self.service_db_pool.acquire_write("create_token").await?;
        in_transaction(&mut connection, |transaction| async move {
            let app = self.with_transaction(transaction);
            let principal = app.authenticate(auth_data).await?;
            app.database.create_token(&principal, name, can_write, can_admin).await
        })
        .await
    }

    /// Revoke a previous token
    pub async fn revoke_token(&self, auth_data: &AuthData, token_id: i64) -> Result<(), ApiError> {
        let mut connection = self.service_db_pool.acquire_write("revoke_token").await?;
        in_transaction(&mut connection, |transaction| async move {
            let app = self.with_transaction(transaction);
            let principal = app.authenticate(auth_data).await?;
            app.database.revoke_token(&principal, token_id).await
        })
        .await
    }

    /// Gets the global tokens for the registry, usually for CI purposes
    pub async fn get_global_tokens(&self, auth_data: &AuthData) -> Result<Vec<RegistryUserToken>, ApiError> {
        let mut connection = self.service_db_pool.acquire_read().await?;
        in_transaction(&mut connection, |transaction| async move {
            let app = self.with_transaction(transaction);
            let principal = app.authenticate(auth_data).await?;
            app.database.get_global_tokens(&principal).await
        })
        .await
    }

    /// Creates a global token for the registry
    pub async fn create_global_token(&self, auth_data: &AuthData, name: &str) -> Result<RegistryUserTokenWithSecret, ApiError> {
        let mut connection = self.service_db_pool.acquire_write("create_global_token").await?;
        in_transaction(&mut connection, |transaction| async move {
            let app = self.with_transaction(transaction);
            let principal = app.authenticate(auth_data).await?;
            app.database.create_global_token(&principal, name).await
        })
        .await
    }

    /// Revokes a globel token for the registry
    pub async fn revoke_global_token(&self, auth_data: &AuthData, token_id: i64) -> Result<(), ApiError> {
        let mut connection = self.service_db_pool.acquire_write("revoke_global_token").await?;
        in_transaction(&mut connection, |transaction| async move {
            let app = self.with_transaction(transaction);
            let principal = app.authenticate(auth_data).await?;
            app.database.revoke_global_token(&principal, token_id).await
        })
        .await
    }

    /// Publish a crate
    pub async fn publish_crate_version(&self, auth_data: &AuthData, content: &[u8]) -> Result<CrateUploadResult, ApiError> {
        let mut connection = self.service_db_pool.acquire_write("publish_crate_version").await?;
        in_transaction(&mut connection, |transaction| async move {
            let app = self.with_transaction(transaction);
            let principal = app.authenticate(auth_data).await?;
            let user = app.database.get_user_profile(principal.uid()?).await?;
            // deserialize payload
            let package = CrateUploadData::new(content)?;
            let index_data = package.build_index_data();
            // publish
            let r = app.database.publish_crate_version(&principal, &package).await?;
            self.service_storage.store_crate(&package.metadata, package.content).await?;
            self.service_index.publish_crate_version(&index_data).await?;
            let targets = app.database.get_crate_targets(&package.metadata.name).await?;
            // generate the doc
            self.service_docs_generator
                .queue(
                    &JobCrate {
                        name: package.metadata.name.clone(),
                        version: package.metadata.vers.clone(),
                        targets,
                    },
                    &DocGenTrigger::Upload { by: user },
                )
                .await?;
            Ok(r)
        })
        .await
    }

    /// Gets all the data about a crate
    pub async fn get_crate_info(&self, auth_data: &AuthData, package: &str) -> Result<CrateInfo, ApiError> {
        let mut connection = self.service_db_pool.acquire_read().await?;
        in_transaction(&mut connection, |transaction| async move {
            let app = self.with_transaction(transaction);
            let _principal = app.authenticate(auth_data).await?;
            let versions = app
                .database
                .get_crate_versions(package, self.service_index.get_crate_data(package).await?)
                .await?;
            let metadata = self
                .service_storage
                .download_crate_metadata(package, &versions.last().unwrap().index.vers)
                .await?;
            let targets = app.database.get_crate_targets(package).await?;
            Ok(CrateInfo {
                metadata,
                versions,
                targets,
            })
        })
        .await
    }

    /// Downloads the last README for a crate
    pub async fn get_crate_last_readme(&self, auth_data: &AuthData, package: &str) -> Result<Vec<u8>, ApiError> {
        let mut connection = self.service_db_pool.acquire_read().await?;
        in_transaction(&mut connection, |transaction| async move {
            let app = self.with_transaction(transaction);
            let _principal = app.authenticate(auth_data).await?;
            let version = app.database.get_crate_last_version(package).await?;
            let readme = self.service_storage.download_crate_readme(package, &version).await?;
            Ok(readme)
        })
        .await
    }

    /// Downloads the README for a crate
    pub async fn get_crate_readme(&self, auth_data: &AuthData, package: &str, version: &str) -> Result<Vec<u8>, ApiError> {
        let mut connection = self.service_db_pool.acquire_read().await?;
        in_transaction(&mut connection, |transaction| async move {
            let app = self.with_transaction(transaction);
            let _principal = app.authenticate(auth_data).await?;
            let readme = self.service_storage.download_crate_readme(package, version).await?;
            Ok(readme)
        })
        .await
    }

    /// Downloads the content for a crate
    pub async fn get_crate_content(&self, auth_data: &AuthData, package: &str, version: &str) -> Result<Vec<u8>, ApiError> {
        let mut connection = self.service_db_pool.acquire_read().await?;
        in_transaction(&mut connection, |transaction| async move {
            let app = self.with_transaction(transaction);
            let _principal = app.authenticate(auth_data).await?;
            app.database.check_crate_exists(package, version).await?;
            app.database.increment_crate_version_dl_count(package, version).await?;
            let content = self.service_storage.download_crate(package, version).await?;
            Ok(content)
        })
        .await
    }

    /// Yank a crate version
    pub async fn yank_crate_version(
        &self,
        auth_data: &AuthData,
        package: &str,
        version: &str,
    ) -> Result<YesNoResult, ApiError> {
        let mut connection = self.service_db_pool.acquire_write("yank_crate_version").await?;
        in_transaction(&mut connection, |transaction| async move {
            let app = self.with_transaction(transaction);
            let principal = app.authenticate(auth_data).await?;
            app.database.yank_crate_version(&principal, package, version).await
        })
        .await
    }

    /// Unyank a crate version
    pub async fn unyank_crate_version(
        &self,
        auth_data: &AuthData,
        package: &str,
        version: &str,
    ) -> Result<YesNoResult, ApiError> {
        let mut connection = self.service_db_pool.acquire_write("unyank_crate_version").await?;
        in_transaction(&mut connection, |transaction| async move {
            let app = self.with_transaction(transaction);
            let principal = app.authenticate(auth_data).await?;
            app.database.unyank_crate_version(&principal, package, version).await
        })
        .await
    }

    /// Gets the packages that need documentation generation
    pub async fn get_undocumented_crates(&self, auth_data: &AuthData) -> Result<Vec<CrateVersion>, ApiError> {
        let mut connection: sqlx::pool::PoolConnection<Sqlite> = self.service_db_pool.acquire_read().await?;
        in_transaction(&mut connection, |transaction| async move {
            let app = self.with_transaction(transaction);
            let _principal = app.authenticate(auth_data).await?;
            let crates = app.database.get_undocumented_crates().await?;
            Ok(crates.into_iter().map(CrateVersion::from).collect())
        })
        .await
    }

    /// Gets the documentation jobs
    pub async fn get_doc_gen_jobs(&self, auth_data: &AuthData) -> Result<Vec<DocGenJob>, ApiError> {
        let mut connection: sqlx::pool::PoolConnection<Sqlite> = self.service_db_pool.acquire_read().await?;
        in_transaction(&mut connection, |transaction| async move {
            let app = self.with_transaction(transaction);
            let _principal = app.authenticate(auth_data).await?;
            self.service_docs_generator.get_jobs().await
        })
        .await
    }

    /// Adds a listener to job updates
    pub async fn get_doc_gen_job_updates(&self, auth_data: &AuthData) -> Result<UnboundedReceiver<DocGenJobUpdate>, ApiError> {
        let mut connection: sqlx::pool::PoolConnection<Sqlite> = self.service_db_pool.acquire_read().await?;
        in_transaction(&mut connection, |transaction| async move {
            let app = self.with_transaction(transaction);
            let _principal = app.authenticate(auth_data).await?;
            Ok::<_, ApiError>(())
        })
        .await?;
        let (sender, receiver) = unbounded_channel();
        self.service_docs_generator.add_update_listener(sender);
        Ok(receiver)
    }

    /// Force the re-generation for the documentation of a package
    pub async fn regen_crate_version_doc(
        &self,
        auth_data: &AuthData,
        package: &str,
        version: &str,
    ) -> Result<DocGenJob, ApiError> {
        let mut connection: sqlx::pool::PoolConnection<Sqlite> =
            self.service_db_pool.acquire_write("regen_crate_version_doc").await?;
        let (user, targets) = in_transaction(&mut connection, |transaction| async move {
            let app = self.with_transaction(transaction);
            let principal = app.authenticate(auth_data).await?;
            let user = app.database.get_user_profile(principal.uid()?).await?;
            app.database.regen_crate_version_doc(&principal, package, version).await?;
            let targets = app.database.get_crate_targets(package).await?;
            Ok::<_, ApiError>((user, targets))
        })
        .await?;
        drop(connection);

        self.service_docs_generator
            .queue(
                &JobCrate {
                    name: package.to_string(),
                    version: version.to_string(),
                    targets,
                },
                &DocGenTrigger::Manual { by: user },
            )
            .await
    }

    /// Gets all the packages that are outdated while also being the latest version
    pub async fn get_crates_outdated_heads(&self, auth_data: &AuthData) -> Result<Vec<CrateVersion>, ApiError> {
        let mut connection: sqlx::pool::PoolConnection<Sqlite> = self.service_db_pool.acquire_read().await?;
        in_transaction(&mut connection, |transaction| async move {
            let app = self.with_transaction(transaction);
            let _principal = app.authenticate(auth_data).await?;
            app.database.get_crates_outdated_heads().await
        })
        .await
    }

    /// Gets the download statistics for a crate
    pub async fn get_crate_dl_stats(&self, auth_data: &AuthData, package: &str) -> Result<DownloadStats, ApiError> {
        let mut connection: sqlx::pool::PoolConnection<Sqlite> = self.service_db_pool.acquire_read().await?;
        in_transaction(&mut connection, |transaction| async move {
            let app = self.with_transaction(transaction);
            let _principal = app.authenticate(auth_data).await?;
            app.database.get_crate_dl_stats(package).await
        })
        .await
    }

    /// Gets the list of owners for a package
    pub async fn get_crate_owners(&self, auth_data: &AuthData, package: &str) -> Result<OwnersQueryResult, ApiError> {
        let mut connection: sqlx::pool::PoolConnection<Sqlite> = self.service_db_pool.acquire_read().await?;
        in_transaction(&mut connection, |transaction| async move {
            let app = self.with_transaction(transaction);
            let _principal = app.authenticate(auth_data).await?;
            app.database.get_crate_owners(package).await
        })
        .await
    }

    /// Add owners to a package
    pub async fn add_crate_owners(
        &self,
        auth_data: &AuthData,
        package: &str,
        new_users: &[String],
    ) -> Result<YesNoMsgResult, ApiError> {
        let mut connection: sqlx::pool::PoolConnection<Sqlite> = self.service_db_pool.acquire_write("add_crate_owners").await?;
        in_transaction(&mut connection, |transaction| async move {
            let app = self.with_transaction(transaction);
            let principal = app.authenticate(auth_data).await?;
            app.database.add_crate_owners(&principal, package, new_users).await
        })
        .await
    }

    /// Remove owners from a package
    pub async fn remove_crate_owners(
        &self,
        auth_data: &AuthData,
        package: &str,
        old_users: &[String],
    ) -> Result<YesNoResult, ApiError> {
        let mut connection: sqlx::pool::PoolConnection<Sqlite> =
            self.service_db_pool.acquire_write("remove_crate_owners").await?;
        in_transaction(&mut connection, |transaction| async move {
            let app = self.with_transaction(transaction);
            let principal = app.authenticate(auth_data).await?;
            app.database.remove_crate_owners(&principal, package, old_users).await
        })
        .await
    }

    /// Gets the targets for a crate
    pub async fn get_crate_targets(&self, auth_data: &AuthData, package: &str) -> Result<Vec<String>, ApiError> {
        let mut connection: sqlx::pool::PoolConnection<Sqlite> = self.service_db_pool.acquire_read().await?;
        in_transaction(&mut connection, |transaction| async move {
            let app = self.with_transaction(transaction);
            let _principal = app.authenticate(auth_data).await?;
            app.database.get_crate_targets(package).await
        })
        .await
    }

    /// Sets the targets for a crate
    pub async fn set_crate_targets(&self, auth_data: &AuthData, package: &str, targets: &[String]) -> Result<(), ApiError> {
        let mut connection: sqlx::pool::PoolConnection<Sqlite> =
            self.service_db_pool.acquire_write("set_crate_targets").await?;
        in_transaction(&mut connection, |transaction| async move {
            let app = self.with_transaction(transaction);
            let principal = app.authenticate(auth_data).await?;
            for target in targets {
                if !self.configuration.self_builtin_targets.contains(target) {
                    return Err(specialize(error_invalid_request(), format!("Unknown target: {target}")));
                }
            }
            app.database.set_crate_targets(&principal, package, targets).await
        })
        .await
    }

    /// Gets the global statistics for the registry
    pub async fn get_crates_stats(&self, auth_data: &AuthData) -> Result<GlobalStats, ApiError> {
        let mut connection: sqlx::pool::PoolConnection<Sqlite> = self.service_db_pool.acquire_read().await?;
        in_transaction(&mut connection, |transaction| async move {
            let app = self.with_transaction(transaction);
            let _principal = app.authenticate(auth_data).await?;
            app.database.get_crates_stats().await
        })
        .await
    }

    /// Search for crates
    pub async fn search_crates(
        &self,
        auth_data: &AuthData,
        query: &str,
        per_page: Option<usize>,
    ) -> Result<SearchResults, ApiError> {
        let mut connection: sqlx::pool::PoolConnection<Sqlite> = self.service_db_pool.acquire_read().await?;
        in_transaction(&mut connection, |transaction| async move {
            let app = self.with_transaction(transaction);
            let _principal = app.authenticate(auth_data).await?;
            app.database.search_crates(query, per_page).await
        })
        .await
    }

    /// Checks the dependencies of a local crate
    pub async fn check_crate_version_deps(
        &self,
        auth_data: &AuthData,
        package: &str,
        version: &str,
    ) -> Result<DepsAnalysis, ApiError> {
        let mut connection: sqlx::pool::PoolConnection<Sqlite> = self.service_db_pool.acquire_read().await?;
        let targets = in_transaction(&mut connection, |transaction| async move {
            let app = self.with_transaction(transaction);
            let _principal = app.authenticate(auth_data).await?;
            app.database.check_crate_exists(package, version).await?;
            app.database.get_crate_targets(package).await
        })
        .await?;
        self.service_deps_checker.check_crate(package, version, &targets).await
    }
}

/// The application, running with a transaction
pub struct ApplicationWithTransaction<'a, 'c> {
    /// The application with its services
    application: &'a Application,
    /// The database access encapsulating a transaction
    database: Database<'c>,
}

impl<'a, 'c> ApplicationWithTransaction<'a, 'c> {
    /// Attempts the authentication of a user
    pub async fn authenticate(&self, auth_data: &AuthData) -> Result<Authentication, ApiError> {
        if let Some(token) = &auth_data.token {
            self.authenticate_token(token).await
        } else {
            let authenticated_user = auth_data.try_authenticate_cookie()?.ok_or_else(error_unauthorized)?;
            self.database.check_is_user(authenticated_user.email()?).await?;
            Ok(authenticated_user)
        }
    }

    /// Tries to authenticate using a token
    pub async fn authenticate_token(&self, token: &Token) -> Result<Authentication, ApiError> {
        if token.id == self.application.configuration.self_service_login
            && token.secret == self.application.configuration.self_service_token
        {
            // self authentication to read
            return Ok(Authentication::new_self());
        }
        let user = self
            .database
            .check_token(&token.id, &token.secret, &|event| {
                self.application.token_usage_update.send(event).unwrap();
            })
            .await?;
        Ok(user)
    }
}
