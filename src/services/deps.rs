/*******************************************************************************
 * Copyright (c) 2024 Cénotélie Opérations SAS (cenotelie.fr)
 ******************************************************************************/

//! Service to fetch data about dependency crates

use std::collections::HashMap;
use std::fmt::Write;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use futures::lock::Mutex;
use log::{error, info};
use sqlx::{Pool, Sqlite};
use tokio::fs::File;
use tokio::io::AsyncBufReadExt;

use crate::model::cargo::{IndexCrateDependency, IndexCrateMetadata};
use crate::model::config::{Configuration, ExternalRegistryProtocol};
use crate::model::deps::{DepAdvisory, DepsAnalysis, DepsGraph, DepsGraphCrateOrigin, BUILTIN_CRATES_REGISTRY_URI};
use crate::model::JobCrate;
use crate::services::database::Database;
use crate::services::emails::EmailSender;
use crate::services::index::Index;
use crate::services::rustsec::RustSecChecker;
use crate::utils::apierror::{error_backend_failure, error_not_found, specialize, ApiError};
use crate::utils::db::in_transaction;
use crate::utils::{stale_instant, FaillibleFuture};

/// Creates a worker for the continuous check of dependencies for head crates
pub fn create_deps_worker(
    configuration: Arc<Configuration>,
    service_deps_checker: Arc<dyn DepsChecker + Send + Sync>,
    service_email_sender: Arc<dyn EmailSender + Send + Sync>,
    pool: Pool<Sqlite>,
) {
    let _handle = tokio::spawn({
        let service_deps_checker = service_deps_checker.clone();
        async move {
            info!("precaching crates.io index");
            if let Err(e) = service_deps_checker.precache_crate_io().await {
                error!("{e}");
                if let Some(backtrace) = &e.backtrace {
                    error!("{backtrace}");
                }
            }
        }
    });
    let _handle = tokio::spawn(async move {
        // every minute
        let mut interval = tokio::time::interval(Duration::from_secs(60));
        loop {
            let _instant = interval.tick().await;
            if let Err(e) = deps_worker_job(
                &configuration,
                service_deps_checker.clone(),
                service_email_sender.clone(),
                &pool,
            )
            .await
            {
                error!("{e}");
                if let Some(backtrace) = &e.backtrace {
                    error!("{backtrace}");
                }
            }
        }
    });
}

/// A job for the worker
async fn deps_worker_job(
    configuration: &Configuration,
    service_deps_checker: Arc<dyn DepsChecker + Send + Sync>,
    service_email_sender: Arc<dyn EmailSender + Send + Sync>,
    pool: &Pool<Sqlite>,
) -> Result<(), ApiError> {
    if configuration.deps_stale_analysis <= 0 {
        // deactivated
        return Ok(());
    }

    let jobs = {
        let mut connection = pool.acquire().await?;
        in_transaction(&mut connection, |transaction| async move {
            let database = Database::new(transaction);
            database.get_unanalyzed_crates(configuration.deps_stale_analysis).await
        })
        .await?
    };
    for job in jobs {
        deps_worker_job_on_crate_version(
            configuration,
            service_deps_checker.as_ref(),
            service_email_sender.as_ref(),
            pool,
            &job,
        )
        .await?;
    }
    Ok(())
}

