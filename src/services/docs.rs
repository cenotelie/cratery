/*******************************************************************************
 * Copyright (c) 2024 Cénotélie Opérations SAS (cenotelie.fr)
 ******************************************************************************/

//! Docs generation and management

use std::fmt::Write;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use chrono::Local;
use flate2::bufread::GzDecoder;
use log::{error, info};
use tar::Archive;
use tokio::process::Command;
use tokio::sync::mpsc::Sender;
use tokio::sync::Mutex;
use tokio::time::interval;

use crate::model::config::Configuration;
use crate::model::docs::{DocGenEvent, DocGenJob, DocGenJobSpec, DocGenJobState, DocGenJobUpdate, DocGenTrigger};
use crate::model::worker::{JobIdentifier, JobSpecification, JobUpdate, WorkersManager};
use crate::model::CHANNEL_NIGHTLY;
use crate::services::database::{db_transaction_read, db_transaction_write};
use crate::services::storage::Storage;
use crate::utils::apierror::{error_backend_failure, error_invalid_request, specialize, ApiError};
use crate::utils::concurrent::n_at_a_time;
use crate::utils::db::RwSqlitePool;
use crate::utils::FaillibleFuture;

/// Service to generate documentation for a crate
pub trait DocsGenerator {
    /// Gets all the jobs
    fn get_jobs(&self) -> FaillibleFuture<'_, Vec<DocGenJob>>;

    /// Gets the log for a job
    fn get_job_log(&self, job_id: i64) -> FaillibleFuture<'_, String>;

    /// Queues a job for documentation generation
    fn queue<'a>(&'a self, spec: &'a DocGenJobSpec, trigger: &'a DocGenTrigger) -> FaillibleFuture<'a, DocGenJob>;

    /// Adds a listener to job updates
    fn add_listener(&self, listener: Sender<DocGenEvent>) -> FaillibleFuture<'_, ()>;
}

/// Gets the documentation generation service
pub fn get_service(
    configuration: Arc<Configuration>,
    service_db_pool: RwSqlitePool,
    service_storage: Arc<dyn Storage + Send + Sync>,
    worker_nodes: WorkersManager,
) -> Arc<dyn DocsGenerator + Send + Sync> {
    let service = Arc::new(DocsGeneratorImpl {
        configuration,
        service_db_pool,
        service_storage,
        listeners: Arc::new(Mutex::new(Vec::new())),
        worker_nodes,
    });
    // launch workers
    let _handle = tokio::spawn({
        let service = service.clone();
        async move {
            service.worker().await;
        }
    });
    service
}

/// Service to generate documentation for a crate
#[derive(Clone)]
struct DocsGeneratorImpl {
    /// The configuration
    configuration: Arc<Configuration>,
    /// The database pool
    service_db_pool: RwSqlitePool,
    /// The storage layer
    service_storage: Arc<dyn Storage + Send + Sync>,
    /// The active listeners
    listeners: Arc<Mutex<Vec<Sender<DocGenEvent>>>>,
    /// The worker nodes
    worker_nodes: WorkersManager,
}

impl DocsGenerator for DocsGeneratorImpl {
    /// Gets all the jobs
    fn get_jobs(&self) -> FaillibleFuture<'_, Vec<DocGenJob>> {
        Box::pin(async move {
            db_transaction_read(
                &self.service_db_pool,
                |database| async move { database.get_docgen_jobs().await },
            )
            .await
        })
    }

    /// Gets the log for a job
    fn get_job_log(&self, job_id: i64) -> FaillibleFuture<'_, String> {
        Box::pin(async move {
            let job = db_transaction_read(&self.service_db_pool, |database| async move {
                database.get_docgen_job(job_id).await
            })
            .await?;
            let data = self.service_storage.download_doc_file(&job_log_location(&job)).await?;
            Ok(String::from_utf8(data)?)
        })
    }

    /// Queues a job for documentation generation
    fn queue<'a>(&'a self, spec: &'a DocGenJobSpec, trigger: &'a DocGenTrigger) -> FaillibleFuture<'a, DocGenJob> {
        Box::pin(async move {
            let job = db_transaction_write(&self.service_db_pool, "create_docgen_job", |database| async move {
                database.create_docgen_job(spec, trigger).await
            })
            .await?;
            self.send_event(DocGenEvent::Queued(Box::new(job.clone()))).await?;
            Ok(job)
        })
    }

    /// Adds a listener to job updates
    fn add_listener(&self, listener: Sender<DocGenEvent>) -> FaillibleFuture<'_, ()> {
        Box::pin(async move {
            self.listeners.lock().await.push(listener);
            Ok(())
        })
    }
}

