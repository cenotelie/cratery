/*******************************************************************************
 * Copyright (c) 2024 Cénotélie Opérations SAS (cenotelie.fr)
 ******************************************************************************/

//! Data model for worker nodes and the protocol to communicate between the master and the workers

use std::fmt::Display;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, RwLock};
use std::task::{Context, Poll, Waker};

use log::{error, info};
use serde_derive::{Deserialize, Serialize};
use thiserror::Error;
use tokio::sync::Mutex;
use tokio::sync::mpsc::{Receiver, Sender};

use super::docs::{DocGenJob, DocGenJobUpdate};
use crate::model::config::{Configuration, NodeRole};
use crate::utils::apierror::ApiError;
use crate::utils::token::generate_token;

/// The descriptor of a worker and its capabilities
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct WorkerDescriptor {
    /// The unique identifier for the worker
    pub identifier: String,
    /// The user-friendly name of the worker
    pub name: String,
    /// The version of the locally installed toolchain
    #[serde(rename = "toolchainVersionStable")]
    pub toolchain_version_stable: semver::Version,
    /// The version of the locally installed toolchain
    #[serde(rename = "toolchainVersionNightly")]
    pub toolchain_version_nightly: semver::Version,
    /// The host target of the locally installed toolchain
    #[serde(rename = "toolchainHost")]
    pub toolchain_host: String,
    /// The locally installed targets
    #[serde(rename = "toolchainInstalledTargets")]
    pub toolchain_installed_targets: Vec<String>,
    /// All the potential targets that the node could install
    #[serde(rename = "toolchainInstallableTargets")]
    pub toolchain_installable_targets: Vec<String>,
    /// The declared capabilities of the worker
    pub capabilities: Vec<String>,
}

impl WorkerDescriptor {
    /// Gets the descriptor for this worker, base of the specified configuration
    #[must_use]
    pub fn get_my_descriptor(config: &Configuration) -> Self {
        Self {
            identifier: generate_token(32),
            name: if let NodeRole::Worker(worker_config) = &config.self_role {
                worker_config.name.clone()
            } else {
                String::new()
            },
            toolchain_version_stable: config.self_toolchain_version_stable.clone(),
            toolchain_version_nightly: config.self_toolchain_version_nightly.clone(),
            toolchain_host: config.self_toolchain_host.clone(),
            toolchain_installed_targets: config.self_installed_targets.clone(),
            toolchain_installable_targets: if config.docs_autoinstall_targets {
                config.self_installable_targets.clone()
            } else {
                Vec::new()
            },
            capabilities: if let NodeRole::Worker(worker_config) = &config.self_role {
                worker_config.capabilities.clone()
            } else {
                Vec::new()
            },
        }
    }

    /// Gets whether this worker matches the selector
    #[must_use]
    pub fn matches(&self, selector: &WorkerSelector) -> bool {
        self.match_host(selector)
            && self.match_installed_target(selector)
            && self.match_available_target(selector)
            && selector.capabilities.iter().all(|cap| self.capabilities.contains(cap))
    }

    #[must_use]
    fn match_host(&self, selector: &WorkerSelector) -> bool {
        selector
            .toolchain_host
            .as_ref()
            .is_none_or(|host| &self.toolchain_host == host)
    }

    #[must_use]
    fn match_installed_target(&self, selector: &WorkerSelector) -> bool {
        selector
            .toolchain_installed_target
            .as_ref()
            .is_none_or(|target| self.toolchain_installed_targets.contains(target))
    }

    #[must_use]
    fn match_available_target(&self, selector: &WorkerSelector) -> bool {
        selector.toolchain_available_target.as_ref().is_none_or(|target| {
            self.toolchain_installed_targets.contains(target) || self.toolchain_installable_targets.contains(target)
        })
    }
}

/// The data for registering a worker
#[derive(Debug)]
pub struct WorkerRegistrationData {
    /// The worker's description
    pub descriptor: WorkerDescriptor,
    /// The sender to send jobs to the worker
    pub job_sender: Sender<JobSpecification>,
    /// The receiver to receive updates from the worker
    pub update_receiver: Receiver<JobUpdate>,
}

/// The state of a worker
#[derive(Debug)]
enum WorkerState {
    /// Available for jobs
    Available(Receiver<JobUpdate>),
    /// In use for a job
    InUse(JobIdentifier),
}