async fn deps_worker_job_on_crate_version(
    configuration: &Configuration,
    service_deps_checker: &(dyn DepsChecker + Send + Sync),
    service_email_sender: &(dyn EmailSender + Send + Sync),
    pool: &Pool<Sqlite>,
    job: &JobCrate,
) -> Result<(), ApiError> {
    info!("checking deps for {} {}", job.name, job.version);
    let analysis = service_deps_checker
        .check_crate(&job.name, &job.version, &job.targets)
        .await?;
    let has_outdated = analysis.direct_dependencies.iter().any(|info| info.is_outdated);
    let has_cves = !analysis.advisories.is_empty();
    let (old_has_outdated, old_has_cves) = {
        let mut connection = pool.acquire().await?;
        in_transaction(&mut connection, |transaction| async move {
            let database = Database::new(transaction);
            database
                .set_crate_deps_analysis(&job.name, &job.version, has_outdated, has_cves)
                .await
        })
        .await?
    };
    if (has_outdated != old_has_outdated && configuration.deps_notify_outdated)
        || (has_cves != old_has_cves && configuration.deps_notify_cves)
    {
        // must send some notification
        let owners = {
            let mut connection = pool.acquire().await?;
            in_transaction(&mut connection, |transaction| async move {
                let database = Database::new(transaction);
                database.get_crate_owners(&job.name).await
            })
            .await?
        };
        let owners = owners.users.into_iter().map(|owner| owner.email).collect::<Vec<_>>();
        if has_outdated != old_has_outdated {
            // new outdated dependencies ...
            let mut body = String::new();
            writeln!(
                body,
                "New outdated dependencies have been found for {} {}",
                job.name, job.version
            )
            .unwrap();
            writeln!(
                body,
                "See {}/crates/{}/{}",
                configuration.web_public_uri, job.name, job.version
            )
            .unwrap();
            writeln!(body).unwrap();
            for dep in &analysis.direct_dependencies {
                if dep.is_outdated {
                    writeln!(
                        body,
                        "- {}, required {}, latest is {}",
                        dep.package, dep.required, dep.last_version
                    )
                    .unwrap();
                }
            }
            service_email_sender
                .send_email(
                    &owners,
                    &format!("Cratery - outdated dependencies for {} {}", job.name, job.version),
                    body,
                )
                .await?;
        }
        if has_cves != old_has_cves {
            // new CVEs ...
            let mut body = String::new();
            writeln!(
                body,
                "New vulnerable dependencies have been found for {} {}",
                job.name, job.version
            )
            .unwrap();
            writeln!(
                body,
                "See {}/crates/{}/{}",
                configuration.web_public_uri, job.name, job.version
            )
            .unwrap();
            writeln!(body).unwrap();
            for adv in &analysis.advisories {
                writeln!(
                    body,
                    "- {} resolved version {} is vulnerable to CVE https://rustsec.org/advisories/{}.html",
                    adv.package, adv.version, adv.content.id
                )
                .unwrap();
                writeln!(body, "  => {}", adv.content.summary).unwrap();
            }
            service_email_sender
                .send_email(
                    &owners,
                    &format!("Cratery - vulnerable dependencies for {} {}", job.name, job.version),
                    body,
                )
                .await?;
        }
    }
    Ok(())
}

/// Service to check the dependencies of a crate
pub trait DepsChecker {
    /// Ensures that a local cache for crates.io exists
    fn precache_crate_io(&self) -> FaillibleFuture<'_, ()>;

    /// Checks the dependencies of a local crate
    fn check_crate<'a>(
        &'a self,
        package: &'a str,
        version: &'a str,
        targets: &'a [String],
    ) -> FaillibleFuture<'a, DepsAnalysis>;
}

/// Gets the dependencies checker service
pub fn get_deps_checker(
    configuration: Arc<Configuration>,
    service_index: Arc<dyn Index + Send + Sync>,
    service_rustsec: Arc<dyn RustSecChecker + Send + Sync>,
) -> Arc<dyn DepsChecker + Send + Sync> {
    Arc::new(DepsCheckerImpl {
        data: Mutex::new(DepsCheckerData::default()),
        configuration,
        service_index,
        service_rustsec,
    })
}

/// Data for the service to check the dependencies of a crate
#[derive(Debug, Clone, Default)]
struct DepsCheckerData {
    /// The last time a piece of data was touched
    last_touch: HashMap<String, Instant>,
}

/// Service to check the dependencies of a crate
struct DepsCheckerImpl {
    /// The data for the service
    data: Mutex<DepsCheckerData>,
    /// The app configuration
    configuration: Arc<Configuration>,
    /// Access to the index
    service_index: Arc<dyn Index + Send + Sync>,
    /// The `RustSec` service
    service_rustsec: Arc<dyn RustSecChecker + Send + Sync>,
}

/// The URI identifying crates.io as the registry for a dependency
const CRATES_IO_REGISTRY_URI: &str = "https://github.com/rust-lang/crates.io-index";
/// The prefixes URI for the index for dependencies on crates.io
const _CRATES_IO_INDEX_SPARSE_URI: &str = "https://index.crates.io/";
/// Registry name for crates.io
const CRATES_IO_NAME: &str = "crates.io";
/// Name of the sub-directory to use within the data directory
const DATA_SUB_DIR: &str = "deps";

impl DepsChecker for DepsCheckerImpl {
    /// Ensures that a local cache for crates.io exists
    fn precache_crate_io(&self) -> FaillibleFuture<'_, ()> {
        Box::pin(async move { self.do_precache_crate_io().await })
    }

    /// Checks the dependencies of a local crate
    fn check_crate<'a>(
        &'a self,
        package: &'a str,
        version: &'a str,
        targets: &'a [String],
    ) -> FaillibleFuture<'a, DepsAnalysis> {
        Box::pin(async move { self.do_check_crate(package, version, targets).await })
    }
}

