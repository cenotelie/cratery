/*******************************************************************************
 * Copyright (c) 2024 Cénotélie Opérations SAS (cenotelie.fr)
 ******************************************************************************/

//! Implementation of axum routes to expose the application

use std::borrow::Cow;
use std::collections::HashMap;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;

use axum::body::{Body, Bytes};
use axum::extract::{Path, Query, State};
use axum::http::header::{HeaderName, SET_COOKIE};
use axum::http::{header, HeaderValue, Request, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::{BoxError, Json};
use cookie::Key;
use futures::{Stream, StreamExt};
use serde::Deserialize;
use tokio::fs::File;
use tokio_stream::wrappers::ReceiverStream;
use tokio_util::io::ReaderStream;

use crate::application::Application;
use crate::model::auth::{Authentication, RegistryUserToken, RegistryUserTokenWithSecret};
use crate::model::cargo::{
    CrateUploadResult, OwnersChangeQuery, OwnersQueryResult, RegistryUser, SearchResults, YesNoMsgResult, YesNoResult,
};
use crate::model::deps::DepsAnalysis;
use crate::model::docs::{DocGenJob, DocGenJobSpec};
use crate::model::packages::CrateInfo;
use crate::model::stats::{DownloadStats, GlobalStats};
use crate::model::{AppVersion, CrateVersion, RegistryInformation};
use crate::services::index::Index;
use crate::utils::apierror::{error_invalid_request, error_not_found, specialize, ApiError};
use crate::utils::axum::auth::{AuthData, AxumStateForCookies};
use crate::utils::axum::embedded::{EmbeddedResources, WebappResource};
use crate::utils::axum::extractors::Base64;
use crate::utils::axum::sse::{Event, ServerSentEventStream};
use crate::utils::axum::{response, response_error, ApiResult};
use crate::utils::token::generate_token;

/// The state of this application for axum
pub struct AxumState {
    /// The main application
    pub application: Arc<Application>,
    /// Key to access private cookies
    pub cookie_key: Key,
    /// The static resources for the web app
    pub webapp_resources: EmbeddedResources,
}

impl AxumStateForCookies for AxumState {
    fn get_domain(&self) -> Cow<'static, str> {
        Cow::Owned(self.application.configuration.web_domain.clone())
    }

    fn get_id_cookie_name(&self) -> Cow<'static, str> {
        Cow::Borrowed("cratery-user")
    }

    fn get_cookie_key(&self) -> &Key {
        &self.cookie_key
    }
}

impl AxumState {
    /// Gets the resource in the web app for the specified path
    async fn get_webapp_resource(&self, path: &str) -> Option<WebappResource> {
        if let Some(hot_reload_path) = self.application.configuration.web_hot_reload_path.as_ref() {
            let mut final_path = PathBuf::from(hot_reload_path);
            for element in path.split('/') {
                final_path.push(element);
            }
            let file_name = final_path.file_name().and_then(|n| n.to_str()).unwrap();
            let content_type = get_content_type(file_name);
            let data = tokio::fs::read(&final_path).await.ok()?;
            Some(WebappResource::HotReload {
                content_type: content_type.to_string(),
                data,
            })
        } else {
            let resource = self.webapp_resources.get(path).cloned()?;
            Some(WebappResource::Embedded(resource))
        }
    }
}

#[derive(Deserialize)]
pub struct PathInfoCrate {
    package: String,
}

#[derive(Deserialize)]
pub struct PathInfoCrateVersion {
    package: String,
    version: String,
}

/// Response for a GET on the root
/// Redirect to the web app
pub async fn get_root(State(state): State<Arc<AxumState>>) -> (StatusCode, [(HeaderName, HeaderValue); 2]) {
    let target = format!("{}/webapp/index.html", state.application.configuration.web_public_uri);
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
        state.application.configuration.oauth_login_uri,
        "code",
        urlencoding::encode(&format!(
            "{}/webapp/oauthcallback.html",
            state.application.configuration.web_public_uri
        )),
        urlencoding::encode(&state.application.configuration.oauth_client_id),
        urlencoding::encode(&state.application.configuration.oauth_client_scope),
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
            (
                header::CACHE_CONTROL,
                HeaderValue::from_static("public, max-age=3600, immutable"),
            ),
        ],
    )
}

