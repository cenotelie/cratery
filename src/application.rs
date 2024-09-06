/*******************************************************************************
 * Copyright (c) 2024 Cénotélie Opérations SAS (cenotelie.fr)
 ******************************************************************************/

//! Main application

use std::ops::Deref;
use std::sync::Arc;

use log::{error, info};
use tokio::sync::mpsc::{channel, Receiver, Sender};

use crate::model::auth::{Authentication, RegistryUserToken, RegistryUserTokenWithSecret};
use crate::model::cargo::{
    CrateUploadData, CrateUploadResult, OwnersQueryResult, RegistryUser, SearchResults, YesNoMsgResult, YesNoResult,
};
use crate::model::config::Configuration;
use crate::model::deps::DepsAnalysis;
use crate::model::docs::{DocGenEvent, DocGenJob, DocGenTrigger};
use crate::model::packages::CrateInfo;
use crate::model::stats::{DownloadStats, GlobalStats};
use crate::model::{AppEvent, CrateVersion, JobCrate, RegistryInformation};
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
    /// Sender to use to notify about events that will be asynchronously handled
    app_events_sender: Sender<AppEvent>,
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

        let (app_events_sender, app_events_receiver) = channel(64);

        let this = Arc::new(Self {
            configuration,
            service_db_pool,
            service_storage,
            service_index,
            service_rustsec,
            service_deps_checker,
            service_email_sender,
            service_docs_generator,
            app_events_sender,
        });

        let _handle = {
            let app = this.clone();
            tokio::spawn(async move {
                app.events_handler(app_events_receiver).await;
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
    async fn events_handler(&self, mut receiver: Receiver<AppEvent>) {
        const BUFFER_SIZE: usize = 16;
        let mut events = Vec::with_capacity(BUFFER_SIZE);
        loop {
            let count = receiver.recv_many(&mut events, BUFFER_SIZE).await;
            if count == 0 {
                break;
            }
            if let Err(e) = self.events_handler_handle(&events).await {
                error!("{e}");
                if let Some(backtrace) = e.backtrace {
                    error!("{backtrace}");
                }
            }
            events.clear();
        }
    }

    /// Handles a set of events
    async fn events_handler_handle(&self, events: &[AppEvent]) -> Result<(), ApiError> {
        let mut connection = self.service_db_pool.acquire_write("events_handler_handle").await?;
        in_transaction(&mut connection, |transaction| async move {
            let app = self.with_transaction(transaction);
            for event in events {
                match event {
                    AppEvent::TokenUse(usage) => {
                        app.database.update_token_last_usage(usage).await?;
                    }
                    AppEvent::CrateDownload(CrateVersion { name, version }) => {
                        app.database.increment_crate_version_dl_count(name, version).await?;
                    }
                }
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
            let authentication = app.authenticate(auth_data).await?;
            app.database.get_current_user(&authentication).await
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
            let authentication = app.authenticate(auth_data).await?;
            app.database.get_users(&authentication).await
        })
        .await
    }

    /// Updates the information of a user
    pub async fn update_user(&self, auth_data: &AuthData, target: &RegistryUser) -> Result<RegistryUser, ApiError> {
        let mut connection = self.service_db_pool.acquire_write("update_user").await?;
        in_transaction(&mut connection, |transaction| async move {
            let app = self.with_transaction(transaction);
            let authentication = app.authenticate(auth_data).await?;
            app.database.update_user(&authentication, target).await
        })
        .await
    }

    /// Attempts to deactivate a user
    pub async fn deactivate_user(&self, auth_data: &AuthData, target: &str) -> Result<(), ApiError> {
        let mut connection = self.service_db_pool.acquire_write("deactivate_user").await?;
        in_transaction(&mut connection, |transaction| async move {
            let app = self.with_transaction(transaction);
            let authentication = app.authenticate(auth_data).await?;
            app.database.deactivate_user(&authentication, target).await
        })
        .await
    }

    /// Attempts to re-activate a user
    pub async fn reactivate_user(&self, auth_data: &AuthData, target: &str) -> Result<(), ApiError> {
        let mut connection = self.service_db_pool.acquire_write("reactivate_user").await?;
        in_transaction(&mut connection, |transaction| async move {
            let app = self.with_transaction(transaction);
            let authentication = app.authenticate(auth_data).await?;
            app.database.reactivate_user(&authentication, target).await
        })
        .await
    }

    /// Attempts to delete a user
    pub async fn delete_user(&self, auth_data: &AuthData, target: &str) -> Result<(), ApiError> {
        let mut connection = self.service_db_pool.acquire_write("delete_user").await?;
        in_transaction(&mut connection, |transaction| async move {
            let app = self.with_transaction(transaction);
            let authentication = app.authenticate(auth_data).await?;
            app.database.delete_user(&authentication, target).await
        })
        .await
    }

    /// Gets the tokens for a user
    pub async fn get_tokens(&self, auth_data: &AuthData) -> Result<Vec<RegistryUserToken>, ApiError> {
        let mut connection = self.service_db_pool.acquire_read().await?;
        in_transaction(&mut connection, |transaction| async move {
            let app = self.with_transaction(transaction);
            let authentication = app.authenticate(auth_data).await?;
            app.database.get_tokens(&authentication).await
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
            let authentication = app.authenticate(auth_data).await?;
            app.database.create_token(&authentication, name, can_write, can_admin).await
        })
        .await
    }

    /// Revoke a previous token
    pub async fn revoke_token(&self, auth_data: &AuthData, token_id: i64) -> Result<(), ApiError> {
        let mut connection = self.service_db_pool.acquire_write("revoke_token").await?;
        in_transaction(&mut connection, |transaction| async move {
            let app = self.with_transaction(transaction);
            let authentication = app.authenticate(auth_data).await?;
            app.database.revoke_token(&authentication, token_id).await
        })
        .await
    }

    /// Gets the global tokens for the registry, usually for CI purposes
    pub async fn get_global_tokens(&self, auth_data: &AuthData) -> Result<Vec<RegistryUserToken>, ApiError> {
        let mut connection = self.service_db_pool.acquire_read().await?;
        in_transaction(&mut connection, |transaction| async move {
            let app = self.with_transaction(transaction);
            let authentication = app.authenticate(auth_data).await?;
            app.database.get_global_tokens(&authentication).await
        })
        .await
    }

    /// Creates a global token for the registry
    pub async fn create_global_token(&self, auth_data: &AuthData, name: &str) -> Result<RegistryUserTokenWithSecret, ApiError> {
        let mut connection = self.service_db_pool.acquire_write("create_global_token").await?;
        in_transaction(&mut connection, |transaction| async move {
            let app = self.with_transaction(transaction);
            let authentication = app.authenticate(auth_data).await?;
            app.database.create_global_token(&authentication, name).await
        })
        .await
    }

    /// Revokes a globel token for the registry
    pub async fn revoke_global_token(&self, auth_data: &AuthData, token_id: i64) -> Result<(), ApiError> {
        let mut connection = self.service_db_pool.acquire_write("revoke_global_token").await?;
        in_transaction(&mut connection, |transaction| async move {
            let app = self.with_transaction(transaction);
            let authentication = app.authenticate(auth_data).await?;
            app.database.revoke_global_token(&authentication, token_id).await
        })
        .await
    }

    /// Publish a crate
    pub async fn publish_crate_version(&self, auth_data: &AuthData, content: &[u8]) -> Result<CrateUploadResult, ApiError> {
        // deserialize payload
        let package = CrateUploadData::new(content)?;
        let index_data = package.build_index_data();

        let (user, result, targets) = {
            let package = &package;
            let mut connection = self.service_db_pool.acquire_write("publish_crate_version").await?;
            in_transaction(&mut connection, |transaction| async move {
                let app = self.with_transaction(transaction);
                let authentication = app.authenticate(auth_data).await?;
                let user = app.database.get_user_profile(authentication.uid()?).await?;
                // publish
                let result = app.database.publish_crate_version(&authentication, package).await?;
                let targets = app.database.get_crate_targets(&package.metadata.name).await?;
                Ok::<_, ApiError>((user, result, targets))
            })
            .await
        }?;

        self.service_storage.store_crate(&package.metadata, package.content).await?;
        self.service_index.publish_crate_version(&index_data).await?;
        self.service_docs_generator
            .queue(
                &JobCrate {
                    name: index_data.name.clone(),
                    version: index_data.vers.clone(),
                    targets,
                },
                &DocGenTrigger::Upload { by: user },
            )
            .await?;
        Ok(result)
    }

    /// Gets all the data about a crate
    pub async fn get_crate_info(&self, auth_data: &AuthData, package: &str) -> Result<CrateInfo, ApiError> {
        let (versions, targets) = {
            let mut connection = self.service_db_pool.acquire_read().await?;
            in_transaction(&mut connection, |transaction| async move {
                let app = self.with_transaction(transaction);
                let _authentication = app.authenticate(auth_data).await?;
                let versions = app
                    .database
                    .get_crate_versions(package, self.service_index.get_crate_data(package).await?)
                    .await?;
                let targets = app.database.get_crate_targets(package).await?;
                Ok::<_, ApiError>((versions, targets))
            })
            .await
        }?;
        let metadata = self
            .service_storage
            .download_crate_metadata(package, &versions.last().unwrap().index.vers)
            .await?;
        Ok(CrateInfo {
            metadata,
            versions,
            targets,
        })
    }

    /// Downloads the last README for a crate
    pub async fn get_crate_last_readme(&self, auth_data: &AuthData, package: &str) -> Result<Vec<u8>, ApiError> {
        let version = {
            let mut connection = self.service_db_pool.acquire_read().await?;
            in_transaction(&mut connection, |transaction| async move {
                let app = self.with_transaction(transaction);
                let _authentication = app.authenticate(auth_data).await?;
                let version = app.database.get_crate_last_version(package).await?;
                Ok::<_, ApiError>(version)
            })
            .await
        }?;
        let readme = self.service_storage.download_crate_readme(package, &version).await?;
        Ok(readme)
    }

    /// Downloads the README for a crate
    pub async fn get_crate_readme(&self, auth_data: &AuthData, package: &str, version: &str) -> Result<Vec<u8>, ApiError> {
        let _authentication = self.authenticate(auth_data).await?;
        let readme = self.service_storage.download_crate_readme(package, version).await?;
        Ok(readme)
    }

    /// Downloads the content for a crate
    pub async fn get_crate_content(&self, auth_data: &AuthData, package: &str, version: &str) -> Result<Vec<u8>, ApiError> {
        {
            let mut connection = self.service_db_pool.acquire_read().await?;
            in_transaction(&mut connection, |transaction| async move {
                let app = self.with_transaction(transaction);
                let _authentication = app.authenticate(auth_data).await?;
                app.database.check_crate_exists(package, version).await?;
                Ok::<_, ApiError>(())
            })
            .await?;
        }
        let content = self.service_storage.download_crate(package, version).await?;
        self.app_events_sender
            .send(AppEvent::CrateDownload(CrateVersion {
                name: package.to_string(),
                version: version.to_string(),
            }))
            .await?;
        Ok(content)
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
            let authentication = app.authenticate(auth_data).await?;
            app.database.yank_crate_version(&authentication, package, version).await
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
            let authentication = app.authenticate(auth_data).await?;
            app.database.unyank_crate_version(&authentication, package, version).await
        })
        .await
    }

    /// Gets the packages that need documentation generation
    pub async fn get_undocumented_crates(&self, auth_data: &AuthData) -> Result<Vec<CrateVersion>, ApiError> {
        let mut connection = self.service_db_pool.acquire_read().await?;
        in_transaction(&mut connection, |transaction| async move {
            let app = self.with_transaction(transaction);
            let _authentication = app.authenticate(auth_data).await?;
            let crates = app.database.get_undocumented_crates().await?;
            Ok(crates.into_iter().map(CrateVersion::from).collect())
        })
        .await
    }

    /// Gets the documentation jobs
    pub async fn get_doc_gen_jobs(&self, auth_data: &AuthData) -> Result<Vec<DocGenJob>, ApiError> {
        let _auth = self.authenticate(auth_data).await?;
        self.service_docs_generator.get_jobs().await
    }

    /// Gets the log for a documentation generation job
    pub async fn get_doc_gen_job_log(&self, auth_data: &AuthData, job_id: i64) -> Result<String, ApiError> {
        let _auth = self.authenticate(auth_data).await?;
        self.service_docs_generator.get_job_log(job_id).await
    }

    /// Adds a listener to job updates
    pub async fn get_doc_gen_job_updates(&self, auth_data: &AuthData) -> Result<Receiver<DocGenEvent>, ApiError> {
        let _auth = self.authenticate(auth_data).await?;
        let (sender, receiver) = channel(16);
        self.service_docs_generator.add_listener(sender).await?;
        Ok(receiver)
    }

    /// Force the re-generation for the documentation of a package
    pub async fn regen_crate_version_doc(
        &self,
        auth_data: &AuthData,
        package: &str,
        version: &str,
    ) -> Result<DocGenJob, ApiError> {
        let (user, targets) = {
            let mut connection = self.service_db_pool.acquire_write("regen_crate_version_doc").await?;
            in_transaction(&mut connection, |transaction| async move {
                let app = self.with_transaction(transaction);
                let authentication = app.authenticate(auth_data).await?;
                let user = app.database.get_user_profile(authentication.uid()?).await?;
                let targets = app.database.get_crate_targets(package).await?;
                app.database
                    .regen_crate_version_doc(&authentication, package, version)
                    .await?;
                Ok::<_, ApiError>((user, targets))
            })
            .await
        }?;

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
        let mut connection = self.service_db_pool.acquire_read().await?;
        in_transaction(&mut connection, |transaction| async move {
            let app = self.with_transaction(transaction);
            let _authentication = app.authenticate(auth_data).await?;
            app.database.get_crates_outdated_heads().await
        })
        .await
    }

    /// Gets the download statistics for a crate
    pub async fn get_crate_dl_stats(&self, auth_data: &AuthData, package: &str) -> Result<DownloadStats, ApiError> {
        let mut connection = self.service_db_pool.acquire_read().await?;
        in_transaction(&mut connection, |transaction| async move {
            let app = self.with_transaction(transaction);
            let _authentication = app.authenticate(auth_data).await?;
            app.database.get_crate_dl_stats(package).await
        })
        .await
    }

    /// Gets the list of owners for a package
    pub async fn get_crate_owners(&self, auth_data: &AuthData, package: &str) -> Result<OwnersQueryResult, ApiError> {
        let mut connection = self.service_db_pool.acquire_read().await?;
        in_transaction(&mut connection, |transaction| async move {
            let app = self.with_transaction(transaction);
            let _authentication = app.authenticate(auth_data).await?;
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
        let mut connection = self.service_db_pool.acquire_write("add_crate_owners").await?;
        in_transaction(&mut connection, |transaction| async move {
            let app = self.with_transaction(transaction);
            let authentication = app.authenticate(auth_data).await?;
            app.database.add_crate_owners(&authentication, package, new_users).await
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
        let mut connection = self.service_db_pool.acquire_write("remove_crate_owners").await?;
        in_transaction(&mut connection, |transaction| async move {
            let app = self.with_transaction(transaction);
            let authentication = app.authenticate(auth_data).await?;
            app.database.remove_crate_owners(&authentication, package, old_users).await
        })
        .await
    }

    /// Gets the targets for a crate
    pub async fn get_crate_targets(&self, auth_data: &AuthData, package: &str) -> Result<Vec<String>, ApiError> {
        let mut connection = self.service_db_pool.acquire_read().await?;
        in_transaction(&mut connection, |transaction| async move {
            let app = self.with_transaction(transaction);
            let _authentication = app.authenticate(auth_data).await?;
            app.database.get_crate_targets(package).await
        })
        .await
    }

    /// Sets the targets for a crate
    pub async fn set_crate_targets(&self, auth_data: &AuthData, package: &str, targets: &[String]) -> Result<(), ApiError> {
        let mut connection = self.service_db_pool.acquire_write("set_crate_targets").await?;
        in_transaction(&mut connection, |transaction| async move {
            let app = self.with_transaction(transaction);
            let authentication = app.authenticate(auth_data).await?;
            for target in targets {
                if !self.configuration.self_builtin_targets.contains(target) {
                    return Err(specialize(error_invalid_request(), format!("Unknown target: {target}")));
                }
            }
            app.database.set_crate_targets(&authentication, package, targets).await
        })
        .await
    }

    /// Gets the global statistics for the registry
    pub async fn get_crates_stats(&self, auth_data: &AuthData) -> Result<GlobalStats, ApiError> {
        let mut connection = self.service_db_pool.acquire_read().await?;
        in_transaction(&mut connection, |transaction| async move {
            let app = self.with_transaction(transaction);
            let _authentication = app.authenticate(auth_data).await?;
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
        let mut connection = self.service_db_pool.acquire_read().await?;
        in_transaction(&mut connection, |transaction| async move {
            let app = self.with_transaction(transaction);
            let _authentication = app.authenticate(auth_data).await?;
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
        let mut connection = self.service_db_pool.acquire_read().await?;
        let targets = in_transaction(&mut connection, |transaction| async move {
            let app = self.with_transaction(transaction);
            let _authentication = app.authenticate(auth_data).await?;
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
            let authentication = auth_data.try_authenticate_cookie()?.ok_or_else(error_unauthorized)?;
            self.database.check_is_user(authentication.email()?).await?;
            Ok(authentication)
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
            .check_token(&token.id, &token.secret, &|usage| async move {
                self.application
                    .app_events_sender
                    .send(AppEvent::TokenUse(usage))
                    .await
                    .unwrap();
            })
            .await?;
        Ok(user)
    }
}
