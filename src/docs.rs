/*******************************************************************************
 * Copyright (c) 2024 Cénotélie Opérations SAS (cenotelie.fr)
 ******************************************************************************/

//! Docs generation and management

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;

use cenotelie_lib_apierror::{error_backend_failure, specialize, ApiError};
use cenotelie_lib_async_utils::parallel_jobs::n_at_a_time;
use flate2::bufread::GzDecoder;
use futures::channel::mpsc::UnboundedSender;
use futures::StreamExt;
use log::{error, info};
use sqlx::{Pool, Sqlite};
use tar::Archive;
use tokio::fs::File;
use tokio::io::{AsyncWriteExt, BufWriter};
use tokio::process::Command;
use tokio::task::JoinHandle;

use crate::app::Application;
use crate::model::config::Configuration;
use crate::model::objects::DocsGenerationJob;
use crate::storage;
use crate::transaction::in_transaction;

/// Creates a worker for the generation of documentation
pub fn create_docs_worker(
    configuration: Arc<Configuration>,
    pool: Pool<Sqlite>,
) -> (UnboundedSender<DocsGenerationJob>, JoinHandle<()>) {
    let (sender, mut receiver) = futures::channel::mpsc::unbounded();
    let handle = tokio::spawn(async move {
        while let Some(job) = receiver.next().await {
            if let Err(e) = docs_worker_job(configuration.clone(), &pool, job).await {
                error!("{e}");
                if let Some(backtrace) = &e.backtrace {
                    error!("{backtrace}");
                }
            }
        }
    });
    (sender, handle)
}

/// Executes a documentation generation job
async fn docs_worker_job(
    configuration: Arc<Configuration>,
    pool: &Pool<Sqlite>,
    job: DocsGenerationJob,
) -> Result<(), ApiError> {
    info!("generating doc for {} {}", job.crate_name, job.crate_version);
    let content = storage::download_crate(&configuration, &job.crate_name, &job.crate_version).await?;
    let temp_folder = extract_content(&job.crate_name, &job.crate_version, &content)?;
    let mut project_folder = generate_doc(&configuration, &temp_folder).await?;
    project_folder.push("target");
    project_folder.push("doc");
    let doc_folder = project_folder;
    upload_package(configuration, &job.crate_name, &job.crate_version, &doc_folder).await?;
    let mut connection = pool.acquire().await?;
    in_transaction(&mut connection, |transaction| async move {
        let app = Application::new(transaction);
        app.set_package_documented(&job.crate_name, &job.crate_version).await
    })
    .await?;
    tokio::fs::remove_dir_all(&temp_folder).await?;
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

/// Generate the documentation for the package in a specific folder
async fn generate_doc(configuration: &Configuration, temp_folder: &Path) -> Result<PathBuf, ApiError> {
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
        .arg(format!("doc.extern-map.registries.local=\"{}/docs\"", configuration.uri));
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

    {
        // write output to file
        let mut path = path.clone();
        path.pop();
        path.push("output.txt");
        let file = File::create(path).await?;
        let mut writer = BufWriter::new(file);
        writer.write_all(&output.stdout).await?;
        writer.write_all(&output.stderr).await?;
        writer.flush().await?;
    }

    if !output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        let error = format!("-- stdout\n{stdout}\n\n-- stderr\n{stderr}");
        return Err(specialize(error_backend_failure(), error));
    }
    Ok(path)
}

/// Uploads the documentation for package
async fn upload_package(
    configuration: Arc<Configuration>,
    crate_name: &str,
    crate_version: &str,
    doc_folder: &Path,
) -> Result<(), ApiError> {
    let files = upload_package_find_files(doc_folder, &format!("docs/{crate_name}/{crate_version}")).await?;
    let results = n_at_a_time(
        files.into_iter().map(|(key, path)| {
            let configuration = configuration.clone();
            Box::pin(async move {
                cenotelie_lib_s3::upload_object_file(&configuration.s3, &configuration.bucket, &key, None, path).await
            })
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