impl DocsGeneratorImpl {
    /// Send an event to listeners
    async fn send_event(&self, event: DocGenEvent) -> Result<(), ApiError> {
        let mut listeners = self.listeners.lock().await;
        let mut index = if listeners.is_empty() {
            None
        } else {
            Some(listeners.len() - 1)
        };
        while let Some(i) = index {
            if listeners[i].send(event.clone()).await.is_err() {
                // remove
                listeners.swap_remove(i);
            }
            index = if i == 0 { None } else { Some(i - 1) };
        }
        Ok(())
    }

    /// Update a job
    async fn update_job(&self, job: &DocGenJob, state: DocGenJobState, log: Option<&str>) -> Result<(), ApiError> {
        db_transaction_write(&self.service_db_pool, "update_job", |database| async move {
            database.update_docgen_job(job.id, state).await?;
            database
                .set_crate_documentation(
                    &job.package,
                    &job.version,
                    &job.target,
                    state != DocGenJobState::Queued,
                    state == DocGenJobState::Success,
                )
                .await?;
            Ok::<_, ApiError>(())
        })
        .await?;

        // send updates
        let now = Local::now().naive_local();
        self.send_event(DocGenEvent::Update(DocGenJobUpdate {
            job_id: job.id,
            state,
            last_update: now,
            log: log.map(str::to_string),
        }))
        .await?;
        Ok(())
    }

    /// Gets the next job, if any
    async fn get_next_job(&self) -> Result<Option<DocGenJob>, ApiError> {
        db_transaction_read(&self.service_db_pool, |database| async move {
            database.get_next_docgen_job().await
        })
        .await
    }

    /// Implementation of the worker
    async fn worker(&self) {
        // check every 10 seconds
        let mut interval = interval(Duration::from_secs(10));
        loop {
            interval.tick().await;
            match self.get_next_job().await {
                Err(e) => {
                    error!("{e}");
                    if let Some(backtrace) = &e.backtrace {
                        error!("{backtrace}");
                    }
                }
                Ok(Some(job)) => {
                    if let Err(e) = self.docs_worker_on_job(&job).await {
                        error!("{e}");
                        if let Some(backtrace) = &e.backtrace {
                            error!("{backtrace}");
                        }
                    }
                }
                Ok(None) => {}
            }
        }
    }

    /// Executes a documentation generation job
    async fn docs_worker_on_job(&self, job: &DocGenJob) -> Result<(), ApiError> {
        if let Err(e) = self.docs_worker_execute_job(job).await {
            self.update_job(job, DocGenJobState::Failure, Some(&e.to_string())).await?;
            // upload the error as log
            let mut log = e.to_string();
            if let Some(backtrace) = &e.backtrace {
                log.push('\n');
                write!(log, "{backtrace}").unwrap();
            }
            self.service_storage
                .store_doc_data(&job_log_location(job), log.as_bytes().to_vec())
                .await?;
        }
        Ok(())
    }

    /// Executes a documentation generation job
    async fn docs_worker_execute_job(&self, job: &DocGenJob) -> Result<(), ApiError> {
        if self.worker_nodes.has_workers() {
            self.docs_worker_execute_job_remote(job).await
        } else {
            self.docs_worker_execute_job_local(job).await
        }
    }

