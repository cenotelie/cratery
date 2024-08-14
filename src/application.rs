/*******************************************************************************
 * Copyright (c) 2024 Cénotélie Opérations SAS (cenotelie.fr)
 ******************************************************************************/

//! Main application

use std::sync::Arc;

use log::info;
use sqlx::sqlite::SqlitePoolOptions;
use sqlx::{Pool, Sqlite};

use crate::model::auth::{AuthenticatedUser, RegistryUserToken, RegistryUserTokenWithSecret};
use crate::model::cargo::{
    CrateUploadData, CrateUploadResult, OwnersQueryResult, RegistryUser, SearchResults, YesNoMsgResult, YesNoResult,
};
use crate::model::config::Configuration;
use crate::model::deps::DepsAnalysis;
use crate::model::packages::CrateInfo;
use crate::model::stats::{DownloadStats, GlobalStats};
use crate::model::{CrateAndVersion, JobCrate};
use crate::services::database::Database;
use crate::services::deps::DepsChecker;
use crate::services::docs::DocsGenerator;
use crate::services::emails::EmailSender;
use crate::services::index::Index;
use crate::services::rustsec::RustSecChecker;
use crate::services::storage::Storage;
use crate::utils::apierror::{error_invalid_request, error_unauthorized, specialize, ApiError};
use crate::utils::axum::auth::{AuthData, Token};
use crate::utils::db::{in_transaction, AppTransaction};

/// The state of this application for axum
pub struct Application {
    /// The configuration
    pub configuration: Arc<Configuration>,
    /// The database pool
    service_db_pool: Pool<Sqlite>,
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
}