impl WorkerState {
    /// Checks whether the worker is available
    #[must_use]
    pub const fn is_available(&self) -> bool {
        matches!(self, Self::Available(_))
    }
}

/// The state of a worker
#[derive(Debug, Serialize, Deserialize, Clone, Copy, PartialEq, Eq)]
pub enum WorkerPublicState {
    /// Available for jobs
    Available,
    /// In use for a job
    InUse(JobIdentifier),
}

impl From<&'_ WorkerState> for WorkerPublicState {
    fn from(value: &WorkerState) -> Self {
        match value {
            WorkerState::Available(_) => Self::Available,
            WorkerState::InUse(job_id) => Self::InUse(*job_id),
        }
    }
}

/// The data for a worker
#[derive(Debug)]
struct WorkerData {
    /// The worker's description
    descriptor: WorkerDescriptor,
    /// The sender to send jobs to the worker
    job_sender: Sender<JobSpecification>,
    /// The worker's state
    state: WorkerState,
}

impl WorkerData {
    /// Checkouts this worker
    ///
    /// # Panics
    ///
    /// Raise a panic when the worker was not available
    #[must_use]
    fn checkout(&mut self, job_id: JobIdentifier) -> Receiver<JobUpdate> {
        let mut old_state = WorkerState::InUse(job_id);
        std::mem::swap(&mut self.state, &mut old_state);
        let WorkerState::Available(receiver) = old_state else {
            panic!("expected an available worker")
        };
        receiver
    }
}

/// The data for a worker
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct WorkerPublicData {
    /// The worker's description
    pub descriptor: WorkerDescriptor,
    /// The worker's state
    pub state: WorkerPublicState,
}

impl<'a> From<&'a WorkerData> for WorkerPublicData {
    fn from(value: &'a WorkerData) -> Self {
        Self {
            descriptor: value.descriptor.clone(),
            state: WorkerPublicState::from(&value.state),
        }
    }
}

/// Data to select a suitable worker
#[derive(Debug, Default, Clone)]
pub struct WorkerSelector {
    /// Requires a specific native target
    pub toolchain_host: Option<String>,
    /// Requires a target to be installed
    pub toolchain_installed_target: Option<String>,
    /// Requires a target to be available, but not necessarily installed
    pub toolchain_available_target: Option<String>,
    /// All the required capabilities
    pub capabilities: Vec<String>,
}

impl WorkerSelector {
    /// Builds a selector that requires a native host for a target
    #[must_use]
    pub const fn new_native_target(target: String) -> Self {
        Self {
            toolchain_host: Some(target),
            toolchain_installed_target: None,
            toolchain_available_target: None,
            capabilities: Vec::new(),
        }
    }

    /// Builds a selector that requires a target to be available
    #[must_use]
    pub const fn new_available_target(target: String) -> Self {
        Self {
            toolchain_host: None,
            toolchain_installed_target: None,
            toolchain_available_target: Some(target),
            capabilities: Vec::new(),
        }
    }
}

impl Display for WorkerSelector {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut first = true;
        write!(f, "(")?;
        if let Some(host) = &self.toolchain_host {
            write!(f, "host={host}")?;
            first = false;
        }
        if let Some(target) = &self.toolchain_installed_target {
            if !first {
                write!(f, ", ")?;
            }
            write!(f, "installed target={target}")?;
            first = false;
        }
        if let Some(target) = &self.toolchain_available_target {
            if !first {
                write!(f, ", ")?;
            }
            write!(f, "available target={target}")?;
            first = false;
        }
        if !self.capabilities.is_empty() {
            if !first {
                write!(f, ", ")?;
            }
            write!(f, "capabilities=")?;
            first = true;
            for capability in &self.capabilities {
                if !first {
                    write!(f, ",")?;
                }
                write!(f, "{capability}")?;
            }
        }
        write!(f, ")")?;
        Ok(())
    }
}

/// Error when no worker matches the selector
#[derive(Debug, Clone, Error)]
#[error("no connected worker matches the selector: {selector}")]
pub struct NoMatchingWorkerError {
    /// The selector that was used
    pub selector: WorkerSelector,
}

/// Wait for a worker
pub struct WorkerWaiter {
    /// The parent manager
    manager: WorkersManager,
    /// The selector to use
    selector: WorkerSelector,
    /// The identifier of the waiting job
    job_id: JobIdentifier,
    /// The resolved worker if any
    worker: Option<WorkerCheckout>,
}

