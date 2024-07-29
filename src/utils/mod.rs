/*******************************************************************************
 * Copyright (c) 2024 Cénotélie Opérations SAS (cenotelie.fr)
******************************************************************************/

//! Utility APIs for async programming

use std::time::{Duration, Instant};

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
