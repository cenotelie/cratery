/*******************************************************************************
 * Copyright (c) 2024 Cénotélie Opérations SAS (cenotelie.fr)
 ******************************************************************************/

//! Implementation of axum routes to expose the application

use std::borrow::Cow;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use axum::body::{Body, Bytes};
use axum::extract::{Path, Query, State};
use axum::http::header::{HeaderName, SET_COOKIE};
use axum::http::{header, HeaderValue, Request, StatusCode};
use axum::{BoxError, Json};
use cookie::Key;
use futures::channel::mpsc::UnboundedSender;
use futures::lock::Mutex;
use futures::{SinkExt, Stream};
use serde::Deserialize;
use sqlx::{Pool, Sqlite};
use tokio::fs::File;
use tokio_util::io::ReaderStream;

use crate::model::config::Configuration;
use crate::model::objects::{
    AuthenticatedUser, CrateInfo, CrateUploadData, CrateUploadResult, DocsGenerationJob, OwnersAddQuery, OwnersQueryResult,
    RegistryUser, RegistryUserToken, RegistryUserTokenWithSecret, SearchResults, YesNoMsgResult, YesNoResult,
};
use crate::model::{generate_token, AppVersion};
use crate::services::database::Database;
use crate::services::index::Index;
use crate::services::storage::Storage;
use crate::utils::apierror::{error_invalid_request, error_not_found, specialize, ApiError};
use crate::utils::axum::auth::{AuthData, AxumStateForCookies, Token};
use crate::utils::axum::db::{AxumStateWithPool, DbConn};
use crate::utils::axum::embedded::Resources;
use crate::utils::axum::extractors::Base64;
use crate::utils::axum::{response, response_error, ApiResult};
use crate::utils::db::in_transaction;

#[derive(Deserialize)]
pub struct PathInfoCrate {
    package: String,
}

#[derive(Deserialize)]
pub struct PathInfoCrateVersion {
    package: String,
    version: String,
}

/// Tries to authenticate using a token
pub async fn authenticate(token: &Token, app: &Database<'_>, config: &Configuration) -> Result<AuthenticatedUser, ApiError> {
    if token.id == config.self_service_login && token.secret == config.self_service_token {
        // self authentication to read
        return Ok(AuthenticatedUser {
            principal: config.self_service_login.clone(),
            can_write: false,
            can_admin: false,
        });
    }
    let user = app.check_token(&token.id, &token.secret).await?;
    Ok(user)
}

/// Response for a GET on the root
/// Redirect to the web app
pub async fn get_root(State(state): State<Arc<AxumState>>) -> (StatusCode, [(HeaderName, HeaderValue); 2]) {
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
pub async fn get_favicon(State(state): State<Arc<AxumState>>) -> (StatusCode, [(HeaderName, HeaderValue); 2], &'static [u8]) {
    let favicon = state.webapp_resources.get("favicon.png").unwrap();
    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, HeaderValue::from_static(favicon.content_type)),
            (header::CACHE_CONTROL, HeaderValue::from_static("max-age=3600")),
        ],
        favicon.content,
    )
}

/// Gets the redirection response when not authenticated
fn get_auth_redirect(state: &AxumState) -> (StatusCode, [(HeaderName, HeaderValue); 2]) {
    // redirect to login
    let nonce = generate_token(64);
    let oauth_state = generate_token(32);
    let target = format!(
        "{}?response_type={}&redirect_uri={}&client_id={}&scope={}&nonce={}&state={}",
        state.configuration.oauth_login_uri,
        "code",
        urlencoding::encode(&format!("{}/webapp/oauthcallback.html", state.configuration.web_public_uri)),
        urlencoding::encode(&state.configuration.oauth_client_id),
        urlencoding::encode(&state.configuration.oauth_client_scope),
        nonce,
        oauth_state
    );
    (
        StatusCode::FOUND,
        [
            (header::LOCATION, HeaderValue::from_str(&target).unwrap()),
            (header::CACHE_CONTROL, HeaderValue::from_static("no-cache")),
        ],
    )
}