/// The empty database
const DB_EMPTY: &[u8] = include_bytes!("empty.db");
/// Maximum number of concurrent connections
const DB_MAX_CONNECTIONS: u32 = 16;

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
        let service_db_pool = SqlitePoolOptions::new()
            .max_connections(DB_MAX_CONNECTIONS)
            .connect_lazy(&configuration.get_database_url())?;
        // migrate the database, if appropriate
        crate::migrations::migrate_to_last(&mut *service_db_pool.acquire().await?).await?;

        let service_storage = crate::services::storage::get_storage(&configuration);
        let service_index = crate::services::index::get_index(&configuration).await?;
        let service_rustsec = crate::services::rustsec::get_rustsec(&configuration);
        let service_deps_checker =
            crate::services::deps::get_deps_checker(configuration.clone(), service_index.clone(), service_rustsec.clone());
        let service_email_sender = crate::services::emails::get_deps_checker(configuration.clone());
        let service_docs_generator =
            crate::services::docs::get_docs_generator(configuration.clone(), service_db_pool.clone(), service_storage.clone());

        // check undocumented packages
        {
            let service_docs_generator = service_docs_generator.clone();
            let mut connection = service_db_pool.acquire().await?;
            in_transaction(&mut connection, |transaction| async move {
                let app = Database::new(transaction);
                let jobs = app.get_undocumented_crates().await?;
                for job in jobs {
                    service_docs_generator.queue(job)?;
                }
                Ok::<_, ApiError>(())
            })
            .await?;
        }

        // deps worker
        crate::services::deps::create_deps_worker(
            configuration.clone(),
            service_deps_checker.clone(),
            service_email_sender.clone(),
            service_db_pool.clone(),
        );

        Ok(Arc::new(Self {
            configuration,
            service_db_pool,
            service_storage,
            service_index,
            service_rustsec,
            service_deps_checker,
            service_email_sender,
            service_docs_generator,
        }))
    }

    /// Gets the storage service
    pub fn get_service_storage(&self) -> &(dyn Storage + Send + Sync) {
        self.service_storage.as_ref()
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

    /// Attempts the authentication of a user
    pub async fn authenticate(&self, auth_data: &AuthData) -> Result<AuthenticatedUser, ApiError> {
        let mut connection = self.service_db_pool.acquire().await?;
        in_transaction(&mut connection, |transaction| async move {
            self.with_transaction(transaction).authenticate(auth_data).await
        })
        .await
    }

    /// Gets the data about the current user
    pub async fn get_current_user(&self, auth_data: &AuthData) -> Result<RegistryUser, ApiError> {
        let mut connection = self.service_db_pool.acquire().await?;
        in_transaction(&mut connection, |transaction| async move {
            let app = self.with_transaction(transaction);
            let principal = app.authenticate(auth_data).await?;
            app.database.get_current_user(&principal).await
        })
        .await
    }

    /// Attempts to login using an OAuth code
    pub async fn login_with_oauth_code(&self, code: &str) -> Result<RegistryUser, ApiError> {
        let mut connection = self.service_db_pool.acquire().await?;
        in_transaction(&mut connection, |transaction| async move {
            let app = self.with_transaction(transaction);
            app.database.login_with_oauth_code(&self.configuration, code).await
        })
        .await
    }

    /// Gets the known users
    pub async fn get_users(&self, auth_data: &AuthData) -> Result<Vec<RegistryUser>, ApiError> {
        let mut connection = self.service_db_pool.acquire().await?;
        in_transaction(&mut connection, |transaction| async move {
            let app = self.with_transaction(transaction);
            let principal = app.authenticate(auth_data).await?;
            app.database.get_users(&principal).await
        })
        .await
    }

    /// Updates the information of a user
    pub async fn update_user(&self, auth_data: &AuthData, target: &RegistryUser) -> Result<RegistryUser, ApiError> {
        let mut connection = self.service_db_pool.acquire().await?;
        in_transaction(&mut connection, |transaction| async move {
            let app = self.with_transaction(transaction);
            let principal = app.authenticate(auth_data).await?;
            app.database.update_user(&principal, target).await
        })
        .await
    }

    /// Attempts to deactivate a user
    pub async fn deactivate_user(&self, auth_data: &AuthData, target: &str) -> Result<(), ApiError> {
        let mut connection = self.service_db_pool.acquire().await?;
        in_transaction(&mut connection, |transaction| async move {
            let app = self.with_transaction(transaction);
            let principal = app.authenticate(auth_data).await?;
            app.database.deactivate_user(&principal, target).await
        })
        .await
    }

    /// Attempts to re-activate a user
    pub async fn reactivate_user(&self, auth_data: &AuthData, target: &str) -> Result<(), ApiError> {
        let mut connection = self.service_db_pool.acquire().await?;
        in_transaction(&mut connection, |transaction| async move {
            let app = self.with_transaction(transaction);
            let principal = app.authenticate(auth_data).await?;
            app.database.reactivate_user(&principal, target).await
        })
        .await
    }

    /// Attempts to delete a user
    pub async fn delete_user(&self, auth_data: &AuthData, target: &str) -> Result<(), ApiError> {
        let mut connection = self.service_db_pool.acquire().await?;
        in_transaction(&mut connection, |transaction| async move {
            let app = self.with_transaction(transaction);
            let principal = app.authenticate(auth_data).await?;
            app.database.delete_user(&principal, target).await
        })
        .await
    }

    /// Gets the tokens for a user
    pub async fn get_tokens(&self, auth_data: &AuthData) -> Result<Vec<RegistryUserToken>, ApiError> {
        let mut connection = self.service_db_pool.acquire().await?;
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
        let mut connection = self.service_db_pool.acquire().await?;
        in_transaction(&mut connection, |transaction| async move {
            let app = self.with_transaction(transaction);
            let principal = app.authenticate(auth_data).await?;
            app.database.create_token(&principal, name, can_write, can_admin).await
        })
        .await
    }

    /// Revoke a previous token
    pub async fn revoke_token(&self, auth_data: &AuthData, token_id: i64) -> Result<(), ApiError> {
        let mut connection = self.service_db_pool.acquire().await?;
        in_transaction(&mut connection, |transaction| async move {
            let app = self.with_transaction(transaction);
            let principal = app.authenticate(auth_data).await?;
            app.database.revoke_token(&principal, token_id).await
        })
        .await
    }

    /// Publish a crate
    pub async fn publish_crate_version(&self, auth_data: &AuthData, content: &[u8]) -> Result<CrateUploadResult, ApiError> {
        let mut connection = self.service_db_pool.acquire().await?;
        in_transaction(&mut connection, |transaction| async move {
            let app = self.with_transaction(transaction);
            let principal = app.authenticate(auth_data).await?;
            // deserialize payload
            let package = CrateUploadData::new(content)?;
            let index_data = package.build_index_data();
            // publish
            let r = app.database.publish_crate_version(&principal, &package).await?;
            self.service_storage.store_crate(&package.metadata, package.content).await?;
            self.service_index.publish_crate_version(&index_data).await?;
            let targets = app.database.get_crate_targets(&package.metadata.name).await?;
            // generate the doc
            self.service_docs_generator.queue(JobCrate {
                name: package.metadata.name.clone(),
                version: package.metadata.vers.clone(),
                targets,
            })?;
            Ok(r)
        })
        .await
    }

    /// Gets all the data about a crate
    pub async fn get_crate_info(&self, auth_data: &AuthData, package: &str) -> Result<CrateInfo, ApiError> {
        let mut connection = self.service_db_pool.acquire().await?;
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
        let mut connection = self.service_db_pool.acquire().await?;
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
        let mut connection = self.service_db_pool.acquire().await?;
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
        let mut connection = self.service_db_pool.acquire().await?;
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
        let mut connection = self.service_db_pool.acquire().await?;
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
        let mut connection = self.service_db_pool.acquire().await?;
        in_transaction(&mut connection, |transaction| async move {
            let app = self.with_transaction(transaction);
            let principal = app.authenticate(auth_data).await?;
            app.database.unyank_crate_version(&principal, package, version).await
        })
        .await
    }

    /// Force the re-generation for the documentation of a package
    pub async fn regen_crate_version_doc(&self, auth_data: &AuthData, package: &str, version: &str) -> Result<(), ApiError> {
        let mut connection: sqlx::pool::PoolConnection<Sqlite> = self.service_db_pool.acquire().await?;
        in_transaction(&mut connection, |transaction| async move {
            let app = self.with_transaction(transaction);
            let principal = app.authenticate(auth_data).await?;
            app.database.regen_crate_version_doc(&principal, package, version).await?;
            let targets = app.database.get_crate_targets(package).await?;
            self.service_docs_generator.queue(JobCrate {
                name: package.to_string(),
                version: version.to_string(),
                targets,
            })?;
            Ok(())
        })
        .await
    }

    /// Gets all the packages that are outdated while also being the latest version
    pub async fn get_crates_outdated_heads(&self, auth_data: &AuthData) -> Result<Vec<CrateAndVersion>, ApiError> {
        let mut connection: sqlx::pool::PoolConnection<Sqlite> = self.service_db_pool.acquire().await?;
        in_transaction(&mut connection, |transaction| async move {
            let app = self.with_transaction(transaction);
            let _principal = app.authenticate(auth_data).await?;
            app.database.get_crates_outdated_heads().await
        })
        .await
    }

    /// Gets the download statistics for a crate
    pub async fn get_crate_dl_stats(&self, auth_data: &AuthData, package: &str) -> Result<DownloadStats, ApiError> {
        let mut connection: sqlx::pool::PoolConnection<Sqlite> = self.service_db_pool.acquire().await?;
        in_transaction(&mut connection, |transaction| async move {
            let app = self.with_transaction(transaction);
            let _principal = app.authenticate(auth_data).await?;
            app.database.get_crate_dl_stats(package).await
        })
        .await
    }

    /// Gets the list of owners for a package
    pub async fn get_crate_owners(&self, auth_data: &AuthData, package: &str) -> Result<OwnersQueryResult, ApiError> {
        let mut connection: sqlx::pool::PoolConnection<Sqlite> = self.service_db_pool.acquire().await?;
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
        let mut connection: sqlx::pool::PoolConnection<Sqlite> = self.service_db_pool.acquire().await?;
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
        let mut connection: sqlx::pool::PoolConnection<Sqlite> = self.service_db_pool.acquire().await?;
        in_transaction(&mut connection, |transaction| async move {
            let app = self.with_transaction(transaction);
            let principal = app.authenticate(auth_data).await?;
            app.database.remove_crate_owners(&principal, package, old_users).await
        })
        .await
    }

    /// Gets the targets for a crate
    pub async fn get_crate_targets(&self, auth_data: &AuthData, package: &str) -> Result<Vec<String>, ApiError> {
        let mut connection: sqlx::pool::PoolConnection<Sqlite> = self.service_db_pool.acquire().await?;
        in_transaction(&mut connection, |transaction| async move {
            let app = self.with_transaction(transaction);
            let _principal = app.authenticate(auth_data).await?;
            app.database.get_crate_targets(package).await
        })
        .await
    }

    /// Sets the targets for a crate
    pub async fn set_crate_targets(&self, auth_data: &AuthData, package: &str, targets: &[String]) -> Result<(), ApiError> {
        let mut connection: sqlx::pool::PoolConnection<Sqlite> = self.service_db_pool.acquire().await?;
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
        let mut connection: sqlx::pool::PoolConnection<Sqlite> = self.service_db_pool.acquire().await?;
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
        let mut connection: sqlx::pool::PoolConnection<Sqlite> = self.service_db_pool.acquire().await?;
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
        let mut connection: sqlx::pool::PoolConnection<Sqlite> = self.service_db_pool.acquire().await?;
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
    pub async fn authenticate(&self, auth_data: &AuthData) -> Result<AuthenticatedUser, ApiError> {
        if let Some(token) = &auth_data.token {
            self.authenticate_token(token).await
        } else {
            let authenticated_user = auth_data.try_authenticate_cookie()?.ok_or_else(error_unauthorized)?;
            self.database.check_is_user(&authenticated_user.principal).await?;
            Ok(authenticated_user)
        }
    }

    /// Tries to authenticate using a token
    pub async fn authenticate_token(&self, token: &Token) -> Result<AuthenticatedUser, ApiError> {
        if token.id == self.application.configuration.self_service_login
            && token.secret == self.application.configuration.self_service_token
        {
            // self authentication to read
            return Ok(AuthenticatedUser {
                uid: -1,
                principal: self.application.configuration.self_service_login.clone(),
                can_write: false,
                can_admin: false,
            });
        }
        let user = self.database.check_token(&token.id, &token.secret).await?;
        Ok(user)
    }
}
