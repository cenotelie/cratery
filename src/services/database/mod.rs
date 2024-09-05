/*******************************************************************************
 * Copyright (c) 2024 Cénotélie Opérations SAS (cenotelie.fr)
 ******************************************************************************/

//! Service for persisting information in the database

pub mod admin;
pub mod jobs;
pub mod packages;
pub mod stats;
pub mod users;

use crate::utils::apierror::{error_forbidden, error_unauthorized, ApiError};
use crate::utils::db::AppTransaction;

/// Represents the application
pub struct Database<'c> {
    /// The connection
    pub(crate) transaction: AppTransaction<'c>,
}

impl<'c> Database<'c> {
    /// Creates a new instance
    pub fn new(transaction: AppTransaction<'c>) -> Database<'c> {
        Database { transaction }
    }

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