impl DepsCheckerImpl {
    /// Ensures that a local cache for crates.io exists
    async fn do_precache_crate_io(&self) -> Result<(), ApiError> {
        self.get_dependency_info_git("rand", CRATES_IO_NAME, CRATES_IO_REGISTRY_URI)
            .await?;
        Ok(())
    }

    /// Checks the dependencies of a local crate
    async fn do_check_crate(&self, package: &str, version: &str, targets: &[String]) -> Result<DepsAnalysis, ApiError> {
        let metadata = self.service_index.get_crate_data(package).await?;
        let metadata = metadata
            .iter()
            .find(|meta| meta.vers == version)
            .ok_or_else(error_not_found)?;

        let graph = self.get_dependencies_closure(&metadata.deps, targets).await?;
        let mut advisories = Vec::new();
        for dep in &graph.crates {
            for resolution in &dep.resolutions {
                let version = dep.versions[resolution.version_index].semver.clone();
                let simples = self.service_rustsec.check_crate(&dep.name, &version).await?;
                for simple in simples {
                    if advisories
                        .iter()
                        .all(|a: &DepAdvisory| a.package != dep.name && a.version != version && a.content.id != simple.id)
                    {
                        advisories.push(DepAdvisory {
                            package: dep.name.clone(),
                            version: version.clone(),
                            content: simple,
                        });
                    }
                }
            }
        }
        Ok(DepsAnalysis::new(&graph, &metadata.deps, advisories))
    }

    /// Gets the transitive closure of dependencies
    async fn get_dependencies_closure(
        &self,
        directs: &[IndexCrateDependency],
        targets: &[String],
    ) -> Result<DepsGraph, ApiError> {
        let mut graph = if targets.is_empty() {
            // use the host as default target
            DepsGraph::new(&[self.configuration.self_toolchain_host.clone()])
        } else {
            DepsGraph::new(targets)
        };
        let get_versions = |registry: Option<String>, name: String| async move {
            self.get_dependency_versions(registry.as_deref(), &name).await
        };
        for direct in directs {
            if direct.is_active_for(targets, &[]) {
                graph
                    .resolve(direct, &[], &[DepsGraphCrateOrigin::Direct(direct.kind)], &get_versions)
                    .await?;
            }
        }
        graph.close(&get_versions).await?;
        Ok(graph)
    }

    /// Retrieves the versions of a dependency
    async fn get_dependency_versions(&self, registry: Option<&str>, name: &str) -> Result<Vec<IndexCrateMetadata>, ApiError> {
        if let Some(registry) = registry {
            if registry == BUILTIN_CRATES_REGISTRY_URI {
                Ok(Self::generate_for_built_in(name, &self.configuration.self_toolchain_version))
            } else if registry == CRATES_IO_REGISTRY_URI {
                self.get_dependency_info_git(name, CRATES_IO_NAME, CRATES_IO_REGISTRY_URI)
                    .await
            } else if let Some(registry) = self
                .configuration
                .external_registries
                .iter()
                .find(|reg| reg.index == registry)
            {
                match registry.protocol {
                    ExternalRegistryProtocol::Git => self.get_dependency_info_git(name, &registry.name, &registry.index).await,
                    ExternalRegistryProtocol::Sparse => {
                        self.get_dependency_info_sparse(
                            name,
                            &registry.name,
                            &registry.index,
                            Some((&registry.login, &registry.token)),
                        )
                        .await
                    }
                }
            } else {
                Err(specialize(error_not_found(), format!("Unknown registry: {registry}")))
            }
        } else {
            // same registry, lookup in internal index
            self.service_index.get_crate_data(name).await
        }
    }

    /// Generates the versions vector for a built-in crate
    fn generate_for_built_in(name: &str, toolchain_version: &str) -> Vec<IndexCrateMetadata> {
        vec![IndexCrateMetadata {
            name: name.to_string(),
            vers: toolchain_version.to_string(),
            ..Default::default()
        }]
    }

    /// Gets the crate index data for a dependency in a registry with the git protocol
    async fn get_dependency_info_git(
        &self,
        dep_name: &str,
        reg_name: &str,
        index_uri: &str,
    ) -> Result<Vec<IndexCrateMetadata>, ApiError> {
        let mut data = self.data.lock().await;

        let last_touch = data.last_touch.get(reg_name).copied().unwrap_or_else(stale_instant);
        let now = Instant::now();
        let is_stale = now.duration_since(last_touch) > Duration::from_millis(self.configuration.deps_stale_registry);

        let mut reg_location = PathBuf::from(&self.configuration.data_dir);
        reg_location.push(DATA_SUB_DIR);
        reg_location.push(reg_name);
        if is_stale {
            if tokio::fs::try_exists(&reg_location).await? {
                crate::utils::execute_git(&reg_location, &["pull", "origin", "master"]).await?;
            } else {
                tokio::fs::create_dir_all(&reg_location).await?;
                crate::utils::execute_git(&reg_location, &["clone", index_uri, "."]).await?;
            }
            data.last_touch.insert(reg_name.to_string(), now);
        }

        // load from file
        let file_path = self.get_dependency_info_file_path(dep_name, reg_name).await?;
        let file = File::open(&file_path).await?;
        let mut reader = tokio::io::BufReader::new(file).lines();
        let mut results = Vec::new();
        while let Some(line) = reader.next_line().await? {
            let data = serde_json::from_str(&line)?;
            results.push(data);
        }
        Ok(results)
    }

