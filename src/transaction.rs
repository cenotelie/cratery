/*******************************************************************************
 * Copyright (c) 2021 Cénotélie Opérations SAS (cenotelie.fr)
******************************************************************************/

//! API for application with a connection

use std::ops::{Deref, DerefMut};

use cenotelie_lib_async_utils::shared::{ResourceLock, SharedResource, StillSharedError};
use futures::Future;
use sqlx::{Acquire, Sqlite, SqliteConnection, Transaction};

/// A simple application transaction
#[derive(Clone)]
pub struct AppTransaction<'c> {
    /// The inner transaction
    inner: SharedResource<Transaction<'c, Sqlite>>,
}

impl<'c> AppTransaction<'c> {
    /// Borrows the shared transaction
    pub async fn borrow<'t>(&'t self) -> CheckedOutAppTransaction<'c, 't> {
        let lock = self.inner.borrow().await;
        CheckedOutAppTransaction { lock }
    }
}

/// A transaction that has been checked out for work
pub struct CheckedOutAppTransaction<'c, 't> {
    /// The lock for the mutex
    lock: ResourceLock<'t, Transaction<'c, Sqlite>>,
}

impl<'c, 't> Deref for CheckedOutAppTransaction<'c, 't> {
    type Target = SqliteConnection;

    fn deref(&self) -> &Self::Target {
        &self.lock
    }
}

impl<'c, 't> DerefMut for CheckedOutAppTransaction<'c, 't> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.lock
    }
}

/// Executes a piece of work in the context of a transaction
/// The transaction is committed if the operation succeed,
/// or rolled back if it fails
///
/// # Errors
///
/// Returns an instance of the `E` type argument
pub async fn in_transaction<'c, F, FUT, T, E>(connection: &'c mut SqliteConnection, workload: F) -> Result<T, E>
where
    F: FnOnce(AppTransaction<'c>) -> FUT,
    FUT: Future<Output = Result<T, E>>,
    E: From<sqlx::Error> + From<StillSharedError>,
{
    let app_transaction = AppTransaction {
        inner: SharedResource::new(connection.begin().await?),
    };
    let result = workload(app_transaction.clone()).await;
    let transaction = app_transaction.inner.into_original()?;
    match result {
        Ok(r) => {
            transaction.commit().await?;
            Ok(r)
        }
        Err(error) => {
            transaction.rollback().await?;
            Err(error)
        }
    }
}
