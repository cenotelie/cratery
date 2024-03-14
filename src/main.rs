/*******************************************************************************
 * Copyright (c) 2024 Cénotélie Opérations SAS (cenotelie.fr)
 ******************************************************************************/

//! Main module

#![forbid(unsafe_code)]
#![warn(clippy::pedantic)]
#![allow(clippy::module_name_repetitions)]

mod app;
mod docs;
mod index;
mod jobs;
mod migrations;
mod model;
mod storage;
mod transaction;

use std::borrow::Cow;
use std::collections::HashMap;
use std::env;
use std::net::SocketAddr;
use std::ops::{Deref, DerefMut};
use std::path::PathBuf;
use std::pin::pin;
use std::str::FromStr;
use std::sync::Arc;

use app::Application;
use axum::body::{Body, Bytes};
use axum::extract::{DefaultBodyLimit, FromRequestParts, Path, Query, State};
use axum::http::header::{HeaderName, SET_COOKIE};
use axum::http::request::Parts;
use axum::http::{header, HeaderValue, Request, StatusCode};
use axum::routing::{delete, get, patch, post, put};
use axum::{async_trait, BoxError, Json, Router};
use cenotelie_lib_apierror::{
    error_backend_failure, error_invalid_request, error_not_found, error_unauthorized, specialize, ApiError,
};
use cenotelie_lib_async_utils::terminate::waiting_sigterm;
use cenotelie_lib_axum_static_files::embed_dir;
use cenotelie_lib_axum_utils::auth::{AuthData, AxumStateForCookies, Token};
use cenotelie_lib_axum_utils::cookie::Key;
use cenotelie_lib_axum_utils::embedded::Resources;
use cenotelie_lib_axum_utils::extractors::Base64;
use cenotelie_lib_axum_utils::logging::LogLayer;
use cenotelie_lib_axum_utils::{response, response_error, ApiResult};
use futures::channel::mpsc::UnboundedSender;
use futures::future::select;
use futures::lock::Mutex;
use futures::{SinkExt, Stream};
use log::{error, info};
use model::config::Configuration;
use model::objects::{
    AuthenticatedUser, CrateInfo, CrateUploadData, CrateUploadResult, DocsGenerationJob, OwnersAddQuery, OwnersQueryResult,
    RegistryUser, RegistryUserToken, RegistryUserTokenWithSecret, SearchResults, YesNoMsgResult, YesNoResult,
};
use model::AppVersion;
use serde::Deserialize;
use sqlx::pool::PoolConnection;
use sqlx::sqlite::SqlitePoolOptions;
use sqlx::{Pool, Sqlite};
use tokio::fs::File;
use tokio_util::io::ReaderStream;
use transaction::in_transaction;

use crate::index::Index;

/// The name of this program
pub const CRATE_NAME: &str = env!("CARGO_PKG_NAME");
/// The commit that was used to build the application
pub const GIT_HASH: &str = env!("GIT_HASH");
/// The git tag tag that was used to build the application
pub const GIT_TAG: &str = env!("GIT_TAG");

/// A pooled Postgresql connection
#[derive(Debug)]
pub struct DbConn(pub PoolConnection<Sqlite>);

/// Trait for an axum state that is able to provide a pool
pub trait AxumStateWithPool {
    /// Gets the connection pool
    fn get_pool(&self) -> &Pool<Sqlite>;
}

#[async_trait]
impl<S> FromRequestParts<Arc<S>> for DbConn
where
    S: AxumStateWithPool + Send + Sync,
{
    type Rejection = (StatusCode, Json<ApiError>);

    async fn from_request_parts(_parts: &mut Parts, state: &Arc<S>) -> Result<Self, Self::Rejection> {
        match state.get_pool().acquire().await {
            Ok(connection) => Ok(DbConn(connection)),
            Err(_) => Err((StatusCode::INTERNAL_SERVER_ERROR, Json(error_backend_failure()))),
        }
    }
}