impl Future for WorkerWaiter {
    type Output = Result<WorkerCheckout, NoMatchingWorkerError>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        if let Some(data) = self.worker.take() {
            self.manager.send_event(WorkerEvent::WorkerStartedJob {
                worker_id: data.descriptor.identifier.clone(),
                job_id: self.job_id,
            });
            return Poll::Ready(Ok(data));
        }
        let mut inner = self.manager.inner.write().unwrap();
        match self
            .manager
            .clone()
            .try_get_worker_for(&mut inner, &self.selector, self.job_id)
        {
            Ok(Some(worker)) => {
                self.manager.send_event(WorkerEvent::WorkerStartedJob {
                    worker_id: worker.descriptor.identifier.clone(),
                    job_id: self.job_id,
                });
                Poll::Ready(Ok(worker))
            }
            Ok(None) => {
                // queue
                inner.queue.push(QueuedRequest {
                    selector: self.selector.clone(),
                    waker: cx.waker().clone(),
                });
                Poll::Pending
            }
            Err(error) => Poll::Ready(Err(error)),
        }
    }
}

/// A checkout for a worker while it is in use
#[derive(Debug)]
pub struct WorkerCheckout {
    /// The parent manager
    manager: WorkersManager,
    /// The worker's description
    descriptor: WorkerDescriptor,
    /// The sender to send jobs to the worker
    job_sender: Sender<JobSpecification>,
    /// The receiver to receive updates from the worker
    update_receiver: Option<Receiver<JobUpdate>>,
}

impl WorkerCheckout {
    /// Gets the job sender
    pub const fn sender(&mut self) -> &mut Sender<JobSpecification> {
        &mut self.job_sender
    }

    /// Gets the update receiver
    pub const fn update_receiver(&mut self) -> &mut Receiver<JobUpdate> {
        self.update_receiver.as_mut().unwrap()
    }
}

impl Drop for WorkerCheckout {
    fn drop(&mut self) {
        self.manager.clone().put_worker_back(self);
    }
}

/// The data of a queued request for a worker
#[derive(Debug)]
struct QueuedRequest {
    /// The associated selector
    selector: WorkerSelector,
    /// The waker
    waker: Waker,
}

/// The inner data for a manager of workers
#[derive(Debug, Default)]
struct WorkersManagerInner {
    /// The workers themselves
    workers: Vec<WorkerData>,
    /// The queue of requests remaining to be solved
    queue: Vec<QueuedRequest>,
}

/// The manager of worker
#[derive(Debug, Default, Clone)]
pub struct WorkersManager {
    /// The inner data
    inner: Arc<RwLock<WorkersManagerInner>>,
    /// The active listeners
    listeners: Arc<Mutex<Vec<Sender<WorkerEvent>>>>,
}

impl WorkersManager {
    /// Gets whether there are connected workers
    #[must_use]
    pub fn has_workers(&self) -> bool {
        !self.inner.read().unwrap().workers.is_empty()
    }

    /// Gets all the registered workers
    #[must_use]
    pub fn get_workers(&self) -> Vec<WorkerPublicData> {
        self.inner
            .read()
            .unwrap()
            .workers
            .iter()
            .map(WorkerPublicData::from)
            .collect::<Vec<_>>()
    }

    /// Registers a new worker
    pub fn register_worker(&self, data: WorkerRegistrationData) {
        info!("=== registering worker {}", data.descriptor.identifier);
        let worker_data = WorkerData {
            descriptor: data.descriptor,
            job_sender: data.job_sender,
            state: WorkerState::Available(data.update_receiver),
        };
        let event = WorkerEvent::WorkerConnected(Box::new(WorkerPublicData::from(&worker_data)));
        self.inner.write().unwrap().workers.push(worker_data);
        self.send_event(event);
    }

    /// Remove a worker
    #[expect(clippy::significant_drop_tightening)]
    pub fn remove_worker(&self, worker_id: &str) {
        info!("=== removing worker {worker_id}");
        let found = {
            let mut inner = self.inner.write().unwrap();
            let size_before = inner.workers.len();
            inner.workers.retain(|w| w.descriptor.identifier != worker_id);
            let size_after = inner.workers.len();
            size_before != size_after
        };
        if found {
            self.send_event(WorkerEvent::WorkerRemoved {
                worker_id: worker_id.to_string(),
            });
        }
    }

