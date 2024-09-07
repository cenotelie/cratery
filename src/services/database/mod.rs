/*******************************************************************************
 * Copyright (c) 2024 Cénotélie Opérations SAS (cenotelie.fr)
 ******************************************************************************/

//! Service for persisting information in the database

pub mod admin;
pub mod jobs;
pub mod packages;
pub mod stats;
pub mod users;

use std::future::Future;

use crate::utils::apierror::{error_forbidden, error_unauthorized, ApiError};
use crate::utils::db::{AppTransaction, RwSqlitePool};

/// Executes a piece of work in the context of a transaction
/// The transaction is committed if the operation succeed,
/// or rolled back if it fails
///
/// # Errors
///
/// Returns an instance of the `E` type argument
pub async fn db_transaction_read<F, FUT, T, E>(pool: &RwSqlitePool, workload: F) -> Result<T, E>
where
    F: FnOnce(Database) -> FUT,
    FUT: Future<Output = Result<T, E>>,
    E: From<sqlx::Error>,
{
    let transaction = pool.acquire_read().await?;
    let result = {
        let database = Database {
            transaction: transaction.clone(),
        };
        workload(database).await
    };
    let transaction = transaction.into_original().unwrap();
    match result {
        Ok(t) => {
            transaction.commit().await?;
            Ok(t)
        }
        Err(error) => {
            transaction.rollback().await?;
            Err(error)
        }
    }
}

/// Executes a piece of work in the context of a transaction
/// The transaction is committed if the operation succeed,
/// or rolled back if it fails
///
/// # Errors
///
/// Returns an instance of the `E` type argument
pub async fn db_transaction_write<F, FUT, T, E>(pool: &RwSqlitePool, operation: &'static str, workload: F) -> Result<T, E>
where
    F: FnOnce(Database) -> FUT,
    FUT: Future<Output = Result<T, E>>,
    E: From<sqlx::Error>,
{
    let transaction = pool.acquire_write(operation).await?;
    let result = {
        let database = Database {
            transaction: transaction.clone(),
        };
        workload(database).await
    };
    let transaction = transaction.into_original().unwrap();
    match result {
        Ok(t) => {
            transaction.commit().await?;
            Ok(t)
        }
        Err(error) => {
            transaction.rollback().await?;
            Err(error)
        }
    }
}

/// Represents the application
pub struct Database {
    /// The connection
    pub(crate) transaction: AppTransaction,
}

impl Database {
    /// Checks the security for an operation and returns the identifier of the target user (login)
    pub async fn check_is_user(&self, email: &str) -> Result<i64, ApiError> {
        let maybe_row = sqlx::query!("SELECT id FROM RegistryUser WHERE isActive = TRUE AND email = $1", email)
            .fetch_optional(&mut *self.transaction.borrow().await)
            .await?;
        let row = maybe_row.ok_or_else(error_unauthorized)?;
        Ok(row.id)
    }

    /// Checks that a user is an admin
    async fn get_is_admin(&self, uid: i64) -> Result<bool, ApiError> {
        let roles = sqlx::query!("SELECT roles FROM RegistryUser WHERE id = $1", uid)
            .fetch_optional(&mut *self.transaction.borrow().await)
            .await?
            .ok_or_else(error_forbidden)?
            .roles;
        Ok(roles.split(',').any(|role| role.trim() == "admin"))
    }

    /// Checks that a user is an admin
    pub async fn check_is_admin(&self, uid: i64) -> Result<(), ApiError> {
        let is_admin = self.get_is_admin(uid).await?;
        if is_admin {
            Ok(())
        } else {
            Err(error_forbidden())
        }
    }
}