impl Deref for DbConn {
    type Target = PoolConnection<Sqlite>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for DbConn {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

#[derive(Deserialize)]
struct PathInfoCrate {
    package: String,
}

#[derive(Deserialize)]
struct PathInfoCrateVersion {
    package: String,
    version: String,
}

/// Tries to authenticate using a token
async fn authenticate(token: &Token, app: &Application<'_>, config: &Configuration) -> Result<AuthenticatedUser, ApiError> {
    if let Token::Basic { id, secret } = token {
        if id == &config.self_service_login && secret == &config.self_service_token {
            // self authentication to read
            return Ok(AuthenticatedUser {
                principal: config.self_service_login.clone(),
                can_write: false,
                can_admin: false,
            });
        }
        let user = app.check_token(id, secret).await?;
        Ok(user)
    } else {
        Err(error_unauthorized())
    }
}

/// Response for a GET on the root
/// Redirect to the web app
async fn get_root(State(state): State<Arc<AxumState>>) -> (StatusCode, [(HeaderName, HeaderValue); 2]) {
    let target = format!("{}/webapp/index.html", state.configuration.web_public_uri);
    (
        StatusCode::FOUND,
        [
            (header::LOCATION, HeaderValue::from_str(&target).unwrap()),
            (header::CACHE_CONTROL, HeaderValue::from_static("no-cache")),
        ],
    )
}

/// Gets the favicon
async fn get_favicon(State(state): State<Arc<AxumState>>) -> (StatusCode, [(HeaderName, HeaderValue); 2], &'static [u8]) {
    let favicon = state.webapp_resources.get("favicon.png").unwrap();
    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, HeaderValue::from_static(favicon.content_type)),
            (header::CACHE_CONTROL, HeaderValue::from_static("max-age=3600")),
        ],
        &favicon.content,
    )
}

/// Gets the favicon
async fn get_webapp_resource(
    mut connection: DbConn,
    auth_data: AuthData,
    State(state): State<Arc<AxumState>>,
    request: Request<Body>,
) -> Result<(StatusCode, [(HeaderName, HeaderValue); 2], &'static [u8]), StatusCode> {
    let path = request.uri().path();
    let path = &path["/webapp/".len()..];

    if path == "index.html" {
        let is_authenticated = in_transaction(&mut connection, |transaction| async {
            let app = Application::new(transaction);
            auth_data
                .authenticate(|token| authenticate(token, &app, &state.configuration))
                .await
        })
        .await
        .is_ok();
        if !is_authenticated {
            // redirect to login
            let target = format!(
                "{}?response_type={}&redirect_uri={}&client_id={}&scope={}&state={}",
                state.configuration.oauth_login_uri,
                "code",
                urlencoding::encode(&format!("{}/webapp/oauthcallback.html", state.configuration.web_public_uri)),
                state.configuration.oauth_client_id,
                state.configuration.oauth_client_scope,
                ""
            );
            return Ok((
                StatusCode::FOUND,
                [
                    (header::LOCATION, HeaderValue::from_str(&target).unwrap()),
                    (header::CACHE_CONTROL, HeaderValue::from_static("no-cache")),
                ],
                &[],
            ));
        }
    }

    let resource = state.webapp_resources.get(path);
    match resource {
        Some(resource) => Ok((
            StatusCode::OK,
            [
                (header::CONTENT_TYPE, HeaderValue::from_static(resource.content_type)),
                (header::CACHE_CONTROL, HeaderValue::from_static("max-age=3600")),
            ],
            &resource.content,
        )),
        None => Err(StatusCode::NOT_FOUND),
    }
}

/// Redirects to the login page
async fn webapp_me(State(state): State<Arc<AxumState>>) -> (StatusCode, [(HeaderName, HeaderValue); 2]) {
    let target = format!("{}/webapp/index.html", state.configuration.web_public_uri);
    (
        StatusCode::FOUND,
        [
            (header::LOCATION, HeaderValue::from_str(&target).unwrap()),
            (header::CACHE_CONTROL, HeaderValue::from_static("no-cache")),
        ],
    )
}

