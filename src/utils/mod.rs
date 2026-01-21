/*******************************************************************************
 * Copyright (c) 2024 Cénotélie Opérations SAS (cenotelie.fr)
******************************************************************************/

//! Utility APIs for async programming

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

use apierror::ApiError;
use futures::future::BoxFuture;
use smol_str::SmolStr;
use thiserror::Error;
use tokio::io::{self, AsyncWriteExt};
use tokio::process::Command;

use crate::utils::apierror::AsStatusCode;

pub mod apierror;
pub mod axum;
pub mod concurrent;
pub mod db;
pub mod hashes;
pub mod shared;
pub mod sigterm;
pub mod token;

#[derive(Debug, Error)]
pub enum CommandError {
    #[error("failed to spawn `{cmd}` cmd at '{location}'")]
    Spawn {
        #[source]
        source: io::Error,
        cmd: SmolStr,
        location: PathBuf,
    },

    #[error("failed to write to stdin for `{cmd}`")]
    StdinWrite {
        #[source]
        source: io::Error,
        cmd: SmolStr,
    },

    #[error("failed to wait output of `{cmd}`")]
    WaitOutput {
        #[source]
        source: io::Error,
        cmd: SmolStr,
    },

    #[error("failed during execution of `{cmd}`:\n-- stdout\n{stdout}\n\n-- stderr\n{stderr}")]
    Execute { cmd: SmolStr, stdout: String, stderr: String },
}
impl AsStatusCode for CommandError {}

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
pub async fn execute_git(location: &Path, args: &[&str]) -> Result<(), CommandError> {
    execute_at_location(location, "git", args, &[]).await.map(|_| ())
}

/// Execute a command at a location
pub async fn execute_at_location(location: &Path, command: &str, args: &[&str], input: &[u8]) -> Result<Vec<u8>, CommandError> {
    let mut child = Command::new(command)
        .current_dir(location)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|source| CommandError::Spawn {
            source,
            cmd: command.into(),
            location: location.to_path_buf(),
        })?;
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(input)
        .await
        .map_err(|source| CommandError::StdinWrite {
            source,
            cmd: command.into(),
        })?;
    let output = child.wait_with_output().await.map_err(|source| CommandError::WaitOutput {
        source,
        cmd: command.into(),
    })?;
    if output.status.success() {
        Ok(output.stdout)
    } else {
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(CommandError::Execute {
            cmd: command.into(),
            stdout: stdout.into_owned(),
            stderr: stderr.into_owned(),
        })
    }
}

/// Box future that outputs a `Result` with an `ApiError`
pub type FaillibleFuture<'a, T> = BoxFuture<'a, Result<T, ApiError>>;

/// Transforms a comma separated list to a `Vec` of owned `String`
#[must_use]
pub fn comma_sep_to_vec(input: &str) -> Vec<String> {
    input
        .split(',')
        .filter_map(|s| {
            let s = s.trim();
            if s.is_empty() { None } else { Some(s.to_string()) }
        })
        .collect::<Vec<_>>()
}
