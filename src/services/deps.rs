/*******************************************************************************
 * Copyright (c) 2024 Cénotélie Opérations SAS (cenotelie.fr)
 ******************************************************************************/

//! Service to fetch data about dependency crates

use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use futures::lock::Mutex;
use semver::{Version, VersionReq};
use tokio::fs::File;
use tokio::io::AsyncBufReadExt;

use crate::model::config::{Configuration, ExternalRegistryProtocol};
use crate::model::deps::DependencyInfo;
use crate::model::objects::{CrateMetadataIndex, DependencyIndex};
use crate::services::index::Index;
use crate::utils::apierror::{error_backend_failure, error_not_found, specialize, ApiError};

/// Service to check the dependencies of a crate
#[derive(Debug, Clone, Default)]
pub struct DependencyChecker {
    /// The last time a piece of data was touched
    last_touch: HashMap<String, Instant>,
}

/// Access to the service to check the dependencies of a crate
pub struct DependencyCheckerAccess<'a> {
    /// The data for the service
    pub data: &'a Mutex<DependencyChecker>,
    /// The app configuration
    pub configuration: &'a Configuration,
    /// Access to the index
    pub index: &'a Mutex<Index>,
}

/// The URI identifying crates.io as the registry for a dependency
const CRATES_IO_REGISTRY_URI: &str = "https://github.com/rust-lang/crates.io-index";
/// The prefis URI for the index for dependencies on crates.io
const CRATES_IO_INDEX_URI: &str = "https://index.crates.io/";
/// Registry name for crates.io
const CRATES_IO_NAME: &str = "crates.io";
/// Name of the sub-directory to use within the data directory
const DATA_SUB_DIR: &str = "deps";

impl<'a> DependencyCheckerAccess<'a> {
    /// Checks the dependencies of a local crate
    pub async fn check_crate(&self, package: &str, version: &str) -> Result<Vec<DependencyInfo>, ApiError> {
        let metadata = self.index.lock().await.get_crate_data(package).await?;
        let metadata = metadata
            .iter()
            .find(|meta| meta.vers == version)
            .ok_or_else(error_not_found)?;

        let mut results = Vec::new();
        for dep in &metadata.deps {
            results.push(self.get_dependency_info(dep).await?);
        }

        Ok(results)
    }

    /// Gets the information about a dependency
    async fn get_dependency_info(&self, dep: &DependencyIndex) -> Result<DependencyInfo, ApiError> {
        let requirement = dep.req.parse::<VersionReq>()?;
        let versions = self.get_dependency_versions(dep).await?;
        let versions = versions
            .into_iter()
            .map(|v| v.vers.parse::<Version>())
            .collect::<Result<Vec<_>, _>>()?;
        let last_version = versions.iter().max().unwrap();
        Ok(DependencyInfo {
            registry: dep.registry.clone(),
            package: dep.name.clone(),
            required: dep.req.clone(),
            kind: dep.kind.parse().unwrap(),
            last_version: last_version.to_string(),
            is_outdated: !requirement.matches(last_version),
        })
    }

    /// Retrieves the versions of a dependency
    async fn get_dependency_versions(&self, dep: &DependencyIndex) -> Result<Vec<CrateMetadataIndex>, ApiError> {
        if let Some(registry) = &dep.registry {
            if registry == CRATES_IO_REGISTRY_URI {
                self.get_dependency_info_sparse(&dep.name, CRATES_IO_NAME, CRATES_IO_INDEX_URI, None)
                    .await
            } else if let Some(registry) = self
                .configuration
                .external_registries
                .iter()
                .find(|reg| &reg.index == registry)
            {
                match registry.protocol {
                    ExternalRegistryProtocol::Git => {
                        self.get_dependency_info_git(&dep.name, &registry.name, &registry.index).await
                    }
                    ExternalRegistryProtocol::Sparse => {
                        self.get_dependency_info_sparse(
                            &dep.name,
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
            // same registry, lookup in internal intex
            self.index.lock().await.get_crate_data(&dep.name).await
        }
    }

    /// Gets the crate index data for a dependency in a registry with the git protocol
    async fn get_dependency_info_git(
        &self,
        dep_name: &str,
        reg_name: &str,
        index_uri: &str,
    ) -> Result<Vec<CrateMetadataIndex>, ApiError> {
        let mut data = self.data.lock().await;

        let now = Instant::now();
        let last_touch = data
            .last_touch
            .get(reg_name)
            .copied()
            .unwrap_or(now.checked_sub(now.elapsed()).unwrap());
        let is_stale = now.duration_since(last_touch) > Duration::from_millis(self.configuration.deps_analysis_stale_period);

        let mut reg_location = PathBuf::from(&self.configuration.data_dir);
        reg_location.push(DATA_SUB_DIR);
        reg_location.push(reg_name);
        if is_stale {
            if tokio::fs::try_exists(&reg_location).await? {
                super::index::execute_git(&reg_location, &["pull", "origin", "master"]).await?;
            } else {
                tokio::fs::create_dir_all(&reg_location).await?;
                super::index::execute_git(&reg_location, &["clone", index_uri, "."]).await?;
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
        let file_path = super::index::build_package_file_path(reg_location, dep_name);
        tokio::fs::create_dir_all(file_path.parent().unwrap()).await?;
        Ok(file_path)
    }

    /// Build the target URI to be used to retrieve the last data and store the access timestamp
    fn get_dependency_info_sparse_target_uri(dep_name: &str, index_uri: &str) -> String {
        let lowercase = dep_name.to_ascii_lowercase();
        let (first, second) = super::index::package_file_path(&lowercase);
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
    ) -> Result<Vec<CrateMetadataIndex>, ApiError> {
        let target_uri = Self::get_dependency_info_sparse_target_uri(dep_name, index_uri);
        let file_path = self.get_dependency_info_file_path(dep_name, reg_name).await?;

        let mut data = self.data.lock().await;

        if tokio::fs::try_exists(&file_path).await? {
            // is it stale?
            let now = Instant::now();
            let last_touch = data
                .last_touch
                .get(&target_uri)
                .copied()
                .unwrap_or(now.checked_sub(now.elapsed()).unwrap());
            let is_stale =
                now.duration_since(last_touch) > Duration::from_millis(self.configuration.deps_analysis_stale_period);
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
        data: &mut DependencyChecker,
    ) -> Result<Vec<CrateMetadataIndex>, ApiError> {
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