/// Gets a file from the documentation
async fn get_docs_resource(
    mut connection: DbConn,
    auth_data: AuthData,
    State(state): State<Arc<AxumState>>,
    request: Request<Body>,
) -> Result<(StatusCode, [(HeaderName, HeaderValue); 2], Body), (StatusCode, [(HeaderName, HeaderValue); 1], Body)> {
    let is_authenticated = in_transaction(&mut connection, |transaction| async {
        let app = Application::new(transaction);
        auth_data
            .authenticate(|token| authenticate(token, &app, &state.configuration))
            .await
    })
    .await
    .is_ok();
    if !is_authenticated {
        // redirect to login
        let target = format!(
            "{}?response_type={}&redirect_uri={}&client_id={}&scope={}&state={}",
            state.configuration.oauth_login_uri,
            "code",
            urlencoding::encode(&format!("{}/webapp/oauthcallback.html", state.configuration.web_public_uri)),
            state.configuration.oauth_client_id,
            state.configuration.oauth_client_scope,
            ""
        );
        return Ok((
            StatusCode::FOUND,
            [
                (header::LOCATION, HeaderValue::from_str(&target).unwrap()),
                (header::CACHE_CONTROL, HeaderValue::from_static("no-cache")),
            ],
            Body::empty(),
        ));
    }

    let path = &request.uri().path()[1..]; // strip leading /
    assert!(path.starts_with("docs/"));
    let extension = get_content_type(path);
    match cenotelie_lib_s3::get_object_stream(&state.configuration.s3, &state.configuration.bucket, path, None).await {
        Ok(stream) => Ok((
            StatusCode::OK,
            [
                (header::CONTENT_TYPE, HeaderValue::from_str(extension).unwrap()),
                (header::CACHE_CONTROL, HeaderValue::from_static("max-age=3600")),
            ],
            Body::from_stream(stream),
        )),
        Err(e) => {
            let message = e.to_string();
            Err((
                StatusCode::NOT_FOUND,
                [(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"))],
                Body::from(message),
            ))
        }
    }
}

fn get_content_type(name: &str) -> &'static str {
    let extension = name.rfind('.').map(|index| &name[(index + 1)..]);
    match extension {
        Some("html") => "text/html",
        Some("css") => "text/css",
        Some("js") => "text/javascript",
        Some("gif") => "image/gif",
        Some("png") => "image/png",
        Some("jpeg") => "image/jpeg",
        Some("bmp") => "image/bmp",
        Some("webp") => "image/webp",
        Some("svg") => "image/svg+xml",
        Some("ico") => "image/x-icon",
        _ => "application/octet-stream",
    }
}

/// Get the current user
async fn api_get_current_user(
    mut connection: DbConn,
    auth_data: AuthData,
    State(state): State<Arc<AxumState>>,
) -> ApiResult<RegistryUser> {
    response(
        in_transaction(&mut connection, |transaction| async {
            let app = Application::new(transaction);
            let authenticated_user = auth_data
                .authenticate(|token| authenticate(token, &app, &state.configuration))
                .await?;
            app.get_current_user(&authenticated_user).await
        })
        .await,
    )
}

/// Attemps to login using an OAuth code
async fn api_login_with_oauth_code(
    mut connection: DbConn,
    mut auth_data: AuthData,
    State(state): State<Arc<AxumState>>,
    body: Bytes,
) -> Result<(StatusCode, [(HeaderName, HeaderValue); 1], Json<RegistryUser>), (StatusCode, Json<ApiError>)> {
    let code = String::from_utf8_lossy(&body);
    let registry_user = in_transaction(&mut connection, |transaction| async {
        let application = Application::new(transaction);
        application.login_with_oauth_code(&state.configuration, &code).await
    })
    .await
    .map_err(response_error)?;
    let cookie = auth_data.create_id_cookie(&AuthenticatedUser {
        principal: registry_user.email.clone(),
        // when authenticated via cookies, can do everything
        can_write: true,
        can_admin: true,
    });
    Ok((
        StatusCode::OK,
        [(SET_COOKIE, HeaderValue::from_str(&cookie.to_string()).unwrap())],
        Json(registry_user),
    ))
}

/// Logout a user
async fn api_logout(mut auth_data: AuthData) -> (StatusCode, [(HeaderName, HeaderValue); 1]) {
    let cookie = auth_data.create_expired_id_cookie();
    (
        StatusCode::OK,
        [(SET_COOKIE, HeaderValue::from_str(&cookie.to_string()).unwrap())],
    )
}

/// Gets the known users
async fn api_get_users(
    mut connection: DbConn,
    auth_data: AuthData,
    State(state): State<Arc<AxumState>>,
) -> ApiResult<Vec<RegistryUser>> {
    response(
        in_transaction(&mut connection, |transaction| async {
            let app = Application::new(transaction);
            let authenticated_user = auth_data
                .authenticate(|token| authenticate(token, &app, &state.configuration))
                .await?;
            app.get_users(&authenticated_user).await
        })
        .await,
    )
}

/// Updates the information of a user
async fn api_update_user(
    mut connection: DbConn,
    auth_data: AuthData,
    State(state): State<Arc<AxumState>>,
    Path(Base64(email)): Path<Base64>,
    target: Json<RegistryUser>,
) -> ApiResult<RegistryUser> {
    if email != target.email {
        return Err(response_error(specialize(
            error_invalid_request(),
            String::from("email in path and body are different"),
        )));
    }
    response(
        in_transaction(&mut connection, |transaction| async {
            let app = Application::new(transaction);
            let authenticated_user = auth_data
                .authenticate(|token| authenticate(token, &app, &state.configuration))
                .await?;
            app.update_user(&authenticated_user, &target).await
        })
        .await,
    )
}

/// Attempts to deactivate a user
async fn api_deactivate_user(
    mut connection: DbConn,
    auth_data: AuthData,
    State(state): State<Arc<AxumState>>,
    Path(Base64(email)): Path<Base64>,
) -> ApiResult<()> {
    response(
        in_transaction(&mut connection, |transaction| async {
            let app = Application::new(transaction);
            let authenticated_user = auth_data
                .authenticate(|token| authenticate(token, &app, &state.configuration))
                .await?;
            app.deactivate_user(&authenticated_user, &email).await
        })
        .await,
    )
}

/// Attempts to deactivate a user
async fn api_reactivate_user(
    mut connection: DbConn,
    auth_data: AuthData,
    State(state): State<Arc<AxumState>>,
    Path(Base64(email)): Path<Base64>,
) -> ApiResult<()> {
    response(
        in_transaction(&mut connection, |transaction| async {
            let app = Application::new(transaction);
            let authenticated_user = auth_data
                .authenticate(|token| authenticate(token, &app, &state.configuration))
                .await?;
            app.reactivate_user(&authenticated_user, &email).await
        })
        .await,
    )
}

/// Attempts to delete a user
async fn api_delete_user(
    mut connection: DbConn,
    auth_data: AuthData,
    State(state): State<Arc<AxumState>>,
    Path(Base64(email)): Path<Base64>,
) -> ApiResult<()> {
    response(
        in_transaction(&mut connection, |transaction| async {
            let app = Application::new(transaction);
            let authenticated_user = auth_data
                .authenticate(|token| authenticate(token, &app, &state.configuration))
                .await?;
            app.delete_user(&authenticated_user, &email).await
        })
        .await,
    )
}

/// Gets the tokens for a user
async fn api_get_tokens(
    mut connection: DbConn,
    auth_data: AuthData,
    State(state): State<Arc<AxumState>>,
) -> ApiResult<Vec<RegistryUserToken>> {
    response(
        in_transaction(&mut connection, |transaction| async {
            let app = Application::new(transaction);
            let authenticated_user = auth_data
                .authenticate(|token| authenticate(token, &app, &state.configuration))
                .await?;
            app.get_tokens(&authenticated_user).await
        })
        .await,
    )
}

#[derive(Deserialize)]
struct CreateTokenQuery {
    #[serde(rename = "canWrite")]
    can_write: bool,
    #[serde(rename = "canAdmin")]
    can_admin: bool,
}

/// Creates a token for the current user
async fn api_create_token(
    mut connection: DbConn,
    auth_data: AuthData,
    State(state): State<Arc<AxumState>>,
    Query(CreateTokenQuery { can_write, can_admin }): Query<CreateTokenQuery>,
    name: String,
) -> ApiResult<RegistryUserTokenWithSecret> {
    response(
        in_transaction(&mut connection, |transaction| async {
            let app = Application::new(transaction);
            let authenticated_user = auth_data
                .authenticate(|token| authenticate(token, &app, &state.configuration))
                .await?;
            app.create_token(&authenticated_user, &name, can_write, can_admin).await
        })
        .await,
    )
}

/// Revoke a previous token
async fn api_revoke_token(
    mut connection: DbConn,
    auth_data: AuthData,
    State(state): State<Arc<AxumState>>,
    Path(token_id): Path<i64>,
) -> ApiResult<()> {
    response(
        in_transaction(&mut connection, |transaction| async {
            let app = Application::new(transaction);
            let authenticated_user = auth_data
                .authenticate(|token| authenticate(token, &app, &state.configuration))
                .await?;
            app.revoke_token(&authenticated_user, token_id).await
        })
        .await,
    )
}

// #[put("/crates/new")]
async fn api_v1_publish(
    mut connection: DbConn,
    auth_data: AuthData,
    State(state): State<Arc<AxumState>>,
    body: Bytes,
) -> ApiResult<CrateUploadResult> {
    response(
        in_transaction(&mut connection, |transaction| async move {
            let app = Application::new(transaction);
            let authenticated_user = auth_data
                .authenticate(|token| authenticate(token, &app, &state.configuration))
                .await?;
            // deserialize payload
            let package = CrateUploadData::new(&body)?;
            let index_data = package.build_index_data();
            // publish
            let index = state.index.lock().await;
            let r = app.publish(&authenticated_user, &package).await?;
            storage::store_crate(&state.configuration, &package.metadata, package.content).await?;
            index.publish_crate_version(&index_data).await?;
            // generate the doc
            state
                .docs_worker_sender
                .clone()
                .send(DocsGenerationJob {
                    crate_name: package.metadata.name.clone(),
                    crate_version: package.metadata.vers.clone(),
                })
                .await?;
            Ok(r)
        })
        .await,
    )
}

async fn api_v1_get_package(
    mut connection: DbConn,
    auth_data: AuthData,
    State(state): State<Arc<AxumState>>,
    Path(PathInfoCrate { package }): Path<PathInfoCrate>,
) -> ApiResult<CrateInfo> {
    response(
        in_transaction(&mut connection, |transaction| async move {
            let app = Application::new(transaction);
            let _principal = auth_data
                .authenticate(|token| authenticate(token, &app, &state.configuration))
                .await?;
            let index = state.index.lock().await;
            let versions = app.get_package_versions(&package, &index).await?;
            let metadata =
                storage::download_crate_metadata(&state.configuration, &package, &versions.last().unwrap().index.vers).await?;
            Ok(CrateInfo { metadata, versions })
        })
        .await,
    )
}

async fn api_v1_get_package_readme_last(
    mut connection: DbConn,
    auth_data: AuthData,
    State(state): State<Arc<AxumState>>,
    Path(PathInfoCrate { package }): Path<PathInfoCrate>,
) -> Result<(StatusCode, [(HeaderName, HeaderValue); 1], Vec<u8>), (StatusCode, Json<ApiError>)> {
    let data = in_transaction(&mut connection, |transaction| async move {
        let app = Application::new(transaction);
        let _principal = auth_data
            .authenticate(|token| authenticate(token, &app, &state.configuration))
            .await?;
        let version = app.get_package_last_version(&package).await?;
        let readme = storage::download_crate_readme(&state.configuration, &package, &version).await?;
        Ok(readme)
    })
    .await
    .map_err(response_error)?;

    Ok((
        StatusCode::OK,
        [(header::CONTENT_TYPE, HeaderValue::from_static("text/markdown"))],
        data,
    ))
}

async fn api_v1_get_package_readme(
    mut connection: DbConn,
    auth_data: AuthData,
    State(state): State<Arc<AxumState>>,
    Path(PathInfoCrateVersion { package, version }): Path<PathInfoCrateVersion>,
) -> Result<(StatusCode, [(HeaderName, HeaderValue); 1], Vec<u8>), (StatusCode, Json<ApiError>)> {
    let data = in_transaction(&mut connection, |transaction| async move {
        let app = Application::new(transaction);
        let _principal = auth_data
            .authenticate(|token| authenticate(token, &app, &state.configuration))
            .await?;
        let readme = storage::download_crate_readme(&state.configuration, &package, &version).await?;
        Ok(readme)
    })
    .await
    .map_err(response_error)?;

    Ok((
        StatusCode::OK,
        [(header::CONTENT_TYPE, HeaderValue::from_static("text/markdown"))],
        data,
    ))
}

// #[get("/crates/{package}/{version}/download")]
async fn api_v1_download(
    mut connection: DbConn,
    auth_data: AuthData,
    State(state): State<Arc<AxumState>>,
    Path(PathInfoCrateVersion { package, version }): Path<PathInfoCrateVersion>,
) -> Result<(StatusCode, [(HeaderName, HeaderValue); 1], Vec<u8>), (StatusCode, Json<ApiError>)> {
    match in_transaction(&mut connection, |transaction| async move {
        let app = Application::new(transaction);
        let _principal = auth_data
            .authenticate(|token| authenticate(token, &app, &state.configuration))
            .await?;
        app.check_package_exists(&package, &version).await?;
        let data = storage::download_crate(&state.configuration, &package, &version).await?;
        Ok::<_, ApiError>(data)
    })
    .await
    {
        Ok(data) => Ok((
            StatusCode::OK,
            [(header::CONTENT_TYPE, HeaderValue::from_static("application/octet-stream"))],
            data,
        )),
        Err(mut error) => {
            if error.http == 401 {
                // map to 403
                error.http = 403;
            }
            Err(response_error(error))
        }
    }
}

// #[delete("/crates/{package}/{version}/yank")]
async fn api_v1_yank(
    mut connection: DbConn,
    auth_data: AuthData,
    State(state): State<Arc<AxumState>>,
    Path(PathInfoCrateVersion { package, version }): Path<PathInfoCrateVersion>,
) -> ApiResult<YesNoResult> {
    response(
        in_transaction(&mut connection, |transaction| async move {
            let app = Application::new(transaction);
            let authenticated_user = auth_data
                .authenticate(|token| authenticate(token, &app, &state.configuration))
                .await?;
            let r = app.yank(&authenticated_user, &package, &version).await?;
            Ok(r)
        })
        .await,
    )
}

// #[put("/crates/{package}/{version}/unyank")]
async fn api_v1_unyank(
    mut connection: DbConn,
    auth_data: AuthData,
    State(state): State<Arc<AxumState>>,
    Path(PathInfoCrateVersion { package, version }): Path<PathInfoCrateVersion>,
) -> ApiResult<YesNoResult> {
    response(
        in_transaction(&mut connection, |transaction| async move {
            let app = Application::new(transaction);
            let authenticated_user = auth_data
                .authenticate(|token| authenticate(token, &app, &state.configuration))
                .await?;
            let r = app.unyank(&authenticated_user, &package, &version).await?;
            Ok(r)
        })
        .await,
    )
}

// #[get("/crates/{package}/owners")]
async fn api_v1_get_owners(
    mut connection: DbConn,
    auth_data: AuthData,
    State(state): State<Arc<AxumState>>,
    Path(PathInfoCrate { package }): Path<PathInfoCrate>,
) -> ApiResult<OwnersQueryResult> {
    response(
        in_transaction(&mut connection, |transaction| async move {
            let app = Application::new(transaction);
            let authenticated_user = auth_data
                .authenticate(|token| authenticate(token, &app, &state.configuration))
                .await?;
            let r = app.get_owners(&authenticated_user, &package).await?;
            Ok(r)
        })
        .await,
    )
}

// #[put("/crates/{package}/owners")]
async fn api_v1_add_owners(
    mut connection: DbConn,
    auth_data: AuthData,
    State(state): State<Arc<AxumState>>,
    Path(PathInfoCrate { package }): Path<PathInfoCrate>,
    input: Json<OwnersAddQuery>,
) -> ApiResult<YesNoMsgResult> {
    response(
        in_transaction(&mut connection, |transaction| async move {
            let app = Application::new(transaction);
            let authenticated_user = auth_data
                .authenticate(|token| authenticate(token, &app, &state.configuration))
                .await?;
            let r = app.add_owners(&authenticated_user, &package, &input.users).await?;
            Ok(r)
        })
        .await,
    )
}

// #[delete("/crates/{package}/owners")]
async fn api_v1_remove_owners(
    mut connection: DbConn,
    auth_data: AuthData,
    State(state): State<Arc<AxumState>>,
    Path(PathInfoCrate { package }): Path<PathInfoCrate>,
    input: Json<OwnersAddQuery>,
) -> ApiResult<YesNoResult> {
    response(
        in_transaction(&mut connection, |transaction| async move {
            let app = Application::new(transaction);
            let authenticated_user = auth_data
                .authenticate(|token| authenticate(token, &app, &state.configuration))
                .await?;
            let r = app.remove_owners(&authenticated_user, &package, &input.users).await?;
            Ok(r)
        })
        .await,
    )
}

#[derive(Deserialize)]
struct SearchForm {
    q: String,
    per_page: Option<usize>,
}

// #[get("/crates")]
async fn api_v1_search(
    mut connection: DbConn,
    auth_data: AuthData,
    State(state): State<Arc<AxumState>>,
    form: Query<SearchForm>,
) -> ApiResult<SearchResults> {
    response(
        in_transaction(&mut connection, |transaction| async move {
            let app = Application::new(transaction);
            let _principal = auth_data
                .authenticate(|token| authenticate(token, &app, &state.configuration))
                .await?;
            app.search(&form.q, form.per_page).await
        })
        .await,
    )
}

async fn index_serve_inner(
    index: &Index,
    path: &str,
) -> Result<(impl Stream<Item = Result<impl Into<Bytes>, impl Into<BoxError>>>, HeaderValue), ApiError> {
    let file_path: PathBuf = path.parse()?;
    let file = File::open(&index.get_index_file(&file_path).ok_or_else(error_not_found)?)
        .await
        .map_err(|_e| error_not_found())?;
    let stream = ReaderStream::new(file);
    if std::path::Path::new(path)
        .extension()
        .map_or(false, |ext| ext.eq_ignore_ascii_case("json"))
    {
        Ok((stream, HeaderValue::from_static("application/json")))
    } else if path == "/HEAD" || path.starts_with("/info") {
        Ok((stream, HeaderValue::from_static("text/plain; charset=utf-8")))
    } else {
        Ok((stream, HeaderValue::from_static("application/octet-stream")))
    }
}

fn index_serve_map_err(e: ApiError, domain: &str) -> (StatusCode, [(HeaderName, HeaderValue); 2], Json<ApiError>) {
    let (status, body) = response_error(e);
    (
        status,
        [
            (
                header::WWW_AUTHENTICATE,
                HeaderValue::from_str(&format!("Basic realm={domain}")).unwrap(),
            ),
            (header::CACHE_CONTROL, HeaderValue::from_static("no-cache")),
        ],
        body,
    )
}

async fn index_serve_check_auth(
    mut connection: DbConn,
    auth_data: &AuthData,
    config: &Configuration,
) -> Result<(), (StatusCode, [(HeaderName, HeaderValue); 2], Json<ApiError>)> {
    in_transaction(&mut connection, |transaction| async move {
        let app = Application::new(transaction);
        let _principal = auth_data.authenticate(|token| authenticate(token, &app, config)).await?;
        Ok(())
    })
    .await
    .map_err(|e| index_serve_map_err(e, &config.web_domain))?;
    Ok(())
}

async fn index_serve(
    connection: DbConn,
    auth_data: AuthData,
    State(state): State<Arc<AxumState>>,
    request: Request<Body>,
) -> Result<(StatusCode, [(HeaderName, HeaderValue); 2], Body), (StatusCode, [(HeaderName, HeaderValue); 2], Json<ApiError>)> {
    index_serve_check_auth(connection, &auth_data, &state.configuration).await?;
    let index = state.index.lock().await;
    let (stream, content_type) = index_serve_inner(&index, request.uri().path())
        .await
        .map_err(|e| index_serve_map_err(e, &state.configuration.web_domain))?;
    let body = Body::from_stream(stream);
    Ok((
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, content_type),
            (header::CACHE_CONTROL, HeaderValue::from_static("no-cache")),
        ],
        body,
    ))
}

