/*******************************************************************************
 * Copyright (c) 2024 Cénotélie Opérations SAS (cenotelie.fr)
******************************************************************************/

//! Utility to run at most n concurrent jobs

use std::future::Future;
use std::pin::{Pin, pin};
use std::task::{Context, Poll};

use futures::future::{Either, FusedFuture, select, select_all};
use futures::{FutureExt, Stream, StreamExt};

/// Takes an iterator of futures and executes them concurrently, with at most n concurrent futures.
///
/// This is similar to the `futures::future::join_all` function, except that instead of executing them all,
/// we at most have n in concurrent execution.
#[expect(clippy::missing_panics_doc)]
pub async fn n_at_a_time<I, F, T, TEST>(futures: I, n: usize, must_stop: TEST) -> Vec<T>
where
    I: IntoIterator<Item = F>,
    F: Future<Output = T> + Send + Unpin + 'static,
    T: Send + 'static,
    TEST: Fn(&T) -> bool,
{
    let mut iterator = futures.into_iter();
    let mut ongoings = Vec::with_capacity(n);
    let mut results = Vec::new();
    let mut at_end = false;

    loop {
        // launches tasks if necessary
        while !at_end && ongoings.len() < n {
            // get next
            if let Some(task) = iterator.next() {
                ongoings.push(task);
            } else {
                at_end = true;
            }
        }

        if ongoings.is_empty() && at_end {
            break;
        }

        // wait for the next to terminate
        let (r, _index, remaining) = select_all(ongoings).await;
        results.push(r);
        if must_stop(results.last().unwrap()) {
            return results;
        }
        ongoings = remaining;
    }

    results
}

/// Takes a stream of futures and executes them concurrently, with at most n concurrent futures.
///
/// This is similar to the `futures::future::join_all` function, except that instead of executing them all,
/// we at most have n in concurrent execution.
pub async fn n_at_a_time_stream<S, F, T, TEST>(stream: S, n: usize, must_stop: TEST) -> Vec<T>
where
    S: Stream<Item = F>,
    F: Future<Output = T> + Send + Unpin + 'static,
    T: Send + 'static,
    TEST: Fn(&T) -> bool,
{
    let mut stream = pin!(stream);
    let mut ongoings = Vec::with_capacity(n);
    let mut results = Vec::new();
    let mut at_end = false;

    loop {
        // launches tasks if necessary
        while !at_end && ongoings.len() < n {
            // get next
            let mut next_getter = stream.next();
            let next = loop {
                if ongoings.is_empty() {
                    break next_getter.await;
                }
                match select(next_getter, select_all(ongoings)).await {
                    Either::Left((next, selector)) => {
                        ongoings = selector.into_inner();
                        break next;
                    }
                    Either::Right(((r, _index, remaining), nexter)) => {
                        results.push(r);
                        if must_stop(results.last().unwrap()) {
                            return results;
                        }
                        ongoings = remaining;
                        next_getter = nexter;
                    }
                }
            };
            if let Some(task) = next {
                ongoings.push(task);
            } else {
                at_end = true;
            }
        }

        if ongoings.is_empty() && at_end {
            break;
        }

        // wait for the next to terminate
        let (r, _index, remaining) = select_all(ongoings).await;
        results.push(r);
        if must_stop(results.last().unwrap()) {
            return results;
        }
        ongoings = remaining;
    }

    results
}

/// A future that may be there but never resolve if there is none
pub struct MaybeOrNever<F> {
    /// The inner future
    inner: Option<F>,
    /// Whether the inner futurer is terminated
    is_terminated: bool,
}

impl<F> Default for MaybeOrNever<F> {
    fn default() -> Self {
        Self {
            inner: None,
            is_terminated: false,
        }
    }
}

impl<F> MaybeOrNever<F> {
    /// Creates a new future
    pub const fn new(inner: F) -> Self {
        Self {
            inner: Some(inner),
            is_terminated: false,
        }
    }

    /// Gets whether there is no future inside
    pub const fn is_never(&self) -> bool {
        self.inner.is_none()
    }
}

impl<F: Future + Unpin> FusedFuture for MaybeOrNever<F> {
    fn is_terminated(&self) -> bool {
        self.is_terminated
    }
}

/// Transforms a future into a maybe missing one
pub trait MaybeFutureExt: Sized {
    /// Transforms this future into a maybe missing one
    fn maybe(self) -> MaybeOrNever<Self>;
}

impl<T> MaybeFutureExt for T {
    fn maybe(self) -> MaybeOrNever<Self> {
        MaybeOrNever::new(self)
    }
}

impl<F, O> Future for MaybeOrNever<F>
where
    F: Future<Output = O> + Unpin,
{
    type Output = O;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        if self.inner.is_none() {
            Poll::Pending
        } else {
            let r = self.as_mut().inner.as_mut().unwrap().poll_unpin(cx);
            self.is_terminated = r.is_ready();
            r
        }
    }
}