/// Gets the redirection for a crates shortcut
pub async fn get_redirection_crate(
    Path(PathInfoCrate { package }): Path<PathInfoCrate>,
) -> (StatusCode, [(HeaderName, HeaderValue); 2]) {
    let target = format!("/webapp/crate.html?crate={package}");
    (
        StatusCode::FOUND,
        [
            (header::LOCATION, HeaderValue::from_str(&target).unwrap()),
            (header::CACHE_CONTROL, HeaderValue::from_static("max-age=3600")),
        ],
    )
}

/// Gets the favicon
pub async fn get_webapp_resource(
    mut connection: DbConn,
    auth_data: AuthData,
    State(state): State<Arc<AxumState>>,
    request: Request<Body>,
) -> Result<(StatusCode, [(HeaderName, HeaderValue); 2], &'static [u8]), StatusCode> {
    let path = request.uri().path();
    let path = &path["/webapp/".len()..];

    if let Some(crate_name) = path.strip_prefix("crates/") {
        // URL shortcut for crates
        let target = format!("/webapp/crate.html?crate={crate_name}");
        return Ok((
            StatusCode::FOUND,
            [
                (header::LOCATION, HeaderValue::from_str(&target).unwrap()),
                (header::CACHE_CONTROL, HeaderValue::from_static("max-age=3600")),
            ],
            &[],
        ));
    }

    if path == "index.html" {
        let is_authenticated = in_transaction(&mut connection, |transaction| async {
            let app = Database::new(transaction);
            auth_data
                .authenticate(|token| authenticate(token, &app, &state.configuration))
                .await
        })
        .await
        .is_ok();
        if !is_authenticated {
            let (code, headers) = get_auth_redirect(&state);
            return Ok((code, headers, &[]));
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
            resource.content,
        )),
        None => Err(StatusCode::NOT_FOUND),
    }
}

