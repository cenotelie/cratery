/*******************************************************************************
 * Copyright (c) 2024 Cénotélie Opérations SAS (cenotelie.fr)
 ******************************************************************************/

//! Main module

#![forbid(unsafe_code)]
#![warn(clippy::pedantic)]
#![allow(clippy::module_name_repetitions)]

use std::net::SocketAddr;
use std::pin::pin;
use std::str::FromStr;
use std::sync::Arc;

use axum::extract::DefaultBodyLimit;
use axum::routing::{delete, get, patch, post, put};
use axum::Router;
use cookie::Key;
use futures::future::select;
use log::info;

use crate::application::Application;
use crate::routes::AxumState;
use crate::utils::sigterm::waiting_sigterm;

mod application;
mod migrations;
mod model;
mod routes;
mod services;
mod utils;
mod webapp;

/// The name of this program
pub const CRATE_NAME: &str = env!("CARGO_PKG_NAME");
/// The commit that was used to build the application
pub const GIT_HASH: &str = env!("GIT_HASH");
/// The git tag tag that was used to build the application
pub const GIT_TAG: &str = env!("GIT_TAG");

/// Main payload for serving the application
async fn main_serve_app(application: Arc<Application>, cookie_key: Key) -> Result<(), std::io::Error> {
    // web application
    let webapp_resources = webapp::get_resources();
    let body_limit = application.configuration.web_body_limit;
    let socket_addr = SocketAddr::new(
        application.configuration.web_listenon_ip,
        application.configuration.web_listenon_port,
    );
    let state = Arc::new(AxumState {
        application,
        cookie_key,
        webapp_resources,
    });
    let app = Router::new()
        .route("/", get(crate::routes::get_root))
        // special handlings for git
        .route("/info/refs", get(crate::routes::index_serve_info_refs))
        .route("/git-upload-pack", post(crate::routes::index_serve_git_upload_pack))
        // web resources
        .route("/favicon.png", get(crate::routes::get_favicon))
        .route("/crates/:package/:version", get(crate::routes::get_redirection_crate_version))
        .route("/crates/:package", get(crate::routes::get_redirection_crate))
        .route("/webapp/*path", get(crate::routes::get_webapp_resource))
        // api version
        .route("/version", get(crate::routes::get_version))
        // special handling for cargo login
        .route("/me", get(crate::routes::webapp_me))
        // serve the documentation
        .route("/docs/*path", get(crate::routes::get_docs_resource))
        // API
        .nest(
            "/api/v1",
            Router::new()
                .route("/stats", get(crate::routes::api_get_global_stats))
                .route("/me", get(crate::routes::api_get_current_user))
                .route("/oauth/code", post(crate::routes::api_login_with_oauth_code))
                .route("/logout", post(crate::routes::api_logout))
                .nest(
                    "/tokens",
                    Router::new()
                        .route("/", get(crate::routes::api_get_tokens))
                        .route("/", put(crate::routes::api_create_token))
                        .route("/:token_id", delete(crate::routes::api_revoke_token)),
                )
                .nest(
                    "/users",
                    Router::new()
                        .route("/", get(crate::routes::api_get_users))
                        .route("/:target", patch(crate::routes::api_update_user))
                        .route("/:target", delete(crate::routes::api_delete_user))
                        .route("/:target/deactivate", post(crate::routes::api_deactivate_user))
                        .route("/:target/reactivate", post(crate::routes::api_reactivate_user)),
                )
                .nest(
                    "/crates",
                    Router::new()
                        .route("/", get(crate::routes::api_v1_search))
                        .route("/new", put(crate::routes::api_v1_publish_crate_version))
                        .route("/:package", get(crate::routes::api_v1_get_crate_info))
                        .route("/:package/readme", get(crate::routes::api_v1_get_crate_last_readme))
                        .route("/:package/:version/readme", get(crate::routes::api_v1_get_crate_readme))
                        .route("/:package/:version/download", get(crate::routes::api_v1_download_crate))
                        .route("/:package/:version/yank", delete(crate::routes::api_v1_yank))
                        .route("/:package/:version/unyank", put(crate::routes::api_v1_unyank))
                        .route(
                            "/:package/:version/docsregen",
                            post(crate::routes::api_v1_regen_crate_version_doc),
                        )
                        .route("/:package/:version/checkdeps", get(crate::routes::api_v1_check_crate_version))
                        .route("/:package/dlstats", get(crate::routes::api_v1_get_download_stats))
                        .route("/:package/owners", get(crate::routes::api_v1_get_crate_owners))
                        .route("/:package/owners", put(crate::routes::api_v1_add_crate_owners))
                        .route("/:package/owners", delete(crate::routes::api_v1_remove_crate_owners)),
                ),
        )
        // fall back to serving the index
        .fallback(crate::routes::index_serve)
        .layer(DefaultBodyLimit::max(body_limit))
        .with_state(state);
    axum::serve(
        tokio::net::TcpListener::bind(socket_addr)
            .await
            .unwrap_or_else(|_| panic!("failed to bind {socket_addr}")),
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await
}

fn setup_log() {
    let log_date_time_format =
        std::env::var("REGISTRY_LOG_DATE_TIME_FORMAT").unwrap_or_else(|_| String::from("[%Y-%m-%d %H:%M:%S]"));
    let log_level = std::env::var("REGISTRY_LOG_LEVEL")
        .map(|v| log::LevelFilter::from_str(&v).expect("invalid REGISTRY_LOG_LEVEL"))
        .unwrap_or(log::LevelFilter::Info);
    fern::Dispatch::new()
        .filter(move |metdata| {
            let target = metdata.target();
            target.starts_with("cratery") || target.starts_with("cenotelie")
        })
        .format(move |out, message, record| {
            out.finish(format_args!(
                "{}\t{}\t{}",
                chrono::Local::now().format(&log_date_time_format),
                record.level(),
                message
            ));
        })
        .level(log_level)
        .chain(std::io::stdout())
        .apply()
        .expect("log configuration failed");
}

/// Main entry point
#[tokio::main]
async fn main() {
    setup_log();
    info!("{} commit={} tag={}", CRATE_NAME, GIT_HASH, GIT_TAG);

    let (application, worker) = crate::application::Application::launch().await.unwrap();

    let cookie_key = Key::from(
        std::env::var("REGISTRY_WEB_COOKIE_SECRET")
            .expect("REGISTRY_WEB_COOKIE_SECRET must be set")
            .as_bytes(),
    );

    let server = pin!(main_serve_app(application, cookie_key,));

    let program = select(server, worker);
    let _ = waiting_sigterm(program).await;
}