    /// Builds the path in the storage to the local file
    async fn get_dependency_info_file_path(&self, dep_name: &str, reg_name: &str) -> Result<PathBuf, ApiError> {
        let mut reg_location = PathBuf::from(&self.configuration.data_dir);
        reg_location.push(DATA_SUB_DIR);
        reg_location.push(reg_name);
        let file_path = crate::services::index::build_package_file_path(reg_location, dep_name);
        tokio::fs::create_dir_all(file_path.parent().unwrap()).await?;
        Ok(file_path)
    }

    /// Build the target URI to be used to retrieve the last data and store the access timestamp
    fn get_dependency_info_sparse_target_uri(dep_name: &str, index_uri: &str) -> String {
        let lowercase = dep_name.to_ascii_lowercase();
        let (first, second) = crate::services::index::package_file_path(&lowercase);
        // expect `index_uri` to end with a trailing /
        let mut target_uri = format!("{index_uri}{first}");
        if let Some(second) = second {
            target_uri.push('/');
            target_uri.push_str(second);
        }
        target_uri.push('/');
        target_uri.push_str(&lowercase);
        target_uri
    }

    /// Gets the crate index data for a dependency in a sparse registry
    async fn get_dependency_info_sparse(
        &self,
        dep_name: &str,
        reg_name: &str,
        index_uri: &str,
        credentials: Option<(&str, &str)>,
    ) -> Result<Vec<IndexCrateMetadata>, ApiError> {
        let target_uri = Self::get_dependency_info_sparse_target_uri(dep_name, index_uri);
        let file_path = self.get_dependency_info_file_path(dep_name, reg_name).await?;

        let mut data = self.data.lock().await;

        if tokio::fs::try_exists(&file_path).await? {
            // is it stale?
            let last_touch = data.last_touch.get(&target_uri).copied().unwrap_or_else(stale_instant);
            let now = Instant::now();
            let is_stale = now.duration_since(last_touch) > Duration::from_millis(self.configuration.deps_stale_registry);
            if is_stale {
                self.get_dependency_info_sparse_fetch(&file_path, target_uri, credentials, &mut data)
                    .await
            } else {
                // load from file
                let file = File::open(&file_path).await?;
                let mut reader = tokio::io::BufReader::new(file).lines();
                let mut results = Vec::new();
                while let Some(line) = reader.next_line().await? {
                    let data = serde_json::from_str(&line)?;
                    results.push(data);
                }
                Ok(results)
            }
        } else {
            // no data yet, fetch
            self.get_dependency_info_sparse_fetch(&file_path, target_uri, credentials, &mut data)
                .await
        }
    }

    /// Fetches the data for a dependency in a sparse registry
    async fn get_dependency_info_sparse_fetch(
        &self,
        file_path: &Path,
        target_uri: String,
        credentials: Option<(&str, &str)>,
        data: &mut DepsCheckerData,
    ) -> Result<Vec<IndexCrateMetadata>, ApiError> {
        let mut request = reqwest::Client::new().get(&target_uri);
        if let Some((login, password)) = credentials {
            let value = STANDARD.encode(format!("{login}:{password}"));
            request = request.header("Authorization", format!("Basic {value}"));
        }
        let response = request.send().await?;
        if !response.status().is_success() {
            return Err(specialize(
                error_backend_failure(),
                format!(
                    "failed to get dependency info at {target_uri}: error code {}",
                    response.status().as_u16()
                ),
            ));
        }
        let content = response.bytes().await?;
        let bytes: &[u8] = &content;

        // parse
        let mut results = Vec::new();
        for line in BufReader::new(bytes).lines() {
            let line = line?;
            let data = serde_json::from_str(&line)?;
            results.push(data);
        }

        // write to storage
        tokio::fs::write(&file_path, &content).await?;

        // touch the data
        data.last_touch.insert(target_uri, Instant::now());

        Ok(results)
    }
}
