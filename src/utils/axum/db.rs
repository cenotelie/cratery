/*******************************************************************************
 * Copyright (c) 2022 Cénotélie Opérations SAS (cenotelie.fr)
******************************************************************************/

//! Database connection management

use std::ops::{Deref, DerefMut};
use std::sync::Arc;

use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use axum::http::StatusCode;
use axum::{async_trait, Json};
use sqlx::pool::PoolConnection;
use sqlx::{Pool, Sqlite};

use crate::utils::apierror::{error_backend_failure, ApiError};

/// A pooled Sqliteql connection
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