/// Redirects to the login page
pub async fn webapp_me(State(state): State<Arc<AxumState>>) -> (StatusCode, [(HeaderName, HeaderValue); 2]) {
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
pub async fn get_docs_resource(
    mut connection: DbConn,
    auth_data: AuthData,
    State(state): State<Arc<AxumState>>,
    request: Request<Body>,
) -> Result<(StatusCode, [(HeaderName, HeaderValue); 2], Body), (StatusCode, [(HeaderName, HeaderValue); 1], Body)> {
    let is_authenticated = in_transaction(&mut connection, |transaction| async {
        let app = Database::new(transaction);
        auth_data
            .authenticate(|token| authenticate(token, &app, &state.configuration))
            .await
    })
    .await
    .is_ok();
    if !is_authenticated {
        let (code, headers) = get_auth_redirect(&state);
        return Ok((code, headers, Body::empty()));
    }

    let path = &request.uri().path()[1..]; // strip leading /
    assert!(path.starts_with("docs/"));
    let extension = get_content_type(path);
    match crate::services::storage::get_storage(&state.configuration)
        .download_doc_file(&path[5..])
        .await
    {
        Ok(content) => Ok((
            StatusCode::OK,
            [
                (header::CONTENT_TYPE, HeaderValue::from_str(extension).unwrap()),
                (header::CACHE_CONTROL, HeaderValue::from_static("max-age=3600")),
            ],
            Body::from(content),
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
pub async fn api_get_current_user(
    mut connection: DbConn,
    auth_data: AuthData,
    State(state): State<Arc<AxumState>>,
) -> ApiResult<RegistryUser> {
    response(
        in_transaction(&mut connection, |transaction| async {
            let app = Database::new(transaction);
            let authenticated_user = auth_data
                .authenticate(|token| authenticate(token, &app, &state.configuration))
                .await?;
            app.get_current_user(&authenticated_user).await
        })
        .await,
    )
}

/// Attemps to login using an OAuth code
pub async fn api_login_with_oauth_code(
    mut connection: DbConn,
    mut auth_data: AuthData,
    State(state): State<Arc<AxumState>>,
    body: Bytes,
) -> Result<(StatusCode, [(HeaderName, HeaderValue); 1], Json<RegistryUser>), (StatusCode, Json<ApiError>)> {
    let code = String::from_utf8_lossy(&body);
    let registry_user = in_transaction(&mut connection, |transaction| async {
        let application = Database::new(transaction);
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
pub async fn api_logout(mut auth_data: AuthData) -> (StatusCode, [(HeaderName, HeaderValue); 1]) {
    let cookie = auth_data.create_expired_id_cookie();
    (
        StatusCode::OK,
        [(SET_COOKIE, HeaderValue::from_str(&cookie.to_string()).unwrap())],
    )
}

/// Gets the known users
pub async fn api_get_users(
    mut connection: DbConn,
    auth_data: AuthData,
    State(state): State<Arc<AxumState>>,
) -> ApiResult<Vec<RegistryUser>> {
    response(
        in_transaction(&mut connection, |transaction| async {
            let app = Database::new(transaction);
            let authenticated_user = auth_data
                .authenticate(|token| authenticate(token, &app, &state.configuration))
                .await?;
            app.get_users(&authenticated_user).await
        })
        .await,
    )
}

/// Updates the information of a user
pub async fn api_update_user(
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
            let app = Database::new(transaction);
            let authenticated_user = auth_data
                .authenticate(|token| authenticate(token, &app, &state.configuration))
                .await?;
            app.update_user(&authenticated_user, &target).await
        })
        .await,
    )
}

/// Attempts to deactivate a user
pub async fn api_deactivate_user(
    mut connection: DbConn,
    auth_data: AuthData,
    State(state): State<Arc<AxumState>>,
    Path(Base64(email)): Path<Base64>,
) -> ApiResult<()> {
    response(
        in_transaction(&mut connection, |transaction| async {
            let app = Database::new(transaction);
            let authenticated_user = auth_data
                .authenticate(|token| authenticate(token, &app, &state.configuration))
                .await?;
            app.deactivate_user(&authenticated_user, &email).await
        })
        .await,
    )
}

/// Attempts to deactivate a user
pub async fn api_reactivate_user(
    mut connection: DbConn,
    auth_data: AuthData,
    State(state): State<Arc<AxumState>>,
    Path(Base64(email)): Path<Base64>,
) -> ApiResult<()> {
    response(
        in_transaction(&mut connection, |transaction| async {
            let app = Database::new(transaction);
            let authenticated_user = auth_data
                .authenticate(|token| authenticate(token, &app, &state.configuration))
                .await?;
            app.reactivate_user(&authenticated_user, &email).await
        })
        .await,
    )
}

/// Attempts to delete a user
pub async fn api_delete_user(
    mut connection: DbConn,
    auth_data: AuthData,
    State(state): State<Arc<AxumState>>,
    Path(Base64(email)): Path<Base64>,
) -> ApiResult<()> {
    response(
        in_transaction(&mut connection, |transaction| async {
            let app = Database::new(transaction);
            let authenticated_user = auth_data
                .authenticate(|token| authenticate(token, &app, &state.configuration))
                .await?;
            app.delete_user(&authenticated_user, &email).await
        })
        .await,
    )
}

/// Gets the tokens for a user
pub async fn api_get_tokens(
    mut connection: DbConn,
    auth_data: AuthData,
    State(state): State<Arc<AxumState>>,
) -> ApiResult<Vec<RegistryUserToken>> {
    response(
        in_transaction(&mut connection, |transaction| async {
            let app = Database::new(transaction);
            let authenticated_user = auth_data
                .authenticate(|token| authenticate(token, &app, &state.configuration))
                .await?;
            app.get_tokens(&authenticated_user).await
        })
        .await,
    )
}

#[derive(Deserialize)]
pub struct CreateTokenQuery {
    #[serde(rename = "canWrite")]
    can_write: bool,
    #[serde(rename = "canAdmin")]
    can_admin: bool,
}

/// Creates a token for the current user
pub async fn api_create_token(
    mut connection: DbConn,
    auth_data: AuthData,
    State(state): State<Arc<AxumState>>,
    Query(CreateTokenQuery { can_write, can_admin }): Query<CreateTokenQuery>,
    name: String,
) -> ApiResult<RegistryUserTokenWithSecret> {
    response(
        in_transaction(&mut connection, |transaction| async {
            let app = Database::new(transaction);
            let authenticated_user = auth_data
                .authenticate(|token| authenticate(token, &app, &state.configuration))
                .await?;
            app.create_token(&authenticated_user, &name, can_write, can_admin).await
        })
        .await,
    )
}

/// Revoke a previous token
pub async fn api_revoke_token(
    mut connection: DbConn,
    auth_data: AuthData,
    State(state): State<Arc<AxumState>>,
    Path(token_id): Path<i64>,
) -> ApiResult<()> {
    response(
        in_transaction(&mut connection, |transaction| async {
            let app = Database::new(transaction);
            let authenticated_user = auth_data
                .authenticate(|token| authenticate(token, &app, &state.configuration))
                .await?;
            app.revoke_token(&authenticated_user, token_id).await
        })
        .await,
    )
}

// #[put("/crates/new")]
pub async fn api_v1_publish(
    mut connection: DbConn,
    auth_data: AuthData,
    State(state): State<Arc<AxumState>>,
    body: Bytes,
) -> ApiResult<CrateUploadResult> {
    response(
        in_transaction(&mut connection, |transaction| async move {
            let app = Database::new(transaction);
            let authenticated_user = auth_data
                .authenticate(|token| authenticate(token, &app, &state.configuration))
                .await?;
            // deserialize payload
            let package = CrateUploadData::new(&body)?;
            let index_data = package.build_index_data();
            // publish
            let index = state.index.lock().await;
            let r = app.publish(&authenticated_user, &package).await?;
            crate::services::storage::get_storage(&state.configuration)
                .store_crate(&package.metadata, package.content)
                .await?;
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

pub async fn api_v1_get_package(
    mut connection: DbConn,
    auth_data: AuthData,
    State(state): State<Arc<AxumState>>,
    Path(PathInfoCrate { package }): Path<PathInfoCrate>,
) -> ApiResult<CrateInfo> {
    response(
        in_transaction(&mut connection, |transaction| async move {
            let app = Database::new(transaction);
            let _principal = auth_data
                .authenticate(|token| authenticate(token, &app, &state.configuration))
                .await?;
            let index = state.index.lock().await;
            let versions = app.get_package_versions(&package, &index).await?;
            let metadata = crate::services::storage::get_storage(&state.configuration)
                .download_crate_metadata(&package, &versions.last().unwrap().index.vers)
                .await?;
            Ok(CrateInfo { metadata, versions })
        })
        .await,
    )
}

pub async fn api_v1_get_package_readme_last(
    mut connection: DbConn,
    auth_data: AuthData,
    State(state): State<Arc<AxumState>>,
    Path(PathInfoCrate { package }): Path<PathInfoCrate>,
) -> Result<(StatusCode, [(HeaderName, HeaderValue); 1], Vec<u8>), (StatusCode, Json<ApiError>)> {
    let data = in_transaction(&mut connection, |transaction| async move {
        let app = Database::new(transaction);
        let _principal = auth_data
            .authenticate(|token| authenticate(token, &app, &state.configuration))
            .await?;
        let version = app.get_package_last_version(&package).await?;
        let readme = crate::services::storage::get_storage(&state.configuration)
            .download_crate_readme(&package, &version)
            .await?;
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

pub async fn api_v1_get_package_readme(
    mut connection: DbConn,
    auth_data: AuthData,
    State(state): State<Arc<AxumState>>,
    Path(PathInfoCrateVersion { package, version }): Path<PathInfoCrateVersion>,
) -> Result<(StatusCode, [(HeaderName, HeaderValue); 1], Vec<u8>), (StatusCode, Json<ApiError>)> {
    let data = in_transaction(&mut connection, |transaction| async move {
        let app = Database::new(transaction);
        let _principal = auth_data
            .authenticate(|token| authenticate(token, &app, &state.configuration))
            .await?;
        let readme = crate::services::storage::get_storage(&state.configuration)
            .download_crate_readme(&package, &version)
            .await?;
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
pub async fn api_v1_download(
    mut connection: DbConn,
    auth_data: AuthData,
    State(state): State<Arc<AxumState>>,
    Path(PathInfoCrateVersion { package, version }): Path<PathInfoCrateVersion>,
) -> Result<(StatusCode, [(HeaderName, HeaderValue); 1], Vec<u8>), (StatusCode, Json<ApiError>)> {
    match in_transaction(&mut connection, |transaction| async move {
        let app = Database::new(transaction);
        let _principal = auth_data
            .authenticate(|token| authenticate(token, &app, &state.configuration))
            .await?;
        app.check_package_exists(&package, &version).await?;
        let data = crate::services::storage::get_storage(&state.configuration)
            .download_crate(&package, &version)
            .await?;
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
pub async fn api_v1_yank(
    mut connection: DbConn,
    auth_data: AuthData,
    State(state): State<Arc<AxumState>>,
    Path(PathInfoCrateVersion { package, version }): Path<PathInfoCrateVersion>,
) -> ApiResult<YesNoResult> {
    response(
        in_transaction(&mut connection, |transaction| async move {
            let app = Database::new(transaction);
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
pub async fn api_v1_unyank(
    mut connection: DbConn,
    auth_data: AuthData,
    State(state): State<Arc<AxumState>>,
    Path(PathInfoCrateVersion { package, version }): Path<PathInfoCrateVersion>,
) -> ApiResult<YesNoResult> {
    response(
        in_transaction(&mut connection, |transaction| async move {
            let app = Database::new(transaction);
            let authenticated_user = auth_data
                .authenticate(|token| authenticate(token, &app, &state.configuration))
                .await?;
            let r = app.unyank(&authenticated_user, &package, &version).await?;
            Ok(r)
        })
        .await,
    )
}

// #[post("/crates/{package}/{version}/docsregen")]
pub async fn api_v1_docs_regen(
    mut connection: DbConn,
    auth_data: AuthData,
    State(state): State<Arc<AxumState>>,
    Path(PathInfoCrateVersion { package, version }): Path<PathInfoCrateVersion>,
) -> ApiResult<()> {
    response(
        in_transaction(&mut connection, |transaction| async move {
            let app = Database::new(transaction);
            let authenticated_user = auth_data
                .authenticate(|token| authenticate(token, &app, &state.configuration))
                .await?;
            app.regenerate_documentation(&authenticated_user, &package, &version).await?;
            state
                .docs_worker_sender
                .clone()
                .send(DocsGenerationJob {
                    crate_name: package.clone(),
                    crate_version: version.clone(),
                })
                .await?;
            Ok(())
        })
        .await,
    )
}

// #[get("/crates/{package}/owners")]
pub async fn api_v1_get_owners(
    mut connection: DbConn,
    auth_data: AuthData,
    State(state): State<Arc<AxumState>>,
    Path(PathInfoCrate { package }): Path<PathInfoCrate>,
) -> ApiResult<OwnersQueryResult> {
    response(
        in_transaction(&mut connection, |transaction| async move {
            let app = Database::new(transaction);
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
pub async fn api_v1_add_owners(
    mut connection: DbConn,
    auth_data: AuthData,
    State(state): State<Arc<AxumState>>,
    Path(PathInfoCrate { package }): Path<PathInfoCrate>,
    input: Json<OwnersAddQuery>,
) -> ApiResult<YesNoMsgResult> {
    response(
        in_transaction(&mut connection, |transaction| async move {
            let app = Database::new(transaction);
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
pub async fn api_v1_remove_owners(
    mut connection: DbConn,
    auth_data: AuthData,
    State(state): State<Arc<AxumState>>,
    Path(PathInfoCrate { package }): Path<PathInfoCrate>,
    input: Json<OwnersAddQuery>,
) -> ApiResult<YesNoResult> {
    response(
        in_transaction(&mut connection, |transaction| async move {
            let app = Database::new(transaction);
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
pub struct SearchForm {
    q: String,
    per_page: Option<usize>,
}

// #[get("/crates")]
pub async fn api_v1_search(
    mut connection: DbConn,
    auth_data: AuthData,
    State(state): State<Arc<AxumState>>,
    form: Query<SearchForm>,
) -> ApiResult<SearchResults> {
    response(
        in_transaction(&mut connection, |transaction| async move {
            let app = Database::new(transaction);
            let _principal = auth_data
                .authenticate(|token| authenticate(token, &app, &state.configuration))
                .await?;
            app.search(&form.q, form.per_page).await
        })
        .await,
    )
}

pub async fn index_serve_inner(
    index: &Index,
    path: &str,
) -> Result<(impl Stream<Item = Result<impl Into<Bytes>, impl Into<BoxError>>>, HeaderValue), ApiError> {
    let file_path: PathBuf = path.parse()?;
    let file_path = index.get_index_file(&file_path).ok_or_else(error_not_found)?;
    let file = File::open(file_path).await.map_err(|_e| error_not_found())?;
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

pub async fn index_serve_check_auth(
    mut connection: DbConn,
    auth_data: &AuthData,
    config: &Configuration,
) -> Result<(), (StatusCode, [(HeaderName, HeaderValue); 2], Json<ApiError>)> {
    in_transaction(&mut connection, |transaction| async move {
        let app = Database::new(transaction);
        let _principal = auth_data.authenticate(|token| authenticate(token, &app, config)).await?;
        Ok(())
    })
    .await
    .map_err(|e| index_serve_map_err(e, &config.web_domain))?;
    Ok(())
}

pub async fn index_serve(
    connection: DbConn,
    auth_data: AuthData,
    State(state): State<Arc<AxumState>>,
    request: Request<Body>,
) -> Result<(StatusCode, [(HeaderName, HeaderValue); 2], Body), (StatusCode, [(HeaderName, HeaderValue); 2], Json<ApiError>)> {
    let map_err = |e| index_serve_map_err(e, &state.configuration.web_domain);
    let path = request.uri().path();
    if path != "/config.json" && !state.configuration.index.allow_protocol_sparse {
        // config.json is always allowed because it is always checked first by cargo
        return Err(map_err(error_not_found()));
    }
    index_serve_check_auth(connection, &auth_data, &state.configuration).await?;
    let index = state.index.lock().await;
    let (stream, content_type) = index_serve_inner(&index, path).await.map_err(map_err)?;
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

pub async fn index_serve_info_refs(
    connection: DbConn,
    auth_data: AuthData,
    State(state): State<Arc<AxumState>>,
    Query(query): Query<HashMap<String, String>>,
) -> Result<(StatusCode, [(HeaderName, HeaderValue); 2], Body), (StatusCode, [(HeaderName, HeaderValue); 2], Json<ApiError>)> {
    let map_err = |e| index_serve_map_err(e, &state.configuration.web_domain);
    if !state.configuration.index.allow_protocol_git {
        return Err(map_err(error_not_found()));
    }
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
        // dumb server response is disabled
        Err(map_err(error_not_found()))
    }
}

pub async fn index_serve_git_upload_pack(
    connection: DbConn,
    auth_data: AuthData,
    State(state): State<Arc<AxumState>>,
    body: Bytes,
) -> Result<(StatusCode, [(HeaderName, HeaderValue); 2], Body), (StatusCode, [(HeaderName, HeaderValue); 2], Json<ApiError>)> {
    let map_err = |e| index_serve_map_err(e, &state.configuration.web_domain);
    if !state.configuration.index.allow_protocol_git {
        return Err(map_err(error_not_found()));
    }
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
    pub configuration: Arc<Configuration>,
    /// A mutex for synchronisation on git commands
    pub index: Mutex<Index>,
    /// The database connection
    pub pool: Pool<Sqlite>,
    /// Key to access private cookies
    pub cookie_key: Key,
    /// Sender of documentation generation jobs
    pub docs_worker_sender: UnboundedSender<DocsGenerationJob>,
    /// The static resources for the web app
    pub webapp_resources: Resources,
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
        commit: crate::GIT_HASH.to_string(),
        tag: crate::GIT_TAG.to_string(),
    }))
}
