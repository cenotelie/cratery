/*******************************************************************************
 * Copyright (c) 2024 Cénotélie Opérations SAS (cenotelie.fr)
 ******************************************************************************/

//! Main module

#![forbid(unsafe_code)]
#![warn(clippy::pedantic)]
#![allow(clippy::module_name_repetitions, clippy::missing_panics_doc, clippy::missing_errors_doc)]

use std::net::SocketAddr;
use std::pin::pin;
use std::str::FromStr;
use std::sync::Arc;

use axum::extract::DefaultBodyLimit;
use axum::routing::{delete, get, patch, post, put};
use axum::Router;
use cookie::Key;
use log::info;

use crate::application::Application;
use crate::routes::AxumState;
use crate::utils::sigterm::waiting_sigterm;

pub mod application;
pub mod migrations;
pub mod model;
pub mod routes;
pub mod services;
pub mod utils;
pub mod webapp;

#[cfg(test)]
mod tests;

/// The name of this program
pub const CRATE_NAME: &str = env!("CARGO_PKG_NAME");
/// The commit that was used to build the application
pub const GIT_HASH: &str = env!("GIT_HASH");
/// The git tag that was used to build the application
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
        .route("/", get(routes::get_root))
        // special handling for git
        .route("/info/refs", get(routes::index_serve_info_refs))
        .route("/git-upload-pack", post(routes::index_serve_git_upload_pack))
        // web resources
        .route("/favicon.png", get(routes::get_favicon))
        .route("/crates/:package/:version", get(routes::get_redirection_crate_version))
        .route("/crates/:package", get(routes::get_redirection_crate))
        .route("/webapp/*path", get(routes::get_webapp_resource))
        // special handling for cargo login
        .route("/me", get(routes::webapp_me))
        // serve the documentation
        .route("/docs/*path", get(routes::get_docs_resource))
        // API
        .nest(
            "/api/v1",
            Router::new()
                .route("/version", get(routes::get_version))
                .route("/registry-information", get(routes::api_v1_get_registry_information))
                .nest(
                    "/me",
                    Router::new().route("/", get(routes::api_v1_get_current_user)).nest(
                        "/tokens",
                        Router::new()
                            .route("/", get(routes::api_v1_get_user_tokens))
                            .route("/", put(routes::api_v1_create_user_token))
                            .route("/:token_id", delete(routes::api_v1_revoke_user_token)),
                    ),
                )
                .route("/oauth/code", post(routes::api_v1_login_with_oauth_code))
                .route("/logout", post(routes::api_v1_logout))
                .nest(
                    "/admin",
                    Router::new()
                        .nest(
                            "/users",
                            Router::new()
                                .route("/", get(routes::api_v1_get_users))
                                .route("/:target", patch(routes::api_v1_update_user))
                                .route("/:target", delete(routes::api_v1_delete_user))
                                .route("/:target/deactivate", post(routes::api_v1_deactivate_user))
                                .route("/:target/reactivate", post(routes::api_v1_reactivate_user)),
                        )
                        .nest(
                            "/tokens",
                            Router::new()
                                .route("/", get(routes::api_v1_get_global_tokens))
                                .route("/", put(routes::api_v1_create_global_token))
                                .route("/:token_id", delete(routes::api_v1_revoke_global_token)),
                        )
                        .route("/jobs/docgen", get(routes::api_v1_get_doc_gen_jobs))
                        .route("/jobs/docgen/updates", get(routes::api_v1_get_doc_gen_job_updates))
                        .route("/jobs/docgen/:job_id/log", get(routes::api_v1_get_doc_gen_job_log)),
                )
                .nest(
                    "/crates",
                    Router::new()
                        .route("/", get(routes::api_v1_cargo_search))
                        .route("/stats", get(routes::api_v1_get_crates_stats))
                        .route("/undocumented", get(routes::api_v1_get_crates_undocumented))
                        .route("/outdated", get(routes::api_v1_get_crates_outdated_heads))
                        .route("/new", put(routes::api_v1_cargo_publish_crate_version))
                        .route("/:package", get(routes::api_v1_get_crate_info))
                        .route("/:package/readme", get(routes::api_v1_get_crate_last_readme))
                        .route("/:package/:version/readme", get(routes::api_v1_get_crate_readme))
                        .route("/:package/:version/download", get(routes::api_v1_download_crate))
                        .route("/:package/:version/yank", delete(routes::api_v1_cargo_yank))
                        .route("/:package/:version/unyank", put(routes::api_v1_cargo_unyank))
                        .route("/:package/:version/docsregen", post(routes::api_v1_regen_crate_version_doc))
                        .route("/:package/:version/checkdeps", get(routes::api_v1_check_crate_version))
                        .route("/:package/dlstats", get(routes::api_v1_get_crate_dl_stats))
                        .route("/:package/owners", get(routes::api_v1_cargo_get_crate_owners))
                        .route("/:package/owners", put(routes::api_v1_cargo_add_crate_owners))
                        .route("/:package/owners", delete(routes::api_v1_cargo_remove_crate_owners))
                        .route("/:package/targets", get(routes::api_v1_get_crate_targets))
                        .route("/:package/targets", patch(routes::api_v1_set_crate_targets)),
                ),
        )
        // fall back to serving the index
        .fallback(routes::index_serve)
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
        .filter(move |metadata| {
            let target = metadata.target();
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

    let application = Application::launch::<services::StandardServiceProvider>().await.unwrap();

    let cookie_key = Key::from(
        std::env::var("REGISTRY_WEB_COOKIE_SECRET")
            .expect("REGISTRY_WEB_COOKIE_SECRET must be set")
            .as_bytes(),
    );

    let server = pin!(main_serve_app(application, cookie_key,));

    let _ = waiting_sigterm(server).await;
}
