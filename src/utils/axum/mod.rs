/*******************************************************************************
 * Copyright (c) 2024 Cénotélie Opérations SAS (cenotelie.fr)
******************************************************************************/

//! Utility APIs for axum

pub mod auth;
pub mod embedded;
pub mod extractors;
pub mod sse;

use axum::Json;
use axum::http::StatusCode;
use log::error;

use crate::utils::apierror::ApiError;

/// Defines an API response
pub type ApiResult<T> = Result<(StatusCode, Json<T>), (StatusCode, Json<ApiError>)>;

/// Produces an error response
///
/// # Panics
///
/// Panic when the HTTP code is not a correct status code
pub fn response_error_http(http: u16, error: ApiError) -> (StatusCode, Json<ApiError>) {
    if http == 500 {
        // log internal errors
        error!("{error}");
        if let Some(backtrace) = &error.backtrace {
            error!("{backtrace}");
        }
    }
    (StatusCode::from_u16(http).unwrap(), Json(error))
}

/// Produces an error response
pub fn response_error(error: ApiError) -> (StatusCode, Json<ApiError>) {
    response_error_http(error.http, error)
}

/// Produces an OK response
///
/// # Panics
///
/// Panic when the HTTP code is not a correct status code
pub fn response_ok_http<T>(http: u16, data: T) -> (StatusCode, Json<T>) {
    (StatusCode::from_u16(http).unwrap(), Json(data))
}

/// Produces an OK response
pub fn response_ok<T>(data: T) -> (StatusCode, Json<T>) {
    response_ok_http(200, data)
}

/// Maps a service result to a web api result
///
/// # Errors
///
/// Maps the corresponding error from the given `Result`.
pub fn response<T>(result: Result<T, ApiError>) -> ApiResult<T> {
    result.map_err(response_error).map(response_ok)
}
