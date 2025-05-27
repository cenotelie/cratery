/*******************************************************************************
 * Copyright (c) 2024 Cénotélie Opérations SAS (cenotelie.fr)
 ******************************************************************************/

//! Main application

use std::future::Future;
use std::ops::Deref;
use std::sync::Arc;

use axum::http::StatusCode;
use log::{error, info};
use thiserror::Error;
use tokio::sync::mpsc::{Receiver, Sender, channel};

use crate::model::auth::{Authentication, RegistryUserToken, RegistryUserTokenWithSecret};
use crate::model::cargo::{
    CrateUploadData, CrateUploadResult, OwnersQueryResult, RegistryUser, SearchResults, YesNoMsgResult, YesNoResult,
};
use crate::model::config::Configuration;
use crate::model::deps::DepsAnalysis;
use crate::model::docs::{DocGenEvent, DocGenJob, DocGenJobSpec, DocGenTrigger};
use crate::model::packages::{CrateInfo, CrateInfoTarget};
use crate::model::stats::{DownloadStats, GlobalStats};
use crate::model::worker::{WorkerEvent, WorkerPublicData, WorkersManager};
use crate::model::{AppEvent, CrateVersion, RegistryInformation};
use crate::services::ServiceProvider;
use crate::services::database::packages::DepsError;
use crate::services::database::{Database, db_transaction_read, db_transaction_write};
use crate::services::deps::DepsChecker;
use crate::services::docs::DocsGenerator;
use crate::services::emails::EmailSender;
use crate::services::index::Index;
use crate::services::rustsec::RustSecChecker;
use crate::services::storage::Storage;
use crate::utils::apierror::{ApiError, AsStatusCode, error_forbidden, error_invalid_request, specialize};
use crate::utils::axum::auth::{AuthData, Token};
use crate::utils::db::RwSqlitePool;

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
    #[expect(dead_code)]
    service_rustsec: Arc<dyn RustSecChecker + Send + Sync>,
    /// Service to check the dependencies of a crate
    service_deps_checker: Arc<dyn DepsChecker + Send + Sync>,
    /// The service to send emails
    #[expect(dead_code)]
    service_email_sender: Arc<dyn EmailSender + Send + Sync>,
    /// The service to generator documentation
    service_docs_generator: Arc<dyn DocsGenerator + Send + Sync>,
    /// Sender to use to notify about events that will be asynchronously handled
    app_events_sender: Sender<AppEvent>,
    /// The connected worker nodes
    pub worker_nodes: WorkersManager,
}

/// The empty database
const DB_EMPTY: &[u8] = include_bytes!("empty.db");

