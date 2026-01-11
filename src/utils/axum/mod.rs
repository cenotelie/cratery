/*******************************************************************************
 * Copyright (c) 2024 Cénotélie Opérations SAS (cenotelie.fr)
******************************************************************************/

//! Utility APIs for axum

pub mod auth;
pub mod embedded;
pub mod extractors;
pub mod sse;

use std::backtrace::{Backtrace, BacktraceStatus};

use axum::Json;
use axum::http::StatusCode;
use log::{error, info};
use uuid::Uuid;

use crate::utils::apierror::{ApiError, AsStatusCode, ResponseError};

/// Defines an API response
pub type ApiResult<T> = Result<(StatusCode, Json<T>), (StatusCode, Json<ResponseError>)>;

/// Produces an error response
pub fn response_error_http(http: StatusCode, error: ApiError) -> (StatusCode, Json<ResponseError>) {
    let uuid = Uuid::new_v4();
    if http == StatusCode::INTERNAL_SERVER_ERROR {
        // log internal errors
        error!("{uuid} {error:?}");
        if let Some(backtrace) = &error.backtrace {
            error!("{backtrace}");
        }
    } else {
        info!("{uuid} {error:?}");
    }
    let body = Json(ResponseError::new(uuid, error.message, error.details));
    (http, body)
}

/// Produces an error response
pub fn response_error(error: ApiError) -> (StatusCode, Json<ResponseError>) {
    response_error_http(error.http, error)
}

/// Produces an error response
#[expect(clippy::needless_pass_by_value)]
pub fn into_response_error(error: impl AsStatusCode) -> (StatusCode, Json<ResponseError>) {
    let status_code = error.status_code();
    let uuid = Uuid::new_v4();
    if status_code == StatusCode::INTERNAL_SERVER_ERROR {
        // log internal errors
        error!("{uuid} {error:?}");
        let backtrace = Backtrace::capture();
        if backtrace.status() == BacktraceStatus::Captured {
            error!("{backtrace}");
        }
    } else {
        info!("{uuid} {error:?}");
    }
    let body = Json(ResponseError::new(uuid, error.to_string(), None));
    (status_code, body)
}

/// Produces an OK response
pub const fn response_ok<T>(data: T) -> (StatusCode, Json<T>) {
    (StatusCode::OK, Json(data))
}

/// Maps a service result to a web api result
///
/// # Errors
///
/// Maps the corresponding error from the given `Result`.
pub fn response<T>(result: Result<T, ApiError>) -> ApiResult<T> {
    result.map_err(response_error).map(response_ok)
}
