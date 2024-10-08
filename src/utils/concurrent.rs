/*******************************************************************************
 * Copyright (c) 2024 Cénotélie Opérations SAS (cenotelie.fr)
******************************************************************************/

//! Utility to run at most n concurrent jobs

use std::future::Future;
use std::pin::pin;

use futures::future::{select, select_all, Either};
use futures::{Stream, StreamExt};

/// Takes an iterator of futures and executes them concurrently, with at most n concurrent futures.
/// This is similar to the `futures::future::join_all` function, except that instead of executing them all,
/// we at most have n in concurrent execution.
#[allow(clippy::missing_panics_doc)]
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