async fn index_serve_info_refs(
    connection: DbConn,
    auth_data: AuthData,
    State(state): State<Arc<AxumState>>,
    Query(query): Query<HashMap<String, String>>,
) -> Result<(StatusCode, [(HeaderName, HeaderValue); 2], Body), (StatusCode, [(HeaderName, HeaderValue); 2], Json<ApiError>)> {
    let map_err = |e| index_serve_map_err(e, &state.configuration.web_domain);
    index_serve_check_auth(connection, &auth_data, &state.configuration).await?;
    let index = state.index.lock().await;
    if query.get("service").map(std::string::String::as_str) == Some("git-upload-pack") {
        // smart server response
        let data = index.get_upload_pack_info_refs().await.map_err(map_err)?;
        Ok((
            StatusCode::OK,
            [
                (
                    header::CONTENT_TYPE,
                    HeaderValue::from_static("application/x-git-upload-pack-advertisement"),
                ),
                (header::CACHE_CONTROL, HeaderValue::from_static("no-cache")),
            ],
            Body::from(data),
        ))
    } else {
        // dumb server response
        let (stream, content_type) = index_serve_inner(&index, "/info/refs")
            .await
            .map_err(|e| index_serve_map_err(e, &state.configuration.web_domain))?;
        let body = Body::from_stream(stream);
        Ok((
            StatusCode::OK,
            [
                (header::CONTENT_TYPE, content_type),
                (header::CACHE_CONTROL, HeaderValue::from_static("no-cache")),
            ],
            body,
        ))
    }
}

