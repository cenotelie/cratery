/*******************************************************************************
 * Copyright (c) 2024 Cénotélie Opérations SAS (cenotelie.fr)
 ******************************************************************************/

//! Docs generation and management

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
use crate::model::docs::{DocGenEvent, DocGenJob, DocGenJobState, DocGenJobUpdate, DocGenTrigger};
use crate::model::JobCrate;
use crate::services::database::{db_transaction_read, db_transaction_write};
use crate::services::storage::Storage;
use crate::utils::apierror::{error_backend_failure, specialize, ApiError};
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
    fn queue<'a>(&'a self, spec: &'a JobCrate, trigger: &'a DocGenTrigger) -> FaillibleFuture<'a, DocGenJob>;

    /// Adds a listener to job updates
    fn add_listener(&self, listener: Sender<DocGenEvent>) -> FaillibleFuture<'_, ()>;
}

/// Gets the documentation generation service
pub fn get_service(
    configuration: Arc<Configuration>,
    service_db_pool: RwSqlitePool,
    service_storage: Arc<dyn Storage + Send + Sync>,
) -> Arc<dyn DocsGenerator + Send + Sync> {
    let service = Arc::new(DocsGeneratorImpl {
        configuration,
        service_db_pool,
        service_storage,
        listeners: Arc::new(Mutex::new(Vec::new())),
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
            let data = self.service_storage.download_doc_file(&Self::job_log_location(&job)).await?;
            Ok(String::from_utf8(data)?)
        })
    }

    /// Queues a job for documentation generation
    fn queue<'a>(&'a self, spec: &'a JobCrate, trigger: &'a DocGenTrigger) -> FaillibleFuture<'a, DocGenJob> {
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
    /// Gets the location in storage of the log for a documentation job
    fn job_log_location(job: &DocGenJob) -> String {
        format!("{}/{}/log_{}.txt", job.package, job.version, job.id)
    }

    /// Send an event to listeners
    async fn send_event(&self, event: DocGenEvent) -> Result<(), ApiError> {
        let mut listeners = self.listeners.lock().await;
        let mut index = if listeners.is_empty() {
            Some(listeners.len() - 1)
        } else {
            None
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
    async fn update_job(&self, job_id: i64, state: DocGenJobState, log: Option<&str>) -> Result<(), ApiError> {
        db_transaction_write(&self.service_db_pool, "update_docgen_job", |database| async move {
            database.update_docgen_job(job_id, state).await
        })
        .await?;

        // send updates
        let now = Local::now().naive_local();
        self.send_event(DocGenEvent::Update(DocGenJobUpdate {
            job_id,
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
        self.update_job(job.id, DocGenJobState::Working, None).await?;
        match self.docs_worker_execute_job(job).await {
            Ok((final_state, output)) => {
                self.update_job(job.id, final_state, output.as_deref()).await?;
            }
            Err(e) => {
                error!("{e}");
                if let Some(backtrace) = &e.backtrace {
                    error!("{backtrace}");
                }
                self.update_job(job.id, DocGenJobState::Failure, Some(&e.to_string())).await?;
            }
        }
        Ok(())
    }

    /// Executes a documentation generation job
    async fn docs_worker_execute_job(&self, job: &DocGenJob) -> Result<(DocGenJobState, Option<String>), ApiError> {
        info!("generating doc for {} {}", job.package, job.version);
        let content = self.service_storage.download_crate(&job.package, &job.version).await?;
        let temp_folder = Self::extract_content(&job.package, &job.version, &content)?;
        let (final_state, output) = match self.generate_doc(&temp_folder).await {
            Ok(mut project_folder) => {
                project_folder.push("target");
                project_folder.push("doc");
                let doc_folder = project_folder;
                self.upload_package(&job.package, &job.version, &doc_folder).await?;
                (DocGenJobState::Success, None)
            }
            Err(e) => {
                // upload the log
                let log = e.details.unwrap();
                self.service_storage
                    .store_doc_data(&Self::job_log_location(job), log.as_bytes().to_vec())
                    .await?;
                (DocGenJobState::Failure, Some(log))
            }
        };

        db_transaction_write(&self.service_db_pool, "set_crate_documentation", |database| async move {
            database
                .set_crate_documentation(&job.package, &job.version, final_state == DocGenJobState::Success)
                .await
        })
        .await?;
        tokio::fs::remove_dir_all(&temp_folder).await?;
        Ok((final_state, output))
    }

    /// Generates and upload the documentation for a crate
    fn extract_content(name: &str, version: &str, content: &[u8]) -> Result<PathBuf, ApiError> {
        let decoder = GzDecoder::new(content);
        let mut archive = Archive::new(decoder);
        let target = format!("/tmp/{name}_{version}");
        archive.unpack(&target)?;
        Ok(PathBuf::from(target))
    }

    /// Generate the documentation for the package in a specific folder
    async fn generate_doc(&self, temp_folder: &Path) -> Result<PathBuf, ApiError> {
        let mut path: PathBuf = temp_folder.to_path_buf();
        // get the first sub dir
        let mut dir = tokio::fs::read_dir(&path).await?;
        let first = dir.next_entry().await?.unwrap();
        path = first.path();

        let mut command = Command::new("cargo");
        command
            .current_dir(&path)
            .arg("rustdoc")
            .arg("-Zunstable-options")
            .arg("-Zrustdoc-map")
            .arg("--all-features")
            .arg("--config")
            .arg("build.rustdocflags=[\"-Zunstable-options\",\"--extern-html-root-takes-precedence\"]")
            .arg("--config")
            .arg(format!(
                "doc.extern-map.registries.{}=\"{}/docs\"",
                self.configuration.self_local_name, self.configuration.web_public_uri
            ));
        for external in &self.configuration.external_registries {
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

        if !output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);
            let error = format!("-- stdout\n{stdout}\n\n-- stderr\n{stderr}");
            return Err(specialize(error_backend_failure(), error));
        }
        Ok(path)
    }

    /// Uploads the documentation for package
    async fn upload_package(&self, name: &str, version: &str, doc_folder: &Path) -> Result<(), ApiError> {
        let files = Self::upload_package_find_files(doc_folder, &format!("{name}/{version}")).await?;
        let results = n_at_a_time(
            files.into_iter().map(|(key, path)| {
                let service_storage = self.service_storage.clone();
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
    async fn upload_package_find_files(folder: &Path, prefix: &str) -> Result<Vec<(String, PathBuf)>, std::io::Error> {
        let mut results = Vec::new();
        let mut to_explore = vec![(folder.to_path_buf(), prefix.to_string())];
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
}
