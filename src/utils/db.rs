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

use log::error;
use serde_derive::{Deserialize, Serialize};
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};
use sqlx::{Pool, Sqlite, SqliteConnection, Transaction};
use thiserror::Error;

use crate::utils::apierror::AsStatusCode;
use crate::utils::shared::{ResourceLock, SharedResource, StillSharedError};

/// Maximum number of concurrent READ connections
const DB_MAX_READ_CONNECTIONS: u32 = 16;

/// Define error than can happen during pool creation.
#[derive(Debug, Error)]
#[error("failed to create Sqlite Connect Options with url `{url}`")]
pub struct PoolCreateError {
    #[source]
    source: sqlx::Error,
    url: String,
}
impl AsStatusCode for PoolCreateError {}

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
    pub fn new(url: &str) -> Result<Self, PoolCreateError> {
        let current_write_op = Arc::new(Mutex::new(None));
        Ok(Self {
            read: SqlitePoolOptions::new()
                .max_connections(DB_MAX_READ_CONNECTIONS)
                .connect_lazy_with(
                    sqlite_connect_from_str(url)?
                        .journal_mode(SqliteJournalMode::Wal)
                        .read_only(true),
                ),
            write: SqlitePoolOptions::new()
                .max_connections(1)
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
                .connect_lazy_with(sqlite_connect_from_str(url)?.journal_mode(SqliteJournalMode::Wal)),
            current_write_op,
        })
    }

    /// Acquires a READ-only connection
    pub async fn acquire_read(&self) -> Result<AppTransaction, sqlx::Error> {
        Ok(AppTransaction {
            inner: SharedResource::new(self.read.begin().await?),
        })
    }

    /// Acquires a write connection
    pub async fn acquire_write(&self, operation: &'static str) -> Result<AppTransaction, sqlx::Error> {
        match self.write.begin().await {
            Ok(c) => {
                *self.current_write_op.lock().unwrap() = Some(operation);
                Ok(AppTransaction {
                    inner: SharedResource::new(c),
                })
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

fn sqlite_connect_from_str(url: &str) -> Result<SqliteConnectOptions, PoolCreateError> {
    SqliteConnectOptions::from_str(url).map_err(|source| PoolCreateError {
        source,
        url: url.to_string(),
    })
}

/// The name of the metadata for the schema version
pub const SCHEMA_METADATA_VERSION: &str = "version";

/// A simple application transaction
#[derive(Clone)]
pub struct AppTransaction {
    /// The inner transaction
    inner: SharedResource<Transaction<'static, Sqlite>>,
}

impl AppTransaction {
    /// Borrows the shared transaction
    pub async fn borrow(&self) -> CheckedOutAppTransaction<'_> {
        let lock = self.inner.borrow().await;
        CheckedOutAppTransaction { lock }
    }

    /// Consumes this wrapper instance and get back the original resource
    ///
    /// # Errors
    ///
    /// Return a `StillSharedError` when the resource is still shared and the original cannot be given back.
    pub fn into_original(self) -> Result<Transaction<'static, Sqlite>, StillSharedError> {
        self.inner.into_original()
    }
}

/// A transaction that has been checked out for work
pub struct CheckedOutAppTransaction<'t> {
    /// The lock for the mutex
    lock: ResourceLock<'t, Transaction<'static, Sqlite>>,
}

impl Deref for CheckedOutAppTransaction<'_> {
    type Target = SqliteConnection;

    fn deref(&self) -> &Self::Target {
        &self.lock
    }
}

impl DerefMut for CheckedOutAppTransaction<'_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.lock
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
        Ok(Self(numbers[0], numbers[1], numbers[2]))
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
#[derive(Debug, Error)]
pub enum MigrationError {
    /// Error when the version number is invalid
    #[error(transparent)]
    InvalidVersion(#[from] InvalidVersionNumber),
    /// An SQL error
    #[error(transparent)]
    Sql(#[from] sqlx::Error),
    /// The transaction was still shared when a migration is terminated
    #[error("the transaction was still shared when a it terminated")]
    SharedTransaction(StillSharedError),
}
impl AsStatusCode for MigrationError {}
