/*******************************************************************************
 * Copyright (c) 2024 Cénotélie Opérations SAS (cenotelie.fr)
******************************************************************************/

//! Utility APIs for async programming

use std::path::Path;
use std::process::Stdio;
use std::time::{Duration, Instant};

use apierror::{error_backend_failure, specialize, ApiError};
use futures::future::BoxFuture;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

pub mod apierror;
pub mod axum;
pub mod concurrent;
pub mod db;
pub mod s3;
pub mod shared;
pub mod sigterm;

/// Pushes an element in a vector if it is not present yet
/// Returns `true` if the vector was modified
pub fn push_if_not_present<T>(v: &mut Vec<T>, item: T) -> bool
where
    T: PartialEq<T>,
{
    if v.contains(&item) {
        false
    } else {
        v.push(item);
        true
    }
}

/// Builds an instant for stale data
/// The value is 7 days before now
#[must_use]
pub fn stale_instant() -> Instant {
    let now = Instant::now();
    now.checked_sub(Duration::from_secs(60 * 60 * 24 * 7)).unwrap()
}

/// Execute a git command
pub async fn execute_git(location: &Path, args: &[&str]) -> Result<(), ApiError> {
    execute_at_location(location, "git", args, &[]).await.map(|_| ())
}

/// Execute a command at a location
pub async fn execute_at_location(location: &Path, command: &str, args: &[&str], input: &[u8]) -> Result<Vec<u8>, ApiError> {
    let mut child = Command::new(command)
        .current_dir(location)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()?;
    child.stdin.as_mut().unwrap().write_all(input).await?;
    let output = child.wait_with_output().await?;
    if output.status.success() {
        Ok(output.stdout)
    } else {
        Err(specialize(error_backend_failure(), String::from_utf8(output.stdout)?))
    }
}

/// Box future that outputs a `Result` with an `ApiError`
pub type FaillibleFuture<'a, T> = BoxFuture<'a, Result<T, ApiError>>;
