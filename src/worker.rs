/*******************************************************************************
 * Copyright (c) 2024 Cénotélie Opérations SAS (cenotelie.fr)
 ******************************************************************************/

//! Main application for worker nodes

use std::sync::Arc;
use std::time::Duration;

use axum::http::uri::InvalidUri;
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use chrono::Local;
use futures::{FutureExt, Sink, SinkExt, StreamExt, select};
use log::{error, info, warn};
use thiserror::Error;
use tokio::net::TcpStream;
use tokio::sync::Mutex;
use tokio::time::MissedTickBehavior;
use tokio_tungstenite::tungstenite::{ClientRequestBuilder, Message};
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

use crate::model::config::{Configuration, ExternalRegistry, NodeRole, NodeRoleWorker, WriteAuthConfigError};
use crate::model::docs::{DocGenJobState, DocGenJobUpdate};
use crate::model::worker::{JobSpecification, JobUpdate, WorkerDescriptor};
use crate::services::{ServiceProvider, StandardServiceProvider};
use crate::utils::apierror::ApiError;
use crate::utils::concurrent::{MaybeFutureExt, MaybeOrNever};

/// The interval between heartbeats, in milliseconds
const HEARTBEAT_INTERVAL: u64 = 100;

/// Define worker error to report.
#[derive(Debug, Error)]
pub enum WorkerError {
    #[error("expected 'worker' role config, found '{0}'")]
    RoleNotWorker(&'static str),

    #[error("failed to parse uri for connecting")]
    UriParsing(#[source] InvalidUri),

    #[error("error from controller when connecting: {0}")]
    Connecting(u16),

    #[error("failed to serialise worker descriptor")]
    WorkerDescriptionSerializing(#[source] serde_json::Error),

    #[error("failed to send Worker descriptor")]
    SendWorkerDesc(#[source] tokio_tungstenite::tungstenite::Error),

    #[error("failed to receive response to Worker Descriptor")]
    ReceiveData(#[source] tokio_tungstenite::tungstenite::Error),

    #[error("expected configuration from server, nothing was received")]
    ConfReceiving,

    #[error("failed to deserialize ExternalRegistry data")]
    DeserializeExternalRegistry(#[source] serde_json::Error),

    #[error("failed to write auth config")]
    WriteAuthConfig(#[source] WriteAuthConfigError),

    #[error("error receiving message")]
    MsgReceive(#[source] tokio_tungstenite::tungstenite::Error),

    #[error("error sending message")]
    MsgSend(#[source] tokio_tungstenite::tungstenite::Error),
}

pub async fn main_worker(config: Configuration) -> Result<(), WorkerError> {
    let descriptor = WorkerDescriptor::get_my_descriptor(&config);
    let worker_config = match &config.self_role {
        NodeRole::Standalone => return Err(WorkerError::RoleNotWorker("Standalone")),
        NodeRole::Master(_) => return Err(WorkerError::RoleNotWorker("Master")),
        NodeRole::Worker(node_role_worker) => node_role_worker,
    };

    let ws = main_worker_connect(worker_config, &descriptor).await?;
    main_loop(ws, &descriptor, config).await?;
    Ok(())
}

/// Establishes the connection to the server
async fn main_worker_connect(
    config: &NodeRoleWorker,
    descriptor: &WorkerDescriptor,
) -> Result<WebSocketStream<MaybeTlsStream<TcpStream>>, WorkerError> {
    info!("connecting to server ...");
    loop {
        let uri = format!("{}/api/v1/admin/workers/connect", config.master_uri)
            .parse()
            .map_err(WorkerError::UriParsing)?;
        let request = ClientRequestBuilder::new(uri).with_header(
            "Authorization",
            format!(
                "Basic {}",
                STANDARD.encode(format!("{}:{}", descriptor.identifier, config.worker_token))
            ),
        );
        if let Ok((ws, response)) = tokio_tungstenite::connect_async(request).await {
            if response.status().as_u16() != 101 {
                return Err(WorkerError::Connecting(response.status().as_u16()));
            }
            return Ok(ws);
        }
    }
}

async fn main_loop(
    ws: WebSocketStream<MaybeTlsStream<TcpStream>>,
    descriptor: &WorkerDescriptor,
    mut config: Configuration,
) -> Result<(), WorkerError> {
    let (mut sender, mut receiver) = ws.split();
    // handshake: send the worker's descriptor
    let worker_desc = serde_json::to_string(descriptor).map_err(WorkerError::WorkerDescriptionSerializing)?;
    sender
        .send(Message::Text(worker_desc.into()))
        .await
        .map_err(WorkerError::SendWorkerDesc)?;
    // handshake: get the connection data to the master
    let message = receiver
        .next()
        .await
        .ok_or(WorkerError::ConfReceiving)?
        .map_err(WorkerError::ReceiveData)?;
    let Message::Text(message) = message else {
        return Err(WorkerError::ConfReceiving);
    };
    let external_config =
        serde_json::from_str::<ExternalRegistry>(message.as_str()).map_err(WorkerError::DeserializeExternalRegistry)?;
    config.set_self_from_external(external_config);
    config.write_auth_config().await.map_err(WorkerError::WriteAuthConfig)?;
    let config = &config;

    info!("connected as {}-{}, waiting for jobs", descriptor.name, descriptor.identifier);
    let sender = Arc::new(Mutex::new(sender));

    let mut receiver_next = receiver.next().fuse();
    let mut current_job = MaybeOrNever::default();
    let mut heartbeat = {
        let sender = sender.clone();
        Box::pin(async move {
            let mut code: u8 = 0;
            let mut last = std::time::Instant::now();

            let mut ticks_interval = tokio::time::interval(Duration::from_millis(HEARTBEAT_INTERVAL));
            ticks_interval.set_missed_tick_behavior(MissedTickBehavior::Delay);
            loop {
                ticks_interval.tick().await;
                let elapsed = last.elapsed();
                if elapsed.as_millis() > (HEARTBEAT_INTERVAL + HEARTBEAT_INTERVAL / 2).into() {
                    warn!("heartbeat: waited too long: {}ms", elapsed.as_millis());
                }
                // send the heartbeat
                sender.lock().await.send(Message::Pong(vec![code].into())).await?;
                code = code.wrapping_add(1);
                last = std::time::Instant::now();
            }
        })
        .fuse()
    };
    loop {
        select! {
            message = receiver_next => {
                if let Some(message) = message {
                    let message = message.map_err(WorkerError::MsgReceive)?;
                    match message {
                        Message::Ping(_) | Message::Pong(_) | Message::Frame(_) => { /* do nothing */ }
                        Message::Close(_) => {
                            sender.lock().await.send(Message::Close(None)).await.map_err(WorkerError::MsgSend)?;
                            return Ok(());
                        }
                        Message::Binary(bytes) => {
                            if let Ok(job) = serde_json::from_slice::<JobSpecification>(bytes.as_ref()) {
                                current_job = Box::pin(worker_on_job(sender.clone(), job, config)).maybe();
                            }
                        }
                        Message::Text(data) => {
                            if let Ok(job) = serde_json::from_str::<JobSpecification>(data.as_str()) {
                                current_job = Box::pin(worker_on_job(sender.clone(), job, config)).maybe();
                            }
                        }
                    }
                    receiver_next = receiver.next().fuse();
                } else {
                    // end of socket
                    return Ok(());
                }
            }
            result = current_job => {
                if let Err(error) = result {
                    error!("{error}");
                    if let Some(backtrace) = &error.backtrace {
                        error!("{backtrace}");
                    }
                }
                current_job = MaybeOrNever::default();
            }
            result = heartbeat => {
                let result: Result<(), ApiError> = result;
                if let Err(error) = result {
                    error!("{error}");
                    if let Some(backtrace) = &error.backtrace {
                        error!("{backtrace}");
                    }
                }
            }
        }
    }
}

/// The main payload when a job was received
async fn worker_on_job<S>(sender: Arc<Mutex<S>>, job: JobSpecification, config: &Configuration) -> Result<(), ApiError>
where
    S: Sink<Message, Error = tokio_tungstenite::tungstenite::Error> + Unpin,
{
    let JobSpecification::DocGen(job) = job;
    let service_storage = StandardServiceProvider::get_storage(config);
    match crate::services::docs::generate_doc_for_job(config, service_storage, &job).await {
        Ok((state, log)) => {
            let now = Local::now().naive_local();
            sender
                .lock()
                .await
                .send(Message::Text(
                    serde_json::to_string(&JobUpdate::DocGen(DocGenJobUpdate {
                        job_id: job.id,
                        state,
                        last_update: now,
                        log: Some(log),
                    }))?
                    .into(),
                ))
                .await?;
        }
        Err(error) => {
            let now = Local::now().naive_local();
            sender
                .lock()
                .await
                .send(Message::Text(
                    serde_json::to_string(&JobUpdate::DocGen(DocGenJobUpdate {
                        job_id: job.id,
                        state: DocGenJobState::Failure,
                        last_update: now,
                        log: Some(format!("{error}")),
                    }))?
                    .into(),
                ))
                .await?;
        }
    }
    Ok(())
}