/// Gets the redirection for a crates shortcut
pub async fn get_redirection_crate_version(
    Path(PathInfoCrateVersion { package, version }): Path<PathInfoCrateVersion>,
) -> (StatusCode, [(HeaderName, HeaderValue); 2]) {
    let target = format!("/webapp/crate.html?crate={package}&version={version}");
    (
        StatusCode::FOUND,
        [
            (header::LOCATION, HeaderValue::from_str(&target).unwrap()),
            (
                header::CACHE_CONTROL,
                HeaderValue::from_static("public, max-age=3600, immutable"),
            ),
        ],
    )
}

/// Gets the favicon
pub async fn get_webapp_resource(
    auth_data: AuthData,
    State(state): State<Arc<AxumState>>,
    request: Request<Body>,
) -> Result<(StatusCode, [(HeaderName, HeaderValue); 2], Cow<'static, [u8]>), StatusCode> {
    let path = request.uri().path();
    let path = &path["/webapp/".len()..];

    if let Some(crate_name) = path.strip_prefix("crates/") {
        // URL shortcut for crates
        let target = format!("/webapp/crate.html?crate={crate_name}");
        return Ok((
            StatusCode::FOUND,
            [
                (header::LOCATION, HeaderValue::from_str(&target).unwrap()),
                (
                    header::CACHE_CONTROL,
                    HeaderValue::from_static("public, max-age=3600, immutable"),
                ),
            ],
            Cow::Borrowed(&[]),
        ));
    }

    if path == "index.html" {
        let is_authenticated = state.application.authenticate(&auth_data).await.is_ok();
        if !is_authenticated {
            let (code, headers) = get_auth_redirect(&state);
            return Ok((code, headers, Cow::Borrowed(&[])));
        }
    }

    let resource = state.get_webapp_resource(path).await;
    match resource {
        Some(resource) => Ok((
            StatusCode::OK,
            [
                (header::CONTENT_TYPE, HeaderValue::from_str(resource.content_type()).unwrap()),
                (
                    header::CACHE_CONTROL,
                    HeaderValue::from_static("public, max-age=3600, immutable"),
                ),
            ],
            resource.into_data(),
        )),
        None => Err(StatusCode::NOT_FOUND),
    }
}

