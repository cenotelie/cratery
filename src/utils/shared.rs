/*******************************************************************************
 * Copyright (c) 2024 Cénotélie Opérations SAS (cenotelie.fr)
******************************************************************************/

//! Utility to share a resource and check it out

use std::fmt::{Debug, Display, Formatter};
use std::ops::{Deref, DerefMut};
use std::sync::Arc;

use futures::lock::{Mutex, MutexGuard};
use serde_derive::{Deserialize, Serialize};

/// Error when trying to get a transaction back and it is still shared
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StillSharedError(usize);

impl Display for StillSharedError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "resource is still shared by {} instances", self.0)
    }
}

impl std::error::Error for StillSharedError {}

/// Access to a shared resource
#[derive(Debug)]
pub struct ResourceLock<'r, R> {
    /// The inner reference to the shared resource, through a locked mutex
    inner: MutexGuard<'r, R>,
}

impl<R> Deref for ResourceLock<'_, R> {
    type Target = R;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl<R> DerefMut for ResourceLock<'_, R> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.inner
    }
}

/// A resource that can be shared and checked out
#[derive(Debug, Default)]
pub struct SharedResource<R> {
    /// The inner resource
    inner: Arc<Mutex<R>>,
}

impl<R> Clone for SharedResource<R> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
    }
}

impl<R> SharedResource<R> {
    /// Wraps an underlying resource
    pub fn new(inner: R) -> Self {
        Self {
            inner: Arc::new(Mutex::new(inner)),
        }
    }

    /// Gets access to the resource
    pub async fn borrow(&self) -> ResourceLock<'_, R> {
        let lock = self.inner.lock().await;
        ResourceLock { inner: lock }
    }

    /// Consumes this wrapper instance and get back the original resource
    ///
    /// # Errors
    ///
    /// Return a `StillSharedError` when the resource is still shared and the original cannot be given back.
    pub fn into_original(self) -> Result<R, StillSharedError> {
        let mutex = Arc::try_unwrap(self.inner).map_err(|arc| {
            let count = Arc::strong_count(&arc);
            StillSharedError(count)
        })?;
        Ok(mutex.into_inner())
    }
}
