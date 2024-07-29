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
use tokio_stream::wrappers::ReadDirStream;

use crate::model::config::Configuration;
use crate::model::osv::{Advisory, SimpleAdvisory};
use crate::utils::apierror::ApiError;
use crate::utils::concurrent::n_at_a_time_stream;
use crate::utils::stale_instant;

/// Service to use the [RustSec](https://github.com/rustsec) data about crates
#[derive(Debug, Clone)]
pub struct RustSecData {
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

impl Default for RustSecData {
    fn default() -> Self {
        RustSecData {
            // last_touch set as 7 days before
            last_touch: stale_instant(),
            db: Arc::new(std::sync::Mutex::new(HashMap::new())),
        }
    }
}

impl RustSecData {
    /// Updates the data
    async fn update_data(&mut self, config: &Configuration) -> Result<(), ApiError> {
        let now = Instant::now();
        let is_stale = now.duration_since(self.last_touch) > Duration::from_millis(config.deps_stale_registry);
        let mut reg_location = PathBuf::from(&config.data_dir);
        reg_location.push(DATA_SUB_DIR);
        if is_stale {
            if tokio::fs::try_exists(&reg_location).await? {
                super::index::execute_git(&reg_location, &["pull", "origin", RUSTSEC_DB_GIT_BRANCH]).await?;
            } else {
                tokio::fs::create_dir_all(&reg_location).await?;
                super::index::execute_git(
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

pub struct RustSecChecker<'a> {
    /// The data for the service
    pub data: &'a Mutex<RustSecData>,
    /// The app configuration
    pub configuration: &'a Configuration,
}

impl<'a> RustSecChecker<'a> {
    /// Gets the advisories against a crate
    pub async fn check_crate(&self, package: &str, version: &semver::Version) -> Result<Vec<SimpleAdvisory>, ApiError> {
        let mut data = self.data.lock().await;
        data.update_data(self.configuration).await?;
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
    }
}