    /// Executes a documentation generation job
    async fn docs_worker_execute_job_remote(&self, job: &DocGenJob) -> Result<(), ApiError> {
        let mut worker = self
            .worker_nodes
            .get_worker_for(job.get_worker_selector(), JobIdentifier::DocGen(job.id))
            .await;
        self.update_job(job, DocGenJobState::Working, None).await?;
        worker.sender().send(JobSpecification::DocGen(job.clone())).await?;
        while let Some(update) = worker.update_receiver().recv().await {
            let JobUpdate::DocGen(update) = update;
            self.update_job(job, update.state, update.log.as_deref()).await?;
            if update.state.is_final() {
                return Ok(());
            }
        }
        Err(specialize(
            error_backend_failure(),
            String::from("worker unexpectedly disconnected"),
        ))
    }

    /// Executes a documentation generation job
    async fn docs_worker_execute_job_local(&self, job: &DocGenJob) -> Result<(), ApiError> {
        self.update_job(job, DocGenJobState::Working, None).await?;
        let (final_state, output) = generate_doc_for_job(&self.configuration, self.service_storage.clone(), job).await?;
        self.update_job(job, final_state, Some(&output)).await?;
        Ok(())
    }
}

/// Gets the location in storage of the log for a documentation job
fn job_log_location(job: &DocGenJob) -> String {
    format!("logs/job_{:06}", job.id)
}

/// Executes a documentation generation job
pub async fn generate_doc_for_job(
    configuration: &Configuration,
    service_storage: Arc<dyn Storage + Send + Sync>,
    job: &DocGenJob,
) -> Result<(DocGenJobState, String), ApiError> {
    info!("generating doc for {} {}", job.package, job.version);
    on_job_check_target(configuration, job).await?;
    let content = service_storage.download_crate(&job.package, &job.version).await?;
    let temp_folder = extract_content(&job.package, &job.version, &content)?;
    let project_folder = get_project_folder_in(&temp_folder).await?;

    let (final_state, output) = if configuration.docs_gen_mock {
        (DocGenJobState::Success, String::from("mocked"))
    } else {
        match do_generate_doc(configuration, &project_folder, &job.target).await {
            Ok(log) => {
                service_storage
                    .store_doc_data(&job_log_location(job), log.as_bytes().to_vec())
                    .await?;
                let mut project_folder = project_folder.clone();
                project_folder.push("target");
                project_folder.push(&job.target);
                project_folder.push("doc");
                let doc_folder = project_folder;
                upload_package(
                    service_storage.clone(),
                    &doc_folder,
                    &format!("{}/{}/{}", &job.package, &job.version, &job.target),
                )
                .await?;
                (DocGenJobState::Success, log)
            }
            Err(e) => {
                // upload the log
                let log = e.details.unwrap();
                service_storage
                    .store_doc_data(&job_log_location(job), log.as_bytes().to_vec())
                    .await?;
                (DocGenJobState::Failure, log)
            }
        }
    };
    tokio::fs::remove_dir_all(&temp_folder).await?;
    Ok((final_state, output))
}

/// Checks for a target to be present when generating documentation
async fn on_job_check_target(configuration: &Configuration, job: &DocGenJob) -> Result<(), ApiError> {
    if configuration.self_installed_targets.iter().any(|t| t == &job.target) {
        // target is installed
        return Ok(());
    }
    if !configuration.docs_autoinstall_targets {
        // cannot install the target
        return Err(specialize(
            error_invalid_request(),
            String::from("target is not installed and installation is de-activated in configuration"),
        ));
    }
    if configuration.self_installable_targets.iter().all(|t| t != &job.target) {
        // target cannot installed
        return Err(specialize(
            error_invalid_request(),
            String::from("target is not installed and cannot be installed on this host"),
        ));
    }
    // check whether it was actually installed after launch
    let actually_installed = crate::model::config::get_installed_targets(CHANNEL_NIGHTLY).await;
    if actually_installed.iter().any(|t| t == &job.target) {
        // the target was actually installed (after the app launched)
        return Ok(());
    }
    // try to install
    crate::model::config::install_target(CHANNEL_NIGHTLY, &job.target).await?;
    Ok(())
}

