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

use crate::model::auth::ROLE_ADMIN;
use crate::utils::apierror::{ApiError, error_forbidden, error_not_found, error_unauthorized, specialize};
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
    pub async fn get_is_admin(&self, uid: i64) -> Result<bool, ApiError> {
        let roles = sqlx::query!("SELECT roles FROM RegistryUser WHERE id = $1", uid)
            .fetch_optional(&mut *self.transaction.borrow().await)
            .await?
            .ok_or_else(error_forbidden)?
            .roles;
        Ok(roles.split(',').any(|role| role.trim() == ROLE_ADMIN))
    }

    /// Checks that a user is an admin
    pub async fn check_is_admin(&self, uid: i64) -> Result<(), ApiError> {
        let is_admin = self.get_is_admin(uid).await?;
        if is_admin { Ok(()) } else { Err(error_forbidden()) }
    }

    /// Checks that a package exists
    pub async fn check_crate_exists(&self, package: &str, version: &str) -> Result<(), ApiError> {
        let _row = sqlx::query!(
            "SELECT id FROM PackageVersion WHERE package = $1 AND version = $2 LIMIT 1",
            package,
            version
        )
        .fetch_optional(&mut *self.transaction.borrow().await)
        .await?
        .ok_or_else(error_not_found)?;
        Ok(())
    }

    /// Checks the ownership of a package
    pub async fn check_is_crate_manager(&self, uid: i64, package: &str) -> Result<i64, ApiError> {
        if self.check_is_admin(uid).await.is_ok() {
            return Ok(uid);
        }
        let row = sqlx::query!(
            "SELECT id from PackageOwner WHERE package = $1 AND owner = $2 LIMIT 1",
            package,
            uid
        )
        .fetch_optional(&mut *self.transaction.borrow().await)
        .await?;
        match row {
            Some(_) => Ok(uid),
            None => Err(specialize(
                error_forbidden(),
                String::from("User is not an owner of this package"),
            )),
        }
    }
}
