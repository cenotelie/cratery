/*******************************************************************************
 * Copyright (c) 2024 Cénotélie Opérations SAS (cenotelie.fr)
 ******************************************************************************/

//! API for interaction with the sqlite database

use std::cmp::Ordering;
use std::convert::TryFrom;
use std::fmt::{Display, Formatter};
use std::ops::{Deref, DerefMut};
use std::str::FromStr;
use std::sync::{Arc, Mutex};

use futures::Future;
use log::error;
use serde_derive::{Deserialize, Serialize};
use sqlx::pool::PoolConnection;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};
use sqlx::{Acquire, Pool, Sqlite, SqliteConnection, Transaction};

use super::apierror::ApiError;
use crate::utils::shared::{ResourceLock, SharedResource, StillSharedError};

/// Maximum number of concurrent READ connections
const DB_MAX_READ_CONNECTIONS: u32 = 16;

/// A pool of sqlite connection that distinguish read-only and write connections
#[derive(Debug, Clone)]
pub struct RwSqlitePool {
    /// The pool of read-only connections
    read: Pool<Sqlite>,
    /// The pool of write connections
    write: Pool<Sqlite>,
    /// The name of the current write operation
    current_write_op: Arc<Mutex<Option<&'static str>>>,
}

impl RwSqlitePool {
    /// Creates a new pool
    pub fn new(url: &str) -> Result<RwSqlitePool, ApiError> {
        let current_write_op = Arc::new(Mutex::new(None));
        Ok(RwSqlitePool {
            read: SqlitePoolOptions::new()
                .max_connections(DB_MAX_READ_CONNECTIONS)
                .connect_lazy_with(
                    SqliteConnectOptions::from_str(url)?
                        .journal_mode(SqliteJournalMode::Wal)
                        .read_only(true),
                ),
            write: SqlitePoolOptions::new()
                .max_connections(1)
                .before_acquire(|_connection, _metadata| {
                    Box::pin(async move {
                        // println!("before_acquire");
                        Ok(true)
                    })
                })
                .after_release({
                    let current_write_op = current_write_op.clone();
                    move |_connection, _metadata| {
                        let current_write_op = current_write_op.clone();
                        Box::pin(async move {
                            *current_write_op.lock().unwrap() = None;
                            Ok(true)
                        })
                    }
                })
                .connect_lazy_with(SqliteConnectOptions::from_str(url)?.journal_mode(SqliteJournalMode::Wal)),
            current_write_op,
        })
    }

    /// Acquires a READ-only connection
    pub async fn acquire_read(&self) -> Result<PoolConnection<Sqlite>, sqlx::Error> {
        self.read.acquire().await
    }

    /// Acquires a write connection
    pub async fn acquire_write(&self, operation: &'static str) -> Result<PoolConnection<Sqlite>, sqlx::Error> {
        match self.write.acquire().await {
            Ok(c) => {
                *self.current_write_op.lock().unwrap() = Some(operation);
                Ok(c)
            }
            Err(e) => {
                if matches!(e, sqlx::Error::PoolTimedOut) {
                    let current = self.current_write_op.lock().unwrap().unwrap_or_default();
                    error!("operation {operation} timed-out waiting for write connection, because it is used by {current}");
                }
                Err(e)
            }
        }
    }
}

/// The name of the metadata for the schema version
pub const SCHEMA_METADATA_VERSION: &str = "version";

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

/// Represents a migration
pub struct Migration<'a> {
    /// The target version
    pub target: &'a str,
    /// The implementation of this migration
    pub content: MigrationContent<'a>,
}

/// The implementation of a migration
pub enum MigrationContent<'a> {
    /// The script to reach the target version
    Sql(&'a [u8]),
}

/// Error when a version number is invalid
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InvalidVersionNumber(pub String);

impl Display for InvalidVersionNumber {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "invalid schema version number {}", &self.0)
    }
}

impl std::error::Error for InvalidVersionNumber {}

/// Represents a version number for a schema
#[derive(Debug, Copy, Clone, Default, Eq, PartialEq)]
pub struct VersionNumber(u32, u32, u32);

impl TryFrom<&str> for VersionNumber {
    type Error = InvalidVersionNumber;

    /// Performs the conversion.
    fn try_from(value: &str) -> Result<Self, Self::Error> {
        let first = usize::from(value.starts_with('v'));
        let Ok(numbers) = value[first..].split('.').map(str::parse).collect::<Result<Vec<u32>, _>>() else {
            return Err(InvalidVersionNumber(value.to_string()));
        };
        if numbers.len() != 3 {
            return Err(InvalidVersionNumber(value.to_string()));
        }
        Ok(VersionNumber(numbers[0], numbers[1], numbers[2]))
    }
}

impl PartialOrd for VersionNumber {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for VersionNumber {
    fn cmp(&self, other: &Self) -> Ordering {
        match self.0.cmp(&other.0) {
            Ordering::Less => Ordering::Less,
            Ordering::Greater => Ordering::Greater,
            Ordering::Equal => match self.1.cmp(&other.1) {
                Ordering::Less => Ordering::Less,
                Ordering::Greater => Ordering::Greater,
                Ordering::Equal => self.2.cmp(&other.2),
            },
        }
    }
}

/// An error during a migration
#[derive(Debug)]
pub enum MigrationError {
    /// Error when the version number is invalid
    InvalidVersion(InvalidVersionNumber),
    /// An SQL error
    Sql(sqlx::Error),
    /// The transaction was still shared when a migration is terminated
    SharedTransaction(StillSharedError),
}

impl Display for MigrationError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            MigrationError::InvalidVersion(inner) => inner.fmt(f),
            MigrationError::Sql(inner) => inner.fmt(f),
            MigrationError::SharedTransaction(_) => write!(f, "the transaction was still shared when a it terminated"),
        }
    }
}

impl std::error::Error for MigrationError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            MigrationError::InvalidVersion(inner) => Some(inner),
            MigrationError::Sql(inner) => Some(inner),
            MigrationError::SharedTransaction(inner) => Some(inner),
        }
    }
}

impl From<InvalidVersionNumber> for MigrationError {
    fn from(err: InvalidVersionNumber) -> MigrationError {
        MigrationError::InvalidVersion(err)
    }
}

impl From<sqlx::Error> for MigrationError {
    fn from(err: sqlx::Error) -> MigrationError {
        MigrationError::Sql(err)
    }
}

impl From<StillSharedError> for MigrationError {
    fn from(err: StillSharedError) -> Self {
        MigrationError::SharedTransaction(err)
    }
}