/// Generates and upload the documentation for a crate
fn extract_content(name: &str, version: &str, content: &[u8]) -> Result<PathBuf, ApiError> {
    let decoder = GzDecoder::new(content);
    let mut archive = Archive::new(decoder);
    let target = format!("/tmp/{name}_{version}");
    archive.unpack(&target)?;
    Ok(PathBuf::from(target))
}

/// Gets the project folder in the specified temp
async fn get_project_folder_in(temp_folder: &Path) -> Result<PathBuf, ApiError> {
    let temp_folder = temp_folder.to_path_buf();
    // get the first sub dir
    let mut dir = tokio::fs::read_dir(&temp_folder).await?;
    Ok(dir.next_entry().await?.unwrap().path())
}

/// Generate the documentation for the package in a specific folder
async fn do_generate_doc(configuration: &Configuration, project_folder: &Path, target: &str) -> Result<String, ApiError> {
    let mut command = Command::new("cargo");
    command
        .current_dir(project_folder)
        .arg(CHANNEL_NIGHTLY)
        .arg("rustdoc")
        .arg("-Zunstable-options")
        .arg("-Zrustdoc-map")
        .arg("--all-features")
        .arg("--target")
        .arg(target)
        .arg("--config")
        .arg("build.rustdocflags=[\"-Zunstable-options\",\"--extern-html-root-takes-precedence\"]")
        .arg("--config")
        .arg(format!(
            "doc.extern-map.registries.{}=\"{}/docs\"",
            configuration.self_local_name, configuration.web_public_uri
        ));
    if configuration.index.allow_protocol_git && configuration.index.allow_protocol_sparse {
        // both git and sparse => add specialized sparse
        command.arg(format!(
            "doc.extern-map.registries.{}sparse=\"{}/docs\"",
            configuration.self_local_name, configuration.web_public_uri
        ));
    }
    for external in &configuration.external_registries {
        command.arg("--config").arg(format!(
            "doc.extern-map.registries.{}=\"{}\"",
            external.name, external.docs_root
        ));
    }
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    drop(child.stdin.take()); // close stdin
    let output = child.wait_with_output().await?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let log = format!("-- stdout\n{stdout}\n\n-- stderr\n{stderr}");

    if output.status.success() {
        Ok(log)
    } else {
        Err(specialize(error_backend_failure(), log))
    }
}

/// Uploads the documentation for package
async fn upload_package(
    service_storage: Arc<dyn Storage + Send + Sync>,
    doc_folder: &Path,
    key_prefix: &str,
) -> Result<(), ApiError> {
    let files = upload_package_find_files(doc_folder, key_prefix).await?;
    let results = n_at_a_time(
        files.into_iter().map(|(key, path)| {
            let service_storage = service_storage.clone();
            Box::pin(async move { service_storage.store_doc_file(&key, &path).await })
        }),
        8,
        Result::is_err,
    )
    .await;
    for result in results {
        result?;
    }
    Ok(())
}

/// Find target to upload in a folder and its sub-folders
async fn upload_package_find_files(folder: &Path, key_prefix: &str) -> Result<Vec<(String, PathBuf)>, std::io::Error> {
    let mut results = Vec::new();
    let mut to_explore = vec![(folder.to_path_buf(), key_prefix.to_string())];
    while let Some((folder, prefix)) = to_explore.pop() {
        let mut dir = tokio::fs::read_dir(folder).await?;
        while let Some(entry) = dir.next_entry().await? {
            let entry_path = entry.path();
            let entry_type = entry.file_type().await?;
            if entry_type.is_file() {
                results.push((format!("{prefix}/{}", entry.file_name().to_str().unwrap()), entry_path));
            } else if entry_type.is_dir() {
                to_explore.push((entry_path, format!("{prefix}/{}", entry.file_name().to_str().unwrap())));
            }
        }
    }
    Ok(results)
}
