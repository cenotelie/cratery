/*******************************************************************************
 * Copyright (c) 2024 Cénotélie Opérations SAS (cenotelie.fr)
 ******************************************************************************/

//! Main application for worker nodes

use std::sync::Arc;
use std::time::Duration;

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use chrono::Local;
use futures::{select, FutureExt, Sink, SinkExt, StreamExt};
use log::{error, info, warn};
use tokio::net::TcpStream;
use tokio::sync::Mutex;
use tokio::time::MissedTickBehavior;
use tokio_tungstenite::tungstenite::{ClientRequestBuilder, Message};
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

use crate::model::config::{Configuration, ExternalRegistry, NodeRole, NodeRoleWorker};
use crate::model::docs::{DocGenJobState, DocGenJobUpdate};
use crate::model::worker::{JobSpecification, JobUpdate, WorkerDescriptor};
use crate::services::{ServiceProvider, StandardServiceProvider};
use crate::utils::apierror::{error_backend_failure, specialize, ApiError};
use crate::utils::concurrent::{MaybeFutureExt, MaybeOrNever};

/// The interval between heartbeats, in milliseconds
const HEARTBEAT_INTERVAL: u64 = 100;

pub async fn main_worker(config: Configuration) {
    let descriptor = WorkerDescriptor::get_my_descriptor(&config);
    let NodeRole::Worker(worker_config) = &config.self_role else {
        panic!("expected worker role config");
    };
    let ws = main_worker_connect(worker_config, &descriptor).await.unwrap();
    main_loop(ws, &descriptor, config).await.unwrap();
}

/// Establishes the connection to the server
async fn main_worker_connect(
    config: &NodeRoleWorker,
    descriptor: &WorkerDescriptor,
) -> Result<WebSocketStream<MaybeTlsStream<TcpStream>>, ApiError> {
    info!("connecting to server ...");
    loop {
        let request = ClientRequestBuilder::new(format!("{}/api/v1/admin/workers/connect", config.master_uri).parse()?)
            .with_header(
                "Authorization",
                format!(
                    "Basic {}",
                    STANDARD.encode(format!("{}:{}", descriptor.identifier, config.worker_token))
                ),
            );
        if let Ok((ws, response)) = tokio_tungstenite::connect_async(request).await {
            if response.status().as_u16() != 101 {
                return Err(specialize(
                    error_backend_failure(),
                    format!("Error from controller when connecting: {}", response.status().as_u16()),
                ));
            }
            return Ok(ws);
        }
    }
}

async fn main_loop(
    ws: WebSocketStream<MaybeTlsStream<TcpStream>>,
    descriptor: &WorkerDescriptor,
    mut config: Configuration,
) -> Result<(), ApiError> {
    let (mut sender, mut receiver) = ws.split();
    // handshake: send the worker's descriptor
    sender.send(Message::Text(serde_json::to_string(descriptor)?.into())).await?;
    // handshake: get the connection data to the master
    let message = receiver
        .next()
        .await
        .ok_or_else(|| specialize(error_backend_failure(), String::from("expected configuration from server")))??;
    let Message::Text(message) = message else {
        return Err(specialize(
            error_backend_failure(),
            String::from("expected configuration from server"),
        ));
    };
    let external_config = serde_json::from_str::<ExternalRegistry>(message.as_str())?;
    config.set_self_from_external(external_config);
    config.write_auth_config().await?;
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
                // send the hearbeat
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
                    let message = message?;
                    match message {
                        Message::Ping(_) | Message::Pong(_) | Message::Frame(_) => { /* do nothing */ }
                        Message::Close(_) => {
                            sender.lock().await.send(Message::Close(None)).await?;
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