    /// Gets a worker for a selector
    pub fn get_worker_for(
        &self,
        selector: WorkerSelector,
        job_id: JobIdentifier,
    ) -> Result<WorkerWaiter, NoMatchingWorkerError> {
        let worker = self
            .clone()
            .try_get_worker_for(&mut self.inner.write().unwrap(), &selector, job_id)?;
        Ok(WorkerWaiter {
            manager: self.clone(),
            selector,
            job_id,
            worker,
        })
    }

    /// Put back a worker as available
    fn put_worker_back(&self, checkout: &mut WorkerCheckout) {
        let mut inner = self.inner.write().unwrap();

        let index = if let Some((index, worker)) = inner
            .workers
            .iter_mut()
            .enumerate()
            .find(|(_, w)| w.descriptor.identifier == checkout.descriptor.identifier)
        {
            worker.state = WorkerState::Available(checkout.update_receiver.take().unwrap());
            self.send_event(WorkerEvent::WorkerAvailable {
                worker_id: worker.descriptor.identifier.clone(),
            });
            Some(index)
        } else {
            None
        };

        if let Some(worker_index) = index {
            // is the worker usable for a specific queued request
            let index = inner.queue.iter().enumerate().find_map(|(index, item)| {
                if inner.workers[worker_index].descriptor.matches(&item.selector) {
                    Some(index)
                } else {
                    None
                }
            });
            if let Some(index) = index {
                let item = inner.queue.remove(index);
                drop(inner);
                item.waker.wake();
            }
        }
    }

    /// Attempts to get a worker for a selector
    fn try_get_worker_for(
        self,
        inner: &mut WorkersManagerInner,
        selector: &WorkerSelector,
        job_id: JobIdentifier,
    ) -> Result<Option<WorkerCheckout>, NoMatchingWorkerError> {
        let mut at_least_one = false;
        for candidate in inner.workers.iter_mut().filter(|w| w.descriptor.matches(selector)) {
            at_least_one = true;
            if candidate.state.is_available() {
                return Ok(Some(WorkerCheckout {
                    manager: self,
                    descriptor: candidate.descriptor.clone(),
                    job_sender: candidate.job_sender.clone(),
                    update_receiver: Some(candidate.checkout(job_id)),
                }));
            }
        }
        if at_least_one {
            Ok(None)
        } else {
            Err(NoMatchingWorkerError {
                selector: selector.clone(),
            })
        }
    }

    /// Adds a listener to job updates
    pub async fn add_listener(&self, listener: tokio::sync::mpsc::Sender<WorkerEvent>) {
        self.listeners.lock().await.push(listener);
    }

    /// Send an event to listeners, do not block
    fn send_event(&self, event: WorkerEvent) {
        let this = self.clone();
        tokio::spawn(async move {
            if let Err(e) = this.do_send_event(event).await {
                error!("{e}");
                if let Some(backtrace) = &e.backtrace {
                    error!("{backtrace}");
                }
            }
        });
    }

    /// Send an event to listeners
    #[expect(clippy::significant_drop_tightening)]
    async fn do_send_event(&self, event: WorkerEvent) -> Result<(), ApiError> {
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
}

/// The identifier of a job for a worker
#[derive(Debug, Serialize, Deserialize, Clone, Copy, PartialEq, Eq)]
pub enum JobIdentifier {
    /// A documentation generation job
    DocGen(i64),
}

/// An specification of an job to be executed
#[derive(Debug, Serialize, Deserialize, Clone)]
pub enum JobSpecification {
    /// A documentation generation job
    DocGen(DocGenJob),
}

impl JobSpecification {
    /// Gets the job identifier
    #[must_use]
    pub const fn get_id(&self) -> JobIdentifier {
        match self {
            Self::DocGen(doc_gen_job) => JobIdentifier::DocGen(doc_gen_job.id),
        }
    }
}

/// An update about the execution of a job, for the client
#[derive(Debug, Serialize, Deserialize, Clone)]
pub enum JobUpdate {
    /// An update about a documentation generation job
    DocGen(DocGenJobUpdate),
}

/// An event about workers
#[derive(Debug, Serialize, Deserialize, Clone)]
pub enum WorkerEvent {
    /// A worker just connected
    WorkerConnected(Box<WorkerPublicData>),
    /// A worker was removed
    WorkerRemoved { worker_id: String },
    /// A worker started a new job
    WorkerStartedJob { worker_id: String, job_id: JobIdentifier },
    /// A worker became available
    WorkerAvailable { worker_id: String },
}

#[cfg(test)]
mod tests {
    use crate::model::worker::WorkerSelector;

