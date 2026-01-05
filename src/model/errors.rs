/*******************************************************************************
 * Copyright (c) 2024 Cénotélie Opérations SAS (cenotelie.fr)
 ******************************************************************************/

//! Custom errors

use std::env::VarError;

use thiserror::Error;

use crate::utils::apierror::AsStatusCode;

/// Error when an environment error is missing
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("missing expected env var {var_name}")]
pub struct MissingEnvVar {
    /// The original error
    #[source]
    pub original: VarError,
    /// The name of the variable
    pub var_name: String,
}
impl AsStatusCode for MissingEnvVar {}