async fn index_serve_git_upload_pack(
    connection: DbConn,
    auth_data: AuthData,
    State(state): State<Arc<AxumState>>,
    body: Bytes,
) -> Result<(StatusCode, [(HeaderName, HeaderValue); 2], Body), (StatusCode, [(HeaderName, HeaderValue); 2], Json<ApiError>)> {
    let map_err = |e| index_serve_map_err(e, &state.configuration.web_domain);
    index_serve_check_auth(connection, &auth_data, &state.configuration).await?;
    let index = state.index.lock().await;
    let data = index.get_upload_pack_for(&body).await.map_err(map_err)?;
    Ok((
        StatusCode::OK,
        [
            (
                header::CONTENT_TYPE,
                HeaderValue::from_static("application/x-git-upload-pack-result"),
            ),
            (header::CACHE_CONTROL, HeaderValue::from_static("no-cache")),
        ],
        Body::from(data),
    ))
}

/// The state of this application for axum
pub struct AxumState {
    /// The configuration
    configuration: Arc<Configuration>,
    /// A mutex for synchronisation on git commands
    index: Mutex<Index>,
    /// The database connection
    pool: Pool<Sqlite>,
    /// Key to access private cookies
    cookie_key: Key,
    /// Sender of documentation generation jobs
    docs_worker_sender: UnboundedSender<DocsGenerationJob>,
    /// The static resources for the web app
    webapp_resources: Resources,
}