    use super::WorkerDescriptor;

    #[test]
    fn worker_host_match() {
        let selector = WorkerSelector::new_native_target("stable-x86_64-unknown-linux-gnu".into());
        let worker_desk = WorkerDescriptor {
            identifier: String::default(),
            name: String::default(),
            toolchain_version_stable: semver::Version::new(1, 89, 0),
            toolchain_version_nightly: semver::Version::new(1, 91, 0),
            toolchain_host: "stable-x86_64-unknown-linux-gnu".into(),
            toolchain_installed_targets: Vec::new(),
            toolchain_installable_targets: Vec::new(),
            capabilities: Vec::new(),
        };
        assert!(worker_desk.matches(&selector));
    }

    #[test]
    fn worker_host_not_match() {
        let selector = WorkerSelector::new_native_target("stable-x86_64-unknown-linux-gnu".into());
        let worker_desk = WorkerDescriptor {
            identifier: String::default(),
            name: String::default(),
            toolchain_version_stable: semver::Version::new(1, 89, 0),
            toolchain_version_nightly: semver::Version::new(1, 91, 0),
            toolchain_host: "1.89-x86_64-unknown-linux-gnu".into(),
            toolchain_installed_targets: Vec::new(),
            toolchain_installable_targets: Vec::new(),
            capabilities: Vec::new(),
        };
        assert!(!worker_desk.matches(&selector));
    }

    #[test]
    fn worker_target_installed_match() {
        let selector = WorkerSelector::new_available_target("x86_64-unknown-linux-gnu".into());
        let worker_desk = WorkerDescriptor {
            identifier: String::default(),
            name: String::default(),
            toolchain_version_stable: semver::Version::new(1, 89, 0),
            toolchain_version_nightly: semver::Version::new(1, 91, 0),
            toolchain_host: "1.89-x86_64-unknown-linux-gnu".into(),
            toolchain_installed_targets: vec!["x86_64-unknown-linux-gnu".to_string()],
            toolchain_installable_targets: Vec::new(),
            capabilities: Vec::new(),
        };
        assert!(worker_desk.matches(&selector));
    }

    #[test]
    fn worker_target_installed_not_match() {
        let selector = WorkerSelector::new_available_target("x86_64-unknown-linux-gnu".into());
        let worker_desk = WorkerDescriptor {
            identifier: String::default(),
            name: String::default(),
            toolchain_version_stable: semver::Version::new(1, 89, 0),
            toolchain_version_nightly: semver::Version::new(1, 91, 0),
            toolchain_host: "1.89-x86_64-unknown-linux-gnu".into(),
            toolchain_installed_targets: vec!["wasm32-unknown-unknown".to_string()],
            toolchain_installable_targets: Vec::new(),
            capabilities: Vec::new(),
        };
        assert!(!worker_desk.matches(&selector));
    }

    #[test]
    fn work_target_installable_match() {
        let selector = WorkerSelector::new_available_target("aarch64-unknown-linux-musl".into());
        let worker_desk = WorkerDescriptor {
            identifier: String::default(),
            name: String::default(),
            toolchain_version_stable: semver::Version::new(1, 89, 0),
            toolchain_version_nightly: semver::Version::new(1, 91, 0),
            toolchain_host: String::new(),
            toolchain_installed_targets: Vec::new(),
            toolchain_installable_targets: vec!["aarch64-unknown-linux-musl".to_string(), "wasm32-unknown-unknown".to_string()],
            capabilities: Vec::new(),
        };
        assert!(worker_desk.matches(&selector));
    }

    #[test]
    fn work_target_installable_not_match() {
        let selector = WorkerSelector::new_available_target("aarch64-linux-android".into());
        let worker_desk = WorkerDescriptor {
            identifier: String::default(),
            name: String::default(),
            toolchain_version_stable: semver::Version::new(1, 89, 0),
            toolchain_version_nightly: semver::Version::new(1, 91, 0),
            toolchain_host: String::new(),
            toolchain_installed_targets: Vec::new(),
            toolchain_installable_targets: vec!["aarch64-unknown-linux-musl".to_string(), "wasm32-unknown-unknown".to_string()],
            capabilities: Vec::new(),
        };
        assert!(!worker_desk.matches(&selector));
    }
}
