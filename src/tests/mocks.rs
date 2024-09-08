/*******************************************************************************
 * Copyright (c) 2024 Cénotélie Opérations SAS (cenotelie.fr)
 ******************************************************************************/

//! Types and utilities for mocking services

use std::env::temp_dir;
use std::path::PathBuf;
use std::sync::Arc;

use chrono::NaiveDateTime;
use semver::Version;
use tokio::sync::mpsc::Sender;

use crate::model::cargo::{CrateMetadata, IndexCrateMetadata};
use crate::model::config::Configuration;
use crate::model::deps::DepsAnalysis;
use crate::model::docs::{DocGenEvent, DocGenJob, DocGenJobState, DocGenTrigger};
use crate::model::osv::SimpleAdvisory;
use crate::model::JobCrate;
use crate::services::deps::DepsChecker;
use crate::services::docs::DocsGenerator;
use crate::services::emails::EmailSender;
use crate::services::index::Index;
use crate::services::rustsec::RustSecChecker;
use crate::services::storage::Storage;
use crate::services::ServiceProvider;
use crate::utils::apierror::ApiError;
use crate::utils::db::RwSqlitePool;
use crate::utils::token::generate_token;
use crate::utils::FaillibleFuture;

/// A mocking service
pub struct MockService;

fn resolved_default<T: Default + Send>() -> FaillibleFuture<'static, T> {
    Box::pin(async { Ok(T::default()) })
}

impl ServiceProvider for MockService {
    /// Gets the configuration
    async fn get_configuration() -> Result<Configuration, ApiError> {
        let mut temp_dir = temp_dir();
        temp_dir.push(format!("cratery-test-{}", generate_token(16)));
        tokio::fs::create_dir_all(&temp_dir).await?;
        Ok(Configuration {
            data_dir: temp_dir.to_str().unwrap().to_string(),
            ..Default::default()
        })
    }

    fn get_storage(_config: &Configuration) -> Arc<dyn Storage + Send + Sync> {
        Arc::new(MockService)
    }

    async fn get_index(_config: &Configuration) -> Result<Arc<dyn Index + Send + Sync>, ApiError> {
        Ok(Arc::new(MockService))
    }

    fn get_rustsec(_config: &Configuration) -> Arc<dyn RustSecChecker + Send + Sync> {
        Arc::new(MockService)
    }

    fn get_deps_checker(
        _configuration: Arc<Configuration>,
        _service_index: Arc<dyn Index + Send + Sync>,
        _service_rustsec: Arc<dyn RustSecChecker + Send + Sync>,
    ) -> Arc<dyn DepsChecker + Send + Sync> {
        Arc::new(MockService)
    }

    fn get_email_sender(_config: Arc<Configuration>) -> Arc<dyn EmailSender + Send + Sync> {
        Arc::new(MockService)
    }

    fn get_docs_generator(
        _configuration: Arc<Configuration>,
        _service_db_pool: RwSqlitePool,
        _service_storage: Arc<dyn Storage + Send + Sync>,
    ) -> Arc<dyn DocsGenerator + Send + Sync> {
        Arc::new(MockService)
    }
}

impl Index for MockService {
    fn get_index_file<'a>(&'a self, _file_path: &'a std::path::Path) -> FaillibleFuture<'a, Option<PathBuf>> {
        resolved_default()
    }

    fn get_upload_pack_info_refs(&self) -> FaillibleFuture<'_, Vec<u8>> {
        resolved_default()
    }

    fn get_upload_pack_for<'a>(&'a self, _input: &'a [u8]) -> FaillibleFuture<'a, Vec<u8>> {
        resolved_default()
    }

    fn publish_crate_version<'a>(&'a self, _metadata: &'a IndexCrateMetadata) -> FaillibleFuture<'a, ()> {
        resolved_default()
    }

    fn get_crate_data<'a>(&'a self, _package: &'a str) -> FaillibleFuture<'a, Vec<IndexCrateMetadata>> {
        resolved_default()
    }
}

impl DepsChecker for MockService {
    fn precache_crate_io(&self) -> FaillibleFuture<'_, ()> {
        resolved_default()
    }

    fn check_crate<'a>(
        &'a self,
        _package: &'a str,
        _version: &'a str,
        _targets: &'a [String],
    ) -> FaillibleFuture<'a, DepsAnalysis> {
        resolved_default()
    }
}

impl DocsGenerator for MockService {
    fn get_jobs(&self) -> FaillibleFuture<'_, Vec<DocGenJob>> {
        resolved_default()
    }

    fn get_job_log(&self, _job_id: i64) -> FaillibleFuture<'_, String> {
        resolved_default()
    }

    fn queue<'a>(&'a self, _spec: &'a JobCrate, trigger: &'a DocGenTrigger) -> FaillibleFuture<'a, DocGenJob> {
        Box::pin(async {
            Ok(DocGenJob {
                id: -1,
                package: String::new(),
                version: String::new(),
                targets: Vec::new(),
                state: DocGenJobState::Queued,
                queued_on: NaiveDateTime::default(),
                started_on: NaiveDateTime::default(),
                finished_on: NaiveDateTime::default(),
                last_update: NaiveDateTime::default(),
                trigger: trigger.clone(),
            })
        })
    }

    fn add_listener(&self, _listener: Sender<DocGenEvent>) -> FaillibleFuture<'_, ()> {
        resolved_default()
    }
}

impl EmailSender for MockService {
    fn send_email<'a>(&'a self, _to: &'a [String], _subject: &'a str, _body: String) -> FaillibleFuture<'a, ()> {
        resolved_default()
    }
}

impl RustSecChecker for MockService {
    fn check_crate<'a>(&'a self, _package: &'a str, _version: &'a Version) -> FaillibleFuture<'a, Vec<SimpleAdvisory>> {
        resolved_default()
    }
}

impl Storage for MockService {
    fn store_crate<'a>(&'a self, _metadata: &'a CrateMetadata, _content: Vec<u8>) -> FaillibleFuture<'a, ()> {
        resolved_default()
    }

    fn download_crate<'a>(&'a self, _name: &'a str, _version: &'a str) -> FaillibleFuture<'a, Vec<u8>> {
        resolved_default()
    }

    fn download_crate_metadata<'a>(&'a self, _name: &'a str, _version: &'a str) -> FaillibleFuture<'a, Option<CrateMetadata>> {
        resolved_default()
    }

    fn download_crate_readme<'a>(&'a self, _name: &'a str, _version: &'a str) -> FaillibleFuture<'a, Vec<u8>> {
        resolved_default()
    }

    fn store_doc_file<'a>(&'a self, _path: &'a str, _file: &'a std::path::Path) -> FaillibleFuture<'a, ()> {
        resolved_default()
    }

    fn store_doc_data<'a>(&'a self, _path: &'a str, _content: Vec<u8>) -> FaillibleFuture<'a, ()> {
        resolved_default()
    }

    fn download_doc_file<'a>(&'a self, _path: &'a str) -> FaillibleFuture<'a, Vec<u8>> {
        resolved_default()
    }
}
