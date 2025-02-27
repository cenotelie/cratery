/*******************************************************************************
 * Copyright (c) 2024 Cénotélie Opérations SAS (cenotelie.fr)
******************************************************************************/

//! Utility to wait for the SIGTERM signal

use std::future::Future;
use std::pin::pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use futures::future::{Either, select};
use tokio::signal::unix::{SignalKind, signal};

/// Executes the specified future and listen for SIGTERM to terminate early
///
/// # Panics
///
/// Raise a panic when the terminate signal cannot be obtained.
pub async fn waiting_sigterm<Fut, R>(future: Fut) -> Either<R, Fut>
where
    Fut: Future<Output = R> + Unpin,
{
    waiting_sigterm_flag(future, Arc::new(AtomicBool::new(false)), 0).await
}

/// Executes the specified future and listen for SIGTERM to terminate early
/// When a SIGTERM signal is received, `signal_flag` is set so that the future
/// can observe this signal and try to terminate properly within a grace period.
/// The grace period is in milliseconds.
///
/// # Panics
///
/// Raise a panic when the terminate signal cannot be obtained.
async fn waiting_sigterm_flag<Fut, R>(future: Fut, signal_flag: Arc<AtomicBool>, grace_millis: u64) -> Either<R, Fut>
where
    Fut: Future<Output = R> + Unpin,
{
    let mut signal = signal(SignalKind::terminate()).unwrap();
    // wait for either SIGTERM or the specified future
    let future = match select(pin!(signal.recv()), future).await {
        Either::Left((_, future)) => future,
        Either::Right((r, _)) => return Either::Left(r), // the original future terminated first
    };
    // no grace period, stop now
    if grace_millis == 0 {
        return Either::Right(future);
    }
    // we received the signal, set the flag
    signal_flag.store(true, Ordering::SeqCst);
    // wait for the grace period
    match select(pin!(tokio::time::sleep(Duration::from_millis(grace_millis))), future).await {
        Either::Left(((), future)) => Either::Right(future),
        Either::Right((r, _)) => Either::Left(r),
    }
}