impl Application {
    /// Creates a new application
    pub async fn launch<P: ServiceProvider>(configuration: Configuration) -> Result<Arc<Self>, ApiError> {
        // load configuration
        let configuration = Arc::new(configuration);

        // connection pool to the database
        let db_filename = configuration.get_database_filename();
        if tokio::fs::metadata(&db_filename).await.is_err() {
            // write the file
            info!("db file is inaccessible => attempt to create an empty one");
            tokio::fs::write(&db_filename, DB_EMPTY).await?;
        }
        let service_db_pool = RwSqlitePool::new(&configuration.get_database_url())?;
        // migrate the database, if appropriate
        db_transaction_write(&service_db_pool, "migrate_to_last", |database| async move {
            crate::migrations::migrate_to_last(database.transaction).await
        })
        .await?;

        let worker_nodes = WorkersManager::default();

        let db_is_empty =
            db_transaction_read(&service_db_pool, |database| async move { database.get_is_empty().await }).await?;
        let service_storage = P::get_storage(&configuration.deref().clone());
        let service_index = P::get_index(&configuration, db_is_empty).await?;
        let service_rustsec = P::get_rustsec(&configuration);
        let service_deps_checker = P::get_deps_checker(configuration.clone(), service_index.clone(), service_rustsec.clone());
        let service_email_sender = P::get_email_sender(configuration.clone());
        let service_docs_generator = P::get_docs_generator(
            configuration.clone(),
            service_db_pool.clone(),
            service_storage.clone(),
            worker_nodes.clone(),
        );

        // check undocumented packages
        let default_target = &configuration.self_toolchain_host;
        let job_specs = db_transaction_write(
            &service_db_pool,
            "Application::launch::get_undocumented_crates",
            |database| async move {
                let jobs = database.get_undocumented_crates(default_target).await?;
                for job in &jobs {
                    // resolve the docs
                    database
                        .set_crate_documentation(&job.package, &job.version, &job.target, false, false)
                        .await?;
                }
                Ok::<_, ApiError>(jobs)
            },
        )
        .await?;
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
            worker_nodes,
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
    #[must_use]
    pub fn get_service_storage(&self) -> Arc<dyn Storage + Send + Sync> {
        self.service_storage.clone()
    }

    /// Gets the index service
    #[must_use]
    pub fn get_service_index(&self) -> &(dyn Index + Send + Sync) {
        self.service_index.as_ref()
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
        #[derive(Debug, Error)]
        enum EventHandlerError {
            #[error("failed to update token last usage")]
            UpdateTokenUsage(#[source] sqlx::Error),
            #[error("failed to increment crate version download count")]
            CrateDownload(#[source] DepsError),
        }
        impl AsStatusCode for EventHandlerError {
            fn status_code(&self) -> StatusCode {
                match self {
                    Self::UpdateTokenUsage(_) => StatusCode::INTERNAL_SERVER_ERROR,
                    Self::CrateDownload(deps_error) => deps_error.status_code(),
                }
            }
        }

        self.db_transaction_write("events_handler_handle", |app| async move {
            for event in events {
                match event {
                    AppEvent::TokenUse(usage) => {
                        app.database
                            .update_token_last_usage(usage)
                            .await
                            .map_err(EventHandlerError::UpdateTokenUsage)?;
                    }
                    AppEvent::CrateDownload(CrateVersion { package: name, version }) => {
                        app.database
                            .increment_crate_version_dl_count(name, version)
                            .await
                            .map_err(EventHandlerError::CrateDownload)?;
                    }
                }
            }
            Ok::<_, ApiError>(())
        })
        .await
    }

    /// Executes a piece of work in the context of a transaction
    /// The transaction is committed if the operation succeed,
    /// or rolled back if it fails
    ///
    /// # Errors
    ///
    /// Returns an instance of the `E` type argument
    pub(crate) async fn db_transaction_read<'s, F, FUT, T, E>(&'s self, workload: F) -> Result<T, E>
    where
        F: FnOnce(ApplicationWithTransaction<'s>) -> FUT,
        FUT: Future<Output = Result<T, E>>,
        E: From<sqlx::Error>,
    {
        db_transaction_read(&self.service_db_pool, |database| async move {
            workload(ApplicationWithTransaction {
                database,
                application: self,
            })
            .await
        })
        .await
    }

    /// Executes a piece of work in the context of a transaction
    /// The transaction is committed if the operation succeed,
    /// or rolled back if it fails
    ///
    /// # Errors
    ///
    /// Returns an instance of the `E` type argument
    pub(crate) async fn db_transaction_write<'s, F, FUT, T, E>(&'s self, operation: &'static str, workload: F) -> Result<T, E>
    where
        F: FnOnce(ApplicationWithTransaction<'s>) -> FUT,
        FUT: Future<Output = Result<T, E>>,
        E: From<sqlx::Error>,
    {
        db_transaction_write(&self.service_db_pool, operation, |database| async move {
            workload(ApplicationWithTransaction {
                database,
                application: self,
            })
            .await
        })
        .await
    }

    /// Attempts the authentication of a user
    pub async fn authenticate(&self, auth_data: &AuthData) -> Result<Authentication, ApiError> {
        self.db_transaction_read(|app| async move { app.authenticate(auth_data).await })
            .await
    }

    /// Gets the registry configuration
    pub async fn get_registry_information(&self, auth_data: &AuthData) -> Result<RegistryInformation, ApiError> {
        let _authentication = self.authenticate(auth_data).await?;
        Ok(RegistryInformation {
            registry_name: self.configuration.self_local_name.clone(),
            toolchain_host: self.configuration.self_toolchain_host.clone(),
            toolchain_version_stable: self.configuration.self_toolchain_version_stable.clone(),
            toolchain_version_nightly: self.configuration.self_toolchain_version_nightly.clone(),
            toolchain_targets: self.configuration.self_known_targets.clone(),
        })
    }

    /// Gets the connected worker nodes
    pub async fn get_workers(&self, auth_data: &AuthData) -> Result<Vec<WorkerPublicData>, ApiError> {
        let authentication = self.authenticate(auth_data).await?;
        if !authentication.can_admin {
            return Err(error_forbidden());
        }
        Ok(self.worker_nodes.get_workers())
    }

    /// Adds a listener to workers updates
    pub async fn get_workers_updates(&self, auth_data: &AuthData) -> Result<Receiver<WorkerEvent>, ApiError> {
        let authentication = self.authenticate(auth_data).await?;
        if !authentication.can_admin {
            return Err(error_forbidden());
        }
        let (sender, receiver) = channel(16);
        self.worker_nodes.add_listener(sender).await;
        Ok(receiver)
    }

    /// Gets the data about the current user
    pub async fn get_current_user(&self, auth_data: &AuthData) -> Result<RegistryUser, ApiError> {
        self.db_transaction_read(|app| async move {
            let authentication = app.authenticate(auth_data).await?;
            app.database.get_user_profile(authentication.uid()?).await.map_err(Into::into)
        })
        .await
    }

    /// Attempts to login using an OAuth code
    pub async fn login_with_oauth_code(&self, code: &str) -> Result<RegistryUser, ApiError> {
        self.db_transaction_write("login_with_oauth_code", |app| async move {
            app.database
                .login_with_oauth_code(&self.configuration, code)
                .await
                .map_err(ApiError::from)
        })
        .await
    }

    /// Gets the known users
    pub async fn get_users(&self, auth_data: &AuthData) -> Result<Vec<RegistryUser>, ApiError> {
        self.db_transaction_read(|app| async move {
            let authentication = app.authenticate(auth_data).await?;
            app.check_can_admin_registry(&authentication).await?;
            app.database.get_users().await
        })
        .await
    }

    /// Updates the information of a user
    pub async fn update_user(&self, auth_data: &AuthData, target: &RegistryUser) -> Result<RegistryUser, ApiError> {
        self.db_transaction_write("update_user", |app| async move {
            let authentication = app.authenticate(auth_data).await?;
            let principal_uid = authentication.uid()?;
            let can_admin = if target.id == principal_uid {
                // same user
                authentication.can_admin && app.database.get_is_admin(principal_uid).await?
            } else {
                // different users, requires admin
                app.check_can_admin_registry(&authentication).await?;
                true
            };
            app.database
                .update_user(principal_uid, target, can_admin)
                .await
                .map_err(ApiError::from)
        })
        .await
    }

    /// Attempts to deactivate a user
    pub async fn deactivate_user(&self, auth_data: &AuthData, target: &str) -> Result<(), ApiError> {
        self.db_transaction_write("deactivate_user", |app| async move {
            let authentication = app.authenticate(auth_data).await?;
            let principal_uid = app.check_can_admin_registry(&authentication).await?;
            app.database.deactivate_user(principal_uid, target).await
        })
        .await
    }

    /// Attempts to re-activate a user
    pub async fn reactivate_user(&self, auth_data: &AuthData, target: &str) -> Result<(), ApiError> {
        self.db_transaction_write("reactivate_user", |app| async move {
            let authentication = app.authenticate(auth_data).await?;
            app.check_can_admin_registry(&authentication).await?;
            app.database.reactivate_user(target).await
        })
        .await
    }

    /// Attempts to delete a user
    pub async fn delete_user(&self, auth_data: &AuthData, target: &str) -> Result<(), ApiError> {
        self.db_transaction_write("delete_user", |app| async move {
            let authentication = app.authenticate(auth_data).await?;
            let principal_uid = app.check_can_admin_registry(&authentication).await?;
            app.database.delete_user(principal_uid, target).await
        })
        .await
    }

    /// Gets the tokens for a user
    pub async fn get_tokens(&self, auth_data: &AuthData) -> Result<Vec<RegistryUserToken>, ApiError> {
        self.db_transaction_read(|app| async move {
            let authentication = app.authenticate(auth_data).await?;
            authentication.check_can_admin()?;
            app.database.get_tokens(authentication.uid()?).await
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
        self.db_transaction_write("create_token", |app| async move {
            let authentication = app.authenticate(auth_data).await?;
            authentication.check_can_admin()?;
            app.database
                .create_token(authentication.uid()?, name, can_write, can_admin)
                .await
        })
        .await
    }

    /// Revoke a previous token
    pub async fn revoke_token(&self, auth_data: &AuthData, token_id: i64) -> Result<(), ApiError> {
        self.db_transaction_write("revoke_token", |app| async move {
            let authentication = app.authenticate(auth_data).await?;
            authentication.check_can_admin()?;
            app.database.revoke_token(authentication.uid()?, token_id).await
        })
        .await
    }

    /// Gets the global tokens for the registry, usually for CI purposes
    pub async fn get_global_tokens(&self, auth_data: &AuthData) -> Result<Vec<RegistryUserToken>, ApiError> {
        self.db_transaction_read(|app| async move {
            let authentication = app.authenticate(auth_data).await?;
            app.check_can_admin_registry(&authentication).await?;
            app.database.get_global_tokens().await
        })
        .await
    }

    /// Creates a global token for the registry
    pub async fn create_global_token(&self, auth_data: &AuthData, name: &str) -> Result<RegistryUserTokenWithSecret, ApiError> {
        self.db_transaction_write("create_global_token", |app| async move {
            let authentication = app.authenticate(auth_data).await?;
            app.check_can_admin_registry(&authentication).await?;
            app.database.create_global_token(name).await.map_err(ApiError::from)
        })
        .await
    }

    /// Revokes a global token for the registry
    pub async fn revoke_global_token(&self, auth_data: &AuthData, token_id: i64) -> Result<(), ApiError> {
        self.db_transaction_write("revoke_global_token", |app| async move {
            let authentication = app.authenticate(auth_data).await?;
            app.check_can_admin_registry(&authentication).await?;
            app.database.revoke_global_token(token_id).await.map_err(ApiError::from)
        })
        .await
    }

    /// Publish a crate
    pub async fn publish_crate_version(&self, auth_data: &AuthData, content: &[u8]) -> Result<CrateUploadResult, ApiError> {
        // deserialize payload
        let package = CrateUploadData::new(content)?;
        let index_data = package.build_index_data();

        let (user, result, targets, capabilities) = {
            let package = &package;
            self.db_transaction_write("publish_crate_version", |app| async move {
                let authentication = app.authenticate(auth_data).await?;
                authentication.check_can_write()?;
                let user = app.database.get_user_profile(authentication.uid()?).await?;
                // publish
                let result = app.database.publish_crate_version(user.id, package).await?;
                let mut targets = app.database.get_crate_targets(&package.metadata.name).await?;
                if targets.is_empty() {
                    targets.push(CrateInfoTarget {
                        target: self.configuration.self_toolchain_host.clone(),
                        docs_use_native: true,
                    });
                }
                for info in &targets {
                    app.database
                        .set_crate_documentation(&package.metadata.name, &package.metadata.vers, &info.target, false, false)
                        .await?;
                }
                let capabilities = app.database.get_crate_required_capabilities(&package.metadata.name).await?;
                Ok::<_, ApiError>((user, result, targets, capabilities))
            })
            .await
        }?;

        self.service_storage.store_crate(&package.metadata, package.content).await?;
        self.service_index.publish_crate_version(&index_data).await?;
        for info in targets {
            self.service_docs_generator
                .queue(
                    &DocGenJobSpec {
                        package: index_data.name.clone(),
                        version: index_data.vers.clone(),
                        target: info.target,
                        use_native: info.docs_use_native,
                        capabilities: capabilities.clone(),
                    },
                    &DocGenTrigger::Upload { by: user.clone() },
                )
                .await?;
        }
        Ok(result)
    }

    /// Gets all the data about a crate
    pub async fn get_crate_info(&self, auth_data: &AuthData, package: &str) -> Result<CrateInfo, ApiError> {
        let info = self
            .db_transaction_read(|app| async move {
                let _authentication = app.authenticate(auth_data).await?;
                app.database
                    .get_crate_info(package, self.service_index.get_crate_data(package).await?)
                    .await
            })
            .await?;
        let metadata = self
            .service_storage
            .download_crate_metadata(package, &info.versions.last().unwrap().index.vers)
            .await?;
        Ok(CrateInfo { metadata, ..info })
    }

    /// Downloads the last README for a crate
    pub async fn get_crate_last_readme(&self, auth_data: &AuthData, package: &str) -> Result<Vec<u8>, ApiError> {
        let version = self
            .db_transaction_read(|app| async move {
                let _authentication = app.authenticate(auth_data).await?;
                let version = app.database.get_crate_last_version(package).await?;
                Ok::<_, ApiError>(version)
            })
            .await?;
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
        let public_read = self.configuration.self_public_read;
        self.db_transaction_read(|app| async move {
            if !public_read {
                let _authentication = app.authenticate(auth_data).await?;
            }
            app.database.check_crate_exists(package, version).await?;
            Ok::<_, ApiError>(())
        })
        .await?;
        let content = self.service_storage.download_crate(package, version).await?;
        self.app_events_sender
            .send(AppEvent::CrateDownload(CrateVersion {
                package: package.to_string(),
                version: version.to_string(),
            }))
            .await?;
        Ok(content)
    }

    /// Completely removes a version from the registry
    pub async fn remove_crate_version(&self, auth_data: &AuthData, package: &str, version: &str) -> Result<(), ApiError> {
        self.db_transaction_write("remove_crate_version", |app| async move {
            let authentication = app.authenticate(auth_data).await?;
            app.check_can_manage_crate(&authentication, package).await?;
            app.database.remove_crate_version(package, version).await?;
            self.service_index.remove_crate_version(package, version).await?;
            Ok(())
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
        self.db_transaction_write("yank_crate_version", |app| async move {
            let authentication = app.authenticate(auth_data).await?;
            app.check_can_manage_crate(&authentication, package).await?;
            app.database.yank_crate_version(package, version).await
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
        self.db_transaction_write("unyank_crate_version", |app| async move {
            let authentication = app.authenticate(auth_data).await?;
            app.check_can_manage_crate(&authentication, package).await?;
            app.database.unyank_crate_version(package, version).await
        })
        .await
    }

    /// Gets the packages that need documentation generation
    pub async fn get_undocumented_crates(&self, auth_data: &AuthData) -> Result<Vec<DocGenJobSpec>, ApiError> {
        self.db_transaction_read(|app| async move {
            let _authentication = app.authenticate(auth_data).await?;
            app.database
                .get_undocumented_crates(&self.configuration.self_toolchain_host)
                .await
                .map_err(ApiError::from)
        })
        .await
    }

    /// Gets the documentation jobs
    pub async fn get_doc_gen_jobs(&self, auth_data: &AuthData) -> Result<Vec<DocGenJob>, ApiError> {
        let _authentication = self.authenticate(auth_data).await?;
        self.service_docs_generator.get_jobs().await
    }

    /// Gets the log for a documentation generation job
    pub async fn get_doc_gen_job_log(&self, auth_data: &AuthData, job_id: i64) -> Result<String, ApiError> {
        let _authentication = self.authenticate(auth_data).await?;
        self.service_docs_generator.get_job_log(job_id).await
    }

    /// Adds a listener to job updates
    pub async fn get_doc_gen_job_updates(&self, auth_data: &AuthData) -> Result<Receiver<DocGenEvent>, ApiError> {
        let _authentication = self.authenticate(auth_data).await?;
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
    ) -> Result<Vec<DocGenJob>, ApiError> {
        let (user, targets, capabilities) = self
            .db_transaction_write("regen_crate_version_doc", |app| async move {
                let authentication = app.authenticate(auth_data).await?;
                let principal_uid = app.check_can_manage_crate(&authentication, package).await?;
                let user = app.database.get_user_profile(principal_uid).await?;
                let targets = app
                    .database
                    .regen_crate_version_doc(package, version, &self.configuration.self_toolchain_host)
                    .await?;
                let capabilities = app.database.get_crate_required_capabilities(package).await?;
                Ok::<_, ApiError>((user, targets, capabilities))
            })
            .await?;

        let mut jobs = Vec::new();
        for info in targets {
            jobs.push(
                self.service_docs_generator
                    .queue(
                        &DocGenJobSpec {
                            package: package.to_string(),
                            version: version.to_string(),
                            target: info.target,
                            use_native: info.docs_use_native,
                            capabilities: capabilities.clone(),
                        },
                        &DocGenTrigger::Manual { by: user.clone() },
                    )
                    .await?,
            );
        }
        Ok(jobs)
    }

    /// Gets all the packages that are outdated while also being the latest version
    pub async fn get_crates_outdated_heads(&self, auth_data: &AuthData) -> Result<Vec<CrateVersion>, ApiError> {
        self.db_transaction_read(|app| async move {
            let _authentication = app.authenticate(auth_data).await?;
            app.database.get_crates_outdated_heads().await
        })
        .await
    }

    /// Gets the download statistics for a crate
    pub async fn get_crate_dl_stats(&self, auth_data: &AuthData, package: &str) -> Result<DownloadStats, ApiError> {
        self.db_transaction_read(|app| async move {
            let _authentication = app.authenticate(auth_data).await?;
            app.database.get_crate_dl_stats(package).await
        })
        .await
    }

    /// Gets the list of owners for a package
    pub async fn get_crate_owners(&self, auth_data: &AuthData, package: &str) -> Result<OwnersQueryResult, ApiError> {
        let public_read = self.configuration.self_public_read;
        self.db_transaction_read(|app| async move {
            if !public_read {
                let _authentication = app.authenticate(auth_data).await?;
            }
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
        self.db_transaction_write("add_crate_owners", |app| async move {
            let authentication = app.authenticate(auth_data).await?;
            app.check_can_manage_crate(&authentication, package).await?;
            app.database.add_crate_owners(package, new_users).await
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
        self.db_transaction_write("remove_crate_owners", |app| async move {
            let authentication = app.authenticate(auth_data).await?;
            app.check_can_manage_crate(&authentication, package).await?;
            app.database.remove_crate_owners(package, old_users).await
        })
        .await
    }

    /// Gets the targets for a crate
    pub async fn get_crate_targets(&self, auth_data: &AuthData, package: &str) -> Result<Vec<CrateInfoTarget>, ApiError> {
        self.db_transaction_read(|app| async move {
            let _authentication = app.authenticate(auth_data).await?;
            app.database.get_crate_targets(package).await
        })
        .await
    }

    /// Sets the targets for a crate
    pub async fn set_crate_targets(
        &self,
        auth_data: &AuthData,
        package: &str,
        targets: &[CrateInfoTarget],
    ) -> Result<(), ApiError> {
        let (user, jobs) = self
            .db_transaction_write("set_crate_targets", |app| async move {
                let authentication = app.authenticate(auth_data).await?;
                let principal_uid = app.check_can_manage_crate(&authentication, package).await?;
                let user = app.database.get_user_profile(principal_uid).await?;
                for info in targets {
                    if !self.configuration.self_known_targets.contains(&info.target) {
                        return Err(specialize(
                            error_invalid_request(),
                            format!("Unknown target: {}", info.target),
                        ));
                    }
                }
                let jobs = app.database.set_crate_targets(package, targets).await?;
                for job in &jobs {
                    app.database
                        .set_crate_documentation(&job.package, &job.version, &job.target, false, false)
                        .await?;
                }
                Ok::<_, ApiError>((user, jobs))
            })
            .await?;
        for job in jobs {
            self.service_docs_generator
                .queue(&job, &DocGenTrigger::NewTarget { by: user.clone() })
                .await?;
        }
        Ok(())
    }

    /// Gets the required capabilities for a crate
    pub async fn get_crate_required_capabilities(&self, auth_data: &AuthData, package: &str) -> Result<Vec<String>, ApiError> {
        self.db_transaction_read(|app| async move {
            let _authentication = app.authenticate(auth_data).await?;
            app.database.get_crate_required_capabilities(package).await
        })
        .await
    }

    /// Sets the required capabilities for a crate
    pub async fn set_crate_required_capabilities(
        &self,
        auth_data: &AuthData,
        package: &str,
        capabilities: &[String],
    ) -> Result<(), ApiError> {
        self.db_transaction_write("set_crate_required_capabilities", |app| async move {
            let authentication = app.authenticate(auth_data).await?;
            let _ = app.check_can_manage_crate(&authentication, package).await?;
            app.database.set_crate_required_capabilities(package, capabilities).await?;
            Ok::<_, ApiError>(())
        })
        .await?;
        Ok(())
    }

    /// Sets the deprecation status on a crate
    pub async fn set_crate_deprecation(&self, auth_data: &AuthData, package: &str, deprecated: bool) -> Result<(), ApiError> {
        self.db_transaction_write("set_crate_deprecation", |app| async move {
            let authentication = app.authenticate(auth_data).await?;
            app.check_can_manage_crate(&authentication, package).await?;
            app.database.set_crate_deprecation(package, deprecated).await
        })
        .await
    }

    /// Sets whether a crate can have versions completely removed
    pub async fn set_crate_can_remove(&self, auth_data: &AuthData, package: &str, can_remove: bool) -> Result<(), ApiError> {
        self.db_transaction_write("set_crate_can_remove", |app| async move {
            let authentication = app.authenticate(auth_data).await?;
            app.check_can_manage_crate(&authentication, package).await?;
            app.database.set_crate_can_remove(package, can_remove).await
        })
        .await
    }

    /// Gets the global statistics for the registry
    pub async fn get_crates_stats(&self, auth_data: &AuthData) -> Result<GlobalStats, ApiError> {
        self.db_transaction_read(|app| async move {
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
        deprecated: Option<bool>,
    ) -> Result<SearchResults, ApiError> {
        let public_read = self.configuration.self_public_read;
        self.db_transaction_read(|app| async move {
            if !public_read {
                let _authentication = app.authenticate(auth_data).await?;
            }
            app.database.search_crates(query, per_page, deprecated).await
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
        let targets = self
            .db_transaction_read(|app| async move {
                let _authentication = app.authenticate(auth_data).await?;
                app.database.check_crate_exists(package, version).await?;
                app.database.get_crate_targets(package).await
            })
            .await?;
        let targets = targets.into_iter().map(|info| info.target).collect::<Vec<_>>();
        self.service_deps_checker.check_crate(package, version, &targets).await
    }
}

#[derive(Debug, Error)]
pub enum AuthenticationError {
    #[error("missing cookie")]
    CookieMissing,

    #[error("failed to deserialize cookie")]
    CookieDeserialization(serde_json::Error),

    #[error("user is not authenticated.")]
    Unauthorized,

    #[error("access is forbidden for user")]
    Forbidden,

    #[error("administration is forbidden for this authentication")]
    AdministrationIsForbidden,

    #[error("writing is forbidden for this authentication")]
    WriteIsForbidden,

    #[error("failed to check global token")]
    GlobalToken(#[source] sqlx::Error),

    #[error("failed to check user token")]
    UserToken(#[source] sqlx::Error),

    #[error("failed to check user token")]
    CheckUser(#[source] sqlx::Error),

    #[error("failed to check user roles")]
    CheckRoles(#[source] sqlx::Error),

    #[error("expected a user to be authenticated")]
    NoUserAuthenticated,
}

impl AsStatusCode for AuthenticationError {
    fn status_code(&self) -> StatusCode {
        match self {
            Self::Unauthorized | Self::CookieMissing => StatusCode::UNAUTHORIZED,
            Self::CookieDeserialization(_)
            | Self::GlobalToken(_)
            | Self::UserToken(_)
            | Self::CheckUser(_)
            | Self::CheckRoles(_) => StatusCode::INTERNAL_SERVER_ERROR,
            Self::NoUserAuthenticated => StatusCode::BAD_REQUEST,
            Self::Forbidden | Self::AdministrationIsForbidden | Self::WriteIsForbidden => StatusCode::FORBIDDEN,
        }
    }
}

#[derive(Debug, Error)]
#[error("the current user can't administrate registry")]
struct CanAdminRegistryError(#[from] AuthenticationError);
impl AsStatusCode for CanAdminRegistryError {
    fn status_code(&self) -> StatusCode {
        self.0.status_code()
    }
}

/// The application, running with a transaction
pub(crate) struct ApplicationWithTransaction<'a> {
    /// The application with its services
    pub(crate) application: &'a Application,
    /// The database access encapsulating a transaction
    pub(crate) database: Database,
}

impl ApplicationWithTransaction<'_> {
    /// Attempts the authentication of a user
    async fn authenticate(&self, auth_data: &AuthData) -> Result<Authentication, ApiError> {
        if let Some(token) = &auth_data.token {
            self.authenticate_token(token).await.map_err(ApiError::from)
        } else {
            let authentication = auth_data
                .try_authenticate_cookie()
                .map_err(AuthenticationError::CookieDeserialization)?
                .ok_or_else(|| AuthenticationError::CookieMissing)?;
            let email = authentication.email().map_err(ApiError::from)?;
            self.database.check_is_user(email).await?;
            Ok(authentication)
        }
    }

    /// Tries to authenticate using a token
    async fn authenticate_token(&self, token: &Token) -> Result<Authentication, AuthenticationError> {
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

    /// Checks that the given authentication can perform admin tasks
    async fn check_can_admin_registry(&self, authentication: &Authentication) -> Result<i64, CanAdminRegistryError> {
        authentication.check_can_admin()?;
        let principal_uid = authentication.uid()?;
        self.database.check_is_admin(principal_uid).await?;
        Ok(principal_uid)
    }

    /// Checks that the given authentication can manage a given crate
    async fn check_can_manage_crate(&self, authentication: &Authentication, package: &str) -> Result<i64, ApiError> {
        authentication.check_can_write()?;
        let principal_uid = authentication.uid()?;
        self.database.check_is_crate_manager(principal_uid, package).await?;
        Ok(principal_uid)
    }
}
