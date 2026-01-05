/*******************************************************************************
 * Copyright (c) 2024 Cénotélie Opérations SAS (cenotelie.fr)
 ******************************************************************************/

//! Definition of the error type for API requests

use std::backtrace::Backtrace;
use std::fmt::{Display, Formatter};

use axum::http::StatusCode;
use serde_derive::{Deserialize, Serialize};

/// Describes an API error
#[derive(Serialize, Deserialize, Debug)]
pub struct ApiError {
    /// The associated HTTP status code
    #[serde(with = "http_serde::status_code")]
    pub http: StatusCode,
    /// A custom error message
    pub message: String,
    /// Optional details for the error
    pub details: Option<String>,
    /// The backtrace when the error was produced
    #[serde(skip)]
    pub backtrace: Option<Backtrace>,
}

impl ApiError {
    /// Creates a new error
    #[expect(clippy::needless_pass_by_value)]
    #[must_use]
    pub fn new<M: ToString>(http: StatusCode, message: M, details: Option<String>) -> Self {
        Self {
            http,
            message: message.to_string(),
            details,
            backtrace: Some(Backtrace::capture()),
        }
    }
}

impl Display for ApiError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let details = self.details.as_ref().map_or("", std::convert::AsRef::as_ref);
        write!(f, "{} ({})", &self.message, &details)
    }
}

impl Clone for ApiError {
    fn clone(&self) -> Self {
        Self {
            http: self.http,
            message: self.message.clone(),
            details: self.details.clone(),
            backtrace: None,
        }
    }
}

/// Indicate the http status code corresponding of an Error.
///
/// This is used to concert an error into an [`ApiError`]
pub trait AsStatusCode: std::error::Error {
    fn status_code(&self) -> StatusCode {
        StatusCode::INTERNAL_SERVER_ERROR
    }
}

impl AsStatusCode for std::convert::Infallible {}
impl AsStatusCode for std::io::Error {}
impl AsStatusCode for std::string::FromUtf8Error {}

impl AsStatusCode for axum::Error {}
impl AsStatusCode for axum::extract::ws::rejection::WebSocketUpgradeRejection {}
impl AsStatusCode for axum::http::uri::InvalidUri {}
impl AsStatusCode for lettre::address::AddressError {}
impl AsStatusCode for lettre::error::Error {}
impl AsStatusCode for lettre::message::header::ContentTypeErr {}
impl AsStatusCode for lettre::transport::smtp::Error {}
impl AsStatusCode for opendal::Error {}
impl AsStatusCode for reqwest::Error {}
impl AsStatusCode for semver::Error {}
impl AsStatusCode for serde_json::Error {}
impl AsStatusCode for sqlx::Error {}
impl<T> AsStatusCode for tokio::sync::mpsc::error::SendError<T> {}
impl AsStatusCode for tokio::time::error::Elapsed {}
impl AsStatusCode for tokio_tungstenite::tungstenite::Error {}

impl<E> From<E> for ApiError
where
    E: AsStatusCode,
{
    fn from(err: E) -> Self {
        let code = err.status_code();
        Self::new(code, "The operation failed in the backend.", Some(err.to_string()))
    }
}

/// Specializes an API error with additional details
pub fn specialize(original: ApiError, details: String) -> ApiError {
    ApiError {
        details: Some(details),
        ..original
    }
}

/// Error when the operation failed in the backend
#[must_use]
pub fn error_backend_failure() -> ApiError {
    ApiError::new(
        StatusCode::INTERNAL_SERVER_ERROR,
        "The operation failed in the backend.",
        None,
    )
}

/// Error when the operation failed due to invalid input
#[must_use]
pub fn error_invalid_request() -> ApiError {
    ApiError::new(
        StatusCode::BAD_REQUEST,
        "The request could not be understood by the server.",
        None,
    )
}

/// Error when the user is not authorized (not logged in)
#[must_use]
pub fn error_unauthorized() -> ApiError {
    ApiError::new(StatusCode::UNAUTHORIZED, "User is not authenticated.", None)
}

/// Error when the requested action is forbidden to the (otherwise authenticated) user
#[must_use]
pub fn error_forbidden() -> ApiError {
    ApiError::new(StatusCode::FORBIDDEN, "This action is forbidden to the user.", None)
}

/// Error when the requested user cannot be found
#[must_use]
pub fn error_not_found() -> ApiError {
    ApiError::new(StatusCode::NOT_FOUND, "The requested resource cannot be found.", None)
}

/// Error when the request has a conflicts
#[must_use]
pub fn error_conflict() -> ApiError {
    ApiError::new(
        StatusCode::CONFLICT,
        "The request could not be processed because of conflict in the current state of the resource.",
        None,
    )
}