/// Redirects to the login page
pub async fn webapp_me(State(state): State<Arc<AxumState>>) -> (StatusCode, [(HeaderName, HeaderValue); 2]) {
    let target = format!("{}/webapp/index.html", state.application.configuration.web_public_uri);
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
    auth_data: AuthData,
    State(state): State<Arc<AxumState>>,
    request: Request<Body>,
) -> Result<(StatusCode, [(HeaderName, HeaderValue); 2], Body), (StatusCode, [(HeaderName, HeaderValue); 1], Body)> {
    let is_authenticated = state.application.authenticate(&auth_data).await.is_ok();
    if !is_authenticated {
        let (code, headers) = get_auth_redirect(&state);
        return Ok((code, headers, Body::empty()));
    }

    let elements = request.uri().path().split('/').filter(|e| !e.is_empty()).collect::<Vec<_>>();
    // expect a path of the following forms:
    // /  0            1            2           3
    // / docs / <package_name> / <version> / <file path>
    // / docs / <package_name> / <version> / <target> / <file path>
    if elements.len() < 4 || elements[0] != "docs" || semver::Version::from_str(elements[2]).is_err() {
        return Err((
            StatusCode::NOT_FOUND,
            [(
                header::CACHE_CONTROL,
                HeaderValue::from_static("public, max-age=3600, immutable"),
            )],
            Body::empty(),
        ));
    }
    // build the key
    let (target, rest_index) = if elements.len() >= 5
        && state
            .application
            .configuration
            .self_builtin_targets
            .iter()
            .any(|t| elements[3] == t)
    {
        (elements[3], 4)
    } else {
        (state.application.configuration.self_toolchain_host.as_str(), 3)
    };
    let key = format!(
        "{}/{}/{}/{}",
        elements[1],
        elements[2],
        target,
        elements[rest_index..].join("/")
    );

    let extension = get_content_type(&key);
    match state.application.get_service_storage().download_doc_file(&key).await {
        Ok(content) => Ok((
            StatusCode::OK,
            [
                (header::CONTENT_TYPE, HeaderValue::from_static(extension)),
                (
                    header::CACHE_CONTROL,
                    HeaderValue::from_static("public, max-age=3600, immutable"),
                ),
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

/// Get server configuration
pub async fn api_v1_get_registry_information(
    auth_data: AuthData,
    State(state): State<Arc<AxumState>>,
) -> ApiResult<RegistryInformation> {
    response(state.application.get_registry_information(&auth_data).await)
}

/// Get the current user
pub async fn api_v1_get_current_user(auth_data: AuthData, State(state): State<Arc<AxumState>>) -> ApiResult<RegistryUser> {
    response(state.application.get_current_user(&auth_data).await)
}

/// Attempts to login using an OAuth code
pub async fn api_v1_login_with_oauth_code(
    mut auth_data: AuthData,
    State(state): State<Arc<AxumState>>,
    body: Bytes,
) -> Result<(StatusCode, [(HeaderName, HeaderValue); 1], Json<RegistryUser>), (StatusCode, Json<ApiError>)> {
    let code = String::from_utf8_lossy(&body);
    let registry_user = state.application.login_with_oauth_code(&code).await.map_err(response_error)?;
    let cookie = auth_data.create_id_cookie(&Authentication::new_user(registry_user.id, registry_user.email.clone()));
    Ok((
        StatusCode::OK,
        [(SET_COOKIE, HeaderValue::from_str(&cookie.to_string()).unwrap())],
        Json(registry_user),
    ))
}

/// Logout a user
pub async fn api_v1_logout(mut auth_data: AuthData) -> (StatusCode, [(HeaderName, HeaderValue); 1]) {
    let cookie = auth_data.create_expired_id_cookie();
    (
        StatusCode::OK,
        [(SET_COOKIE, HeaderValue::from_str(&cookie.to_string()).unwrap())],
    )
}

/// Gets the tokens for a user
pub async fn api_v1_get_user_tokens(
    auth_data: AuthData,
    State(state): State<Arc<AxumState>>,
) -> ApiResult<Vec<RegistryUserToken>> {
    response(state.application.get_tokens(&auth_data).await)
}

#[derive(Deserialize)]
pub struct CreateTokenQuery {
    #[serde(rename = "canWrite")]
    can_write: bool,
    #[serde(rename = "canAdmin")]
    can_admin: bool,
}

/// Creates a token for the current user
pub async fn api_v1_create_user_token(
    auth_data: AuthData,
    State(state): State<Arc<AxumState>>,
    Query(CreateTokenQuery { can_write, can_admin }): Query<CreateTokenQuery>,
    name: String,
) -> ApiResult<RegistryUserTokenWithSecret> {
    response(state.application.create_token(&auth_data, &name, can_write, can_admin).await)
}

/// Revoke a previous token
pub async fn api_v1_revoke_user_token(
    auth_data: AuthData,
    State(state): State<Arc<AxumState>>,
    Path(token_id): Path<i64>,
) -> ApiResult<()> {
    response(state.application.revoke_token(&auth_data, token_id).await)
}

/// Gets the global tokens for the registry, usually for CI purposes
pub async fn api_v1_get_global_tokens(
    auth_data: AuthData,
    State(state): State<Arc<AxumState>>,
) -> ApiResult<Vec<RegistryUserToken>> {
    response(state.application.get_global_tokens(&auth_data).await)
}

/// Creates a global token for the registry
pub async fn api_v1_create_global_token(
    auth_data: AuthData,
    State(state): State<Arc<AxumState>>,
    name: String,
) -> ApiResult<RegistryUserTokenWithSecret> {
    response(state.application.create_global_token(&auth_data, &name).await)
}

/// Revokes a globel token for the registry
pub async fn api_v1_revoke_global_token(
    auth_data: AuthData,
    State(state): State<Arc<AxumState>>,
    Path(token_id): Path<i64>,
) -> ApiResult<()> {
    response(state.application.revoke_global_token(&auth_data, token_id).await)
}

/// Gets the documentation jobs
pub async fn api_v1_get_doc_gen_jobs(auth_data: AuthData, State(state): State<Arc<AxumState>>) -> ApiResult<Vec<DocGenJob>> {
    response(state.application.get_doc_gen_jobs(&auth_data).await)
}

/// Gets the log for a documentation generation job
pub async fn api_v1_get_doc_gen_job_log(
    auth_data: AuthData,
    State(state): State<Arc<AxumState>>,
    Path(job_id): Path<i64>,
) -> ApiResult<String> {
    response(state.application.get_doc_gen_job_log(&auth_data, job_id).await)
}

/// Gets a stream of updates for documentation generation jobs
pub async fn api_v1_get_doc_gen_job_updates(
    auth_data: AuthData,
    State(state): State<Arc<AxumState>>,
) -> Result<Response, (StatusCode, Json<ApiError>)> {
    let receiver = match state.application.get_doc_gen_job_updates(&auth_data).await {
        Ok(r) => r,
        Err(e) => return Err(response_error(e)),
    };
    let stream = ServerSentEventStream::new(ReceiverStream::new(receiver).map(Event::from_data));
    Ok(stream.into_response())
}

/// Gets the known users
pub async fn api_v1_get_users(auth_data: AuthData, State(state): State<Arc<AxumState>>) -> ApiResult<Vec<RegistryUser>> {
    response(state.application.get_users(&auth_data).await)
}

/// Updates the information of a user
pub async fn api_v1_update_user(
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
    response(state.application.update_user(&auth_data, &target).await)
}

/// Attempts to delete a user
pub async fn api_v1_delete_user(
    auth_data: AuthData,
    State(state): State<Arc<AxumState>>,
    Path(Base64(email)): Path<Base64>,
) -> ApiResult<()> {
    response(state.application.delete_user(&auth_data, &email).await)
}

/// Attempts to deactivate a user
pub async fn api_v1_deactivate_user(
    auth_data: AuthData,
    State(state): State<Arc<AxumState>>,
    Path(Base64(email)): Path<Base64>,
) -> ApiResult<()> {
    response(state.application.deactivate_user(&auth_data, &email).await)
}

/// Attempts to deactivate a user
pub async fn api_v1_reactivate_user(
    auth_data: AuthData,
    State(state): State<Arc<AxumState>>,
    Path(Base64(email)): Path<Base64>,
) -> ApiResult<()> {
    response(state.application.reactivate_user(&auth_data, &email).await)
}

#[derive(Deserialize)]
pub struct SearchForm {
    q: String,
    per_page: Option<usize>,
    deprecated: Option<bool>,
}

pub async fn api_v1_cargo_search(
    auth_data: AuthData,
    State(state): State<Arc<AxumState>>,
    form: Query<SearchForm>,
) -> ApiResult<SearchResults> {
    response(
        state
            .application
            .search_crates(&auth_data, &form.q, form.per_page, form.deprecated)
            .await,
    )
}

/// Gets the global statistics for the registry
pub async fn api_v1_get_crates_stats(auth_data: AuthData, State(state): State<Arc<AxumState>>) -> ApiResult<GlobalStats> {
    response(state.application.get_crates_stats(&auth_data).await)
}

/// Gets the packages that need documentation generation
pub async fn api_v1_get_crates_undocumented(
    auth_data: AuthData,
    State(state): State<Arc<AxumState>>,
) -> ApiResult<Vec<DocGenJobSpec>> {
    response(state.application.get_undocumented_crates(&auth_data).await)
}

/// Gets all the packages that are outdated while also being the latest version
pub async fn api_v1_get_crates_outdated_heads(
    auth_data: AuthData,
    State(state): State<Arc<AxumState>>,
) -> ApiResult<Vec<CrateVersion>> {
    response(state.application.get_crates_outdated_heads(&auth_data).await)
}

pub async fn api_v1_cargo_publish_crate_version(
    auth_data: AuthData,
    State(state): State<Arc<AxumState>>,
    body: Bytes,
) -> ApiResult<CrateUploadResult> {
    response(state.application.publish_crate_version(&auth_data, &body).await)
}

pub async fn api_v1_get_crate_info(
    auth_data: AuthData,
    State(state): State<Arc<AxumState>>,
    Path(PathInfoCrate { package }): Path<PathInfoCrate>,
) -> ApiResult<CrateInfo> {
    response(state.application.get_crate_info(&auth_data, &package).await)
}

pub async fn api_v1_get_crate_last_readme(
    auth_data: AuthData,
    State(state): State<Arc<AxumState>>,
    Path(PathInfoCrate { package }): Path<PathInfoCrate>,
) -> Result<(StatusCode, [(HeaderName, HeaderValue); 1], Vec<u8>), (StatusCode, Json<ApiError>)> {
    let data = state
        .application
        .get_crate_last_readme(&auth_data, &package)
        .await
        .map_err(response_error)?;

    Ok((
        StatusCode::OK,
        [(header::CONTENT_TYPE, HeaderValue::from_static("text/markdown"))],
        data,
    ))
}

pub async fn api_v1_get_crate_readme(
    auth_data: AuthData,
    State(state): State<Arc<AxumState>>,
    Path(PathInfoCrateVersion { package, version }): Path<PathInfoCrateVersion>,
) -> Result<(StatusCode, [(HeaderName, HeaderValue); 1], Vec<u8>), (StatusCode, Json<ApiError>)> {
    let data = state
        .application
        .get_crate_readme(&auth_data, &package, &version)
        .await
        .map_err(response_error)?;

    Ok((
        StatusCode::OK,
        [(header::CONTENT_TYPE, HeaderValue::from_static("text/markdown"))],
        data,
    ))
}

pub async fn api_v1_download_crate(
    auth_data: AuthData,
    State(state): State<Arc<AxumState>>,
    Path(PathInfoCrateVersion { package, version }): Path<PathInfoCrateVersion>,
) -> Result<(StatusCode, [(HeaderName, HeaderValue); 1], Vec<u8>), (StatusCode, Json<ApiError>)> {
    match state.application.get_crate_content(&auth_data, &package, &version).await {
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

pub async fn api_v1_cargo_yank(
    auth_data: AuthData,
    State(state): State<Arc<AxumState>>,
    Path(PathInfoCrateVersion { package, version }): Path<PathInfoCrateVersion>,
) -> ApiResult<YesNoResult> {
    response(state.application.yank_crate_version(&auth_data, &package, &version).await)
}

pub async fn api_v1_cargo_unyank(
    auth_data: AuthData,
    State(state): State<Arc<AxumState>>,
    Path(PathInfoCrateVersion { package, version }): Path<PathInfoCrateVersion>,
) -> ApiResult<YesNoResult> {
    response(state.application.unyank_crate_version(&auth_data, &package, &version).await)
}

pub async fn api_v1_regen_crate_version_doc(
    auth_data: AuthData,
    State(state): State<Arc<AxumState>>,
    Path(PathInfoCrateVersion { package, version }): Path<PathInfoCrateVersion>,
) -> ApiResult<Vec<DocGenJob>> {
    response(
        state
            .application
            .regen_crate_version_doc(&auth_data, &package, &version)
            .await,
    )
}

pub async fn api_v1_check_crate_version(
    auth_data: AuthData,
    State(state): State<Arc<AxumState>>,
    Path(PathInfoCrateVersion { package, version }): Path<PathInfoCrateVersion>,
) -> ApiResult<DepsAnalysis> {
    response(
        state
            .application
            .check_crate_version_deps(&auth_data, &package, &version)
            .await,
    )
}

/// Gets the download statistics for a crate
pub async fn api_v1_get_crate_dl_stats(
    auth_data: AuthData,
    State(state): State<Arc<AxumState>>,
    Path(PathInfoCrate { package }): Path<PathInfoCrate>,
) -> ApiResult<DownloadStats> {
    response(state.application.get_crate_dl_stats(&auth_data, &package).await)
}

pub async fn api_v1_cargo_get_crate_owners(
    auth_data: AuthData,
    State(state): State<Arc<AxumState>>,
    Path(PathInfoCrate { package }): Path<PathInfoCrate>,
) -> ApiResult<OwnersQueryResult> {
    response(state.application.get_crate_owners(&auth_data, &package).await)
}

pub async fn api_v1_cargo_add_crate_owners(
    auth_data: AuthData,
    State(state): State<Arc<AxumState>>,
    Path(PathInfoCrate { package }): Path<PathInfoCrate>,
    input: Json<OwnersChangeQuery>,
) -> ApiResult<YesNoMsgResult> {
    response(state.application.add_crate_owners(&auth_data, &package, &input.users).await)
}

pub async fn api_v1_cargo_remove_crate_owners(
    auth_data: AuthData,
    State(state): State<Arc<AxumState>>,
    Path(PathInfoCrate { package }): Path<PathInfoCrate>,
    input: Json<OwnersChangeQuery>,
) -> ApiResult<YesNoResult> {
    response(
        state
            .application
            .remove_crate_owners(&auth_data, &package, &input.users)
            .await,
    )
}

/// Gets the targets for a crate
pub async fn api_v1_get_crate_targets(
    auth_data: AuthData,
    State(state): State<Arc<AxumState>>,
    Path(PathInfoCrate { package }): Path<PathInfoCrate>,
) -> ApiResult<Vec<String>> {
    response(state.application.get_crate_targets(&auth_data, &package).await)
}

/// Sets the targets for a crate
pub async fn api_v1_set_crate_targets(
    auth_data: AuthData,
    State(state): State<Arc<AxumState>>,
    Path(PathInfoCrate { package }): Path<PathInfoCrate>,
    input: Json<Vec<String>>,
) -> ApiResult<()> {
    response(state.application.set_crate_targets(&auth_data, &package, &input).await)
}

/// Sets the deprecation status on a crate
pub async fn api_v1_set_crate_deprecation(
    auth_data: AuthData,
    State(state): State<Arc<AxumState>>,
    Path(PathInfoCrate { package }): Path<PathInfoCrate>,
    input: Json<bool>,
) -> ApiResult<()> {
    response(state.application.set_crate_deprecation(&auth_data, &package, input.0).await)
}

pub async fn index_serve_inner(
    index: &(dyn Index + Send + Sync),
    path: &str,
) -> Result<(impl Stream<Item = Result<impl Into<Bytes>, impl Into<BoxError>>>, HeaderValue), ApiError> {
    let file_path: PathBuf = path.parse()?;
    let file_path = index.get_index_file(&file_path).await?.ok_or_else(error_not_found)?;
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
    application: &Application,
    auth_data: &AuthData,
) -> Result<(), (StatusCode, [(HeaderName, HeaderValue); 2], Json<ApiError>)> {
    application
        .authenticate(auth_data)
        .await
        .map_err(|e| index_serve_map_err(e, &application.configuration.web_domain))?;
    Ok(())
}

pub async fn index_serve(
    auth_data: AuthData,
    State(state): State<Arc<AxumState>>,
    request: Request<Body>,
) -> Result<(StatusCode, [(HeaderName, HeaderValue); 2], Body), (StatusCode, [(HeaderName, HeaderValue); 2], Json<ApiError>)> {
    let map_err = |e| index_serve_map_err(e, &state.application.configuration.web_domain);
    let path = request.uri().path();
    if path != "/config.json" && !state.application.configuration.index.allow_protocol_sparse {
        // config.json is always allowed because it is always checked first by cargo
        return Err(map_err(error_not_found()));
    }
    index_serve_check_auth(&state.application, &auth_data).await?;
    let (stream, content_type) = index_serve_inner(state.application.get_service_index(), path)
        .await
        .map_err(map_err)?;
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

#[allow(clippy::implicit_hasher)]
pub async fn index_serve_info_refs(
    auth_data: AuthData,
    State(state): State<Arc<AxumState>>,
    Query(query): Query<HashMap<String, String>>,
) -> Result<(StatusCode, [(HeaderName, HeaderValue); 2], Body), (StatusCode, [(HeaderName, HeaderValue); 2], Json<ApiError>)> {
    let map_err = |e| index_serve_map_err(e, &state.application.configuration.web_domain);
    if !state.application.configuration.index.allow_protocol_git {
        return Err(map_err(error_not_found()));
    }
    index_serve_check_auth(&state.application, &auth_data).await?;

    if query.get("service").map(String::as_str) == Some("git-upload-pack") {
        // smart server response
        let data = state
            .application
            .get_service_index()
            .get_upload_pack_info_refs()
            .await
            .map_err(map_err)?;
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
    auth_data: AuthData,
    State(state): State<Arc<AxumState>>,
    body: Bytes,
) -> Result<(StatusCode, [(HeaderName, HeaderValue); 2], Body), (StatusCode, [(HeaderName, HeaderValue); 2], Json<ApiError>)> {
    let map_err = |e| index_serve_map_err(e, &state.application.configuration.web_domain);
    if !state.application.configuration.index.allow_protocol_git {
        return Err(map_err(error_not_found()));
    }
    index_serve_check_auth(&state.application, &auth_data).await?;
    let data = state
        .application
        .get_service_index()
        .get_upload_pack_for(&body)
        .await
        .map_err(map_err)?;
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
