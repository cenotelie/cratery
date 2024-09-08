/*******************************************************************************
 * Copyright (c) 2024 Cénotélie Opérations SAS (cenotelie.fr)
 ******************************************************************************/

//! Service to fetch data about advisories against Rust crates on crates.io

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures::lock::Mutex;
use futures::StreamExt;
use log::error;
use semver::Version;
use tokio_stream::wrappers::ReadDirStream;

use crate::model::config::Configuration;
use crate::model::osv::{Advisory, SimpleAdvisory};
use crate::utils::apierror::ApiError;
use crate::utils::concurrent::n_at_a_time_stream;
use crate::utils::{stale_instant, FaillibleFuture};

/// Service to use the [RustSec](https://github.com/rustsec) data about crates
pub trait RustSecChecker {
    /// Gets the advisories against a crate
    fn check_crate<'a>(&'a self, package: &'a str, version: &'a Version) -> FaillibleFuture<'a, Vec<SimpleAdvisory>>;
}

/// Gets the rustsec service
#[must_use]
pub fn get_service(config: &Configuration) -> Arc<dyn RustSecChecker + Send + Sync> {
    Arc::new(RustSecCheckerImpl {
        data: Mutex::new(RustSecData::new(config.data_dir.clone(), config.deps_stale_registry)),
    })
}

struct RustSecCheckerImpl {
    /// The data for the service
    data: Mutex<RustSecData>,
}

impl RustSecChecker for RustSecCheckerImpl {
    /// Gets the advisories against a crate
    fn check_crate<'a>(&'a self, package: &'a str, version: &'a Version) -> FaillibleFuture<'a, Vec<SimpleAdvisory>> {
        Box::pin(async move {
            let mut data = self.data.lock().await;
            data.update_data().await?;
            let db = data.db.lock().unwrap();
            Ok(db
                .get(package)
                .map(|advisories| {
                    advisories
                        .iter()
                        .filter(|advisory| advisory.affects(version))
                        .cloned()
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default())
        })
    }
}

/// Service to use the [RustSec](https://github.com/rustsec) data about crates
#[derive(Debug, Clone)]
struct RustSecData {
    /// The data directory
    data_dir: String,
    /// Number of milliseconds after which the local data about an external registry are deemed stale and must be pulled again
    stale_registry: u64,
    /// The last time the data was updated
    last_touch: Instant,
    /// The known advisories
    db: Arc<std::sync::Mutex<HashMap<String, Vec<SimpleAdvisory>>>>,
}

/// The URI of the git repo with the `RustSec` database
const RUSTSEC_DB_GIT_URI: &str = "https://github.com/rustsec/advisory-db";
/// The branch inside the repo with the actual data
const RUSTSEC_DB_GIT_BRANCH: &str = "osv";
/// Name of the sub-directory to use within the data directory
const DATA_SUB_DIR: &str = "rustsec";

impl RustSecData {
    fn new(data_dir: String, stale_registry: u64) -> Self {
        Self {
            data_dir,
            stale_registry,
            last_touch: stale_instant(),
            db: Arc::new(std::sync::Mutex::new(HashMap::new())),
        }
    }

    /// Updates the data
    async fn update_data(&mut self) -> Result<(), ApiError> {
        let now = Instant::now();
        let is_stale = now.duration_since(self.last_touch) > Duration::from_millis(self.stale_registry);
        let mut reg_location = PathBuf::from(&self.data_dir);
        reg_location.push(DATA_SUB_DIR);
        if is_stale {
            if tokio::fs::try_exists(&reg_location).await? {
                crate::utils::execute_git(&reg_location, &["pull", "origin", RUSTSEC_DB_GIT_BRANCH]).await?;
            } else {
                tokio::fs::create_dir_all(&reg_location).await?;
                crate::utils::execute_git(
                    &reg_location,
                    &["clone", "--branch", RUSTSEC_DB_GIT_BRANCH, RUSTSEC_DB_GIT_URI, "."],
                )
                .await?;
            }
            self.last_touch = Instant::now();
            reg_location.push("crates");
            self.db.lock().unwrap().clear();
            let _results = n_at_a_time_stream(
                ReadDirStream::new(tokio::fs::read_dir(&reg_location).await?).map(|entry| {
                    let db = self.db.clone();
                    Box::pin(async move {
                        let content = tokio::fs::read(&entry?.path()).await?;
                        let advisory = serde_json::from_slice::<Advisory>(&content)?;
                        if let Ok(simple) = SimpleAdvisory::try_from(advisory) {
                            db.lock().unwrap().entry(simple.package.clone()).or_default().push(simple);
                        }
                        Ok::<_, ApiError>(())
                    })
                }),
                10,
                |r| {
                    if let Err(e) = r {
                        error!("{e}");
                        if let Some(backtrace) = &e.backtrace {
                            error!("{backtrace}");
                        }
                    }
                    false
                },
            )
            .await;
        }
        Ok(())
    }
}
