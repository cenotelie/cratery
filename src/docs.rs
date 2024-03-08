//! Docs generation and management

use std::{path::PathBuf, process::Stdio, sync::Arc};

use cenotelie_lib_apierror::{error_backend_failure, specialize, ApiError};
use flate2::bufread::GzDecoder;
use futures::{channel::mpsc::UnboundedSender, StreamExt};
use log::{error, info};
use sqlx::{Pool, Sqlite};
use tar::Archive;
use tokio::{
    fs::File,
    io::{AsyncWriteExt, BufWriter},
    process::Command,
    task::JoinHandle,
};

use crate::{
    objects::{Configuration, DocsGenerationJob},
    storage,
};

/// Creates a worker for the generation of documentation
pub fn create_docs_worker(
    configuration: Arc<Configuration>,
    pool: Pool<Sqlite>,
) -> (UnboundedSender<DocsGenerationJob>, JoinHandle<()>) {
    let (sender, mut receiver) = futures::channel::mpsc::unbounded();
    let handle = tokio::spawn(async move {
        while let Some(job) = receiver.next().await {
            if let Err(e) = docs_worker_job(&configuration, &pool, job).await {
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
async fn docs_worker_job(configuration: &Configuration, _pool: &Pool<Sqlite>, job: DocsGenerationJob) -> Result<(), ApiError> {
    info!("generating doc for {} {}", job.crate_name, job.crate_version);
    // get the content
    let content = storage::download_crate(configuration, &job.crate_name, &job.crate_version).await?;
    // extract to a temp folder
    let location = extract_content(&job.crate_name, &job.crate_version, &content)?;
    // generate the doc
    let _location_inner = generate_doc(configuration, &location, &job.crate_name).await?;
    // transform the doc

    // // set the package as documented
    // let mut connection = pool.acquire().await?;
    // in_transaction(&mut connection, |transaction| async move {
    //     let app = Application::new(transaction);
    //     app.set_package_documented(&job.crate_name, &job.crate_version).await
    // })
    // .await?;

    // cleanup
    // tokio::fs::remove_dir_all(&location).await?;
    Ok(())
}

/// Generates and upload the documentation for a crate
fn extract_content(name: &str, version: &str, content: &[u8]) -> Result<String, ApiError> {
    let decoder = GzDecoder::new(content);
    let mut archive = Archive::new(decoder);
    let target = format!("/tmp/{name}_{version}");
    archive.unpack(&target)?;
    Ok(target)
}

/// Generate the documentation for the package in a specific folder
async fn generate_doc(configuration: &Configuration, location: &str, _name: &str) -> Result<PathBuf, ApiError> {
    let mut path: PathBuf = PathBuf::from(location);
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