impl AxumStateForCookies for AxumState {
    fn get_domain(&self) -> Cow<'static, str> {
        Cow::Owned(self.configuration.web_domain.clone())
    }

    fn get_id_cookie_name(&self) -> Cow<'static, str> {
        Cow::Borrowed("cratery-user")
    }

    fn get_cookie_key(&self) -> &Key {
        &self.cookie_key
    }
}

impl AxumStateWithPool for AxumState {
    fn get_pool(&self) -> &Pool<Sqlite> {
        &self.pool
    }
}

/// Gets the version data for the application
///
/// # Errors
///
/// Always return the `Ok` variant, but use `Result` for possible future usage.
pub async fn get_version() -> ApiResult<AppVersion> {
    response(Ok(AppVersion {
        commit: GIT_HASH.to_string(),
        tag: GIT_TAG.to_string(),
    }))
}

/// Main payload for serving the application
async fn main_serve_app(
    configuration: Arc<Configuration>,
    cookie_key: Key,
    index: Index,
    pool: Pool<Sqlite>,
    docs_worker_sender: UnboundedSender<DocsGenerationJob>,
) -> Result<(), std::io::Error> {
    // web application
    let webapp_resources = embed_dir!("src/webapp");
    let body_limit = configuration.web_body_limit;
    let socket_addr = SocketAddr::new(configuration.web_listenon_ip, configuration.web_listenon_port);
    let state = Arc::new(AxumState {
        configuration,
        index: Mutex::new(index),
        cookie_key,
        pool,
        docs_worker_sender,
        webapp_resources,
    });
    let app = Router::new()
        .route("/", get(get_root))
        // special handlings for git
        .route("/info/refs", get(index_serve_info_refs))
        .route("/git-upload-pack", post(index_serve_git_upload_pack))
        // web resources
        .route("/favicon.png", get(get_favicon))
        .route("/webapp/*path", get(get_webapp_resource))
        // api version
        .route("/version", get(get_version))
        // special handling for cargo login
        .route("/me", get(webapp_me))
        // serve the documentation
        .route("/docs/*path", get(get_docs_resource))
        // API
        .nest(
            "/api/v1",
            Router::new()
                .route("/me", get(api_get_current_user))
                .route("/oauth/code", post(api_login_with_oauth_code))
                .route("/logout", post(api_logout))
                .nest(
                    "/tokens",
                    Router::new()
                        .route("/", get(api_get_tokens))
                        .route("/", put(api_create_token))
                        .route("/:token_id", delete(api_revoke_token)),
                )
                .nest(
                    "/users",
                    Router::new()
                        .route("/", get(api_get_users))
                        .route("/:target", patch(api_update_user))
                        .route("/:target", delete(api_delete_user))
                        .route("/:target/deactivate", post(api_deactivate_user))
                        .route("/:target/reactivate", post(api_reactivate_user)),
                )
                .nest(
                    "/crates",
                    Router::new()
                        .route("/", get(api_v1_search))
                        .route("/new", put(api_v1_publish))
                        .route("/:package", get(api_v1_get_package))
                        .route("/:package/readme", get(api_v1_get_package_readme_last))
                        .route("/:package/:version/readme", get(api_v1_get_package_readme))
                        .route("/:package/:version/download", get(api_v1_download))
                        .route("/:package/:version/yank", delete(api_v1_yank))
                        .route("/:package/:version/unyank", put(api_v1_unyank))
                        .route("/:package/owners", get(api_v1_get_owners))
                        .route("/:package/owners", put(api_v1_add_owners))
                        .route("/:package/owners", delete(api_v1_remove_owners)),
                ),
        )
        // fall back to serving the index
        .fallback(index_serve)
        .layer(LogLayer)
        .layer(DefaultBodyLimit::max(body_limit))
        .with_state(state);
    axum::serve(
        tokio::net::TcpListener::bind(socket_addr)
            .await
            .expect(&format!("failed to bind {socket_addr}")),
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await
}

fn setup_log() {
    let log_date_time_format =
        env::var("REGISTRY_LOG_DATE_TIME_FORMAT").unwrap_or_else(|_| String::from("[%Y-%m-%d %H:%M:%S]"));
    let log_level = env::var("REGISTRY_LOG_LEVEL")
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

    // load configuration
    let configuration = match Configuration::from_env() {
        Ok(c) => c,
        Err(e) => {
            error!("{e}");
            return;
        }
    };
    let configuration = Arc::new(configuration);
    let cookie_key = Key::from(
        env::var("REGISTRY_WEB_COOKIE_SECRET")
            .expect("REGISTRY_WEB_COOKIE_SECRET must be set")
            .as_bytes(),
    );

    // write the auth data
    configuration.write_auth_config().await.unwrap();

    // connection pool to the database
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_lazy(&configuration.get_database_url())
        .unwrap();
    // migrate the database, if appropriate
    migrations::migrate_to_last(&mut pool.acquire().await.unwrap()).await.unwrap();

    // prepare the index
    let index = Index::on_launch(configuration.get_index_git_config()).await.unwrap();

    // extract all readmes
    // jobs::publish_readme_files(&mut pool.acquire().await.unwrap(), &configuration, &index)
    //     .await
    //     .unwrap();

    // docs worker
    let (docs_worker_sender, docs_worker) = docs::create_docs_worker(configuration.clone(), pool.clone());
    // check undocumented packages
    {
        let mut docs_worker_sender = docs_worker_sender.clone();
        let mut connection = pool.acquire().await.unwrap();
        in_transaction(&mut connection, |transaction| async move {
            let app = Application::new(transaction);
            let jobs = app.get_undocumented_packages().await?;
            for job in jobs {
                docs_worker_sender.send(job).await?;
            }
            Ok::<_, ApiError>(())
        })
        .await
        .unwrap();
    }

    let server = pin!(main_serve_app(
        configuration.clone(),
        cookie_key,
        index,
        pool,
        docs_worker_sender
    ));
    let program = select(docs_worker, server);
    let _ = waiting_sigterm(program).await;
}
