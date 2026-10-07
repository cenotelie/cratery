/*******************************************************************************
 * Copyright (c) 2024 Cénotélie Opérations SAS (cenotelie.fr)
 ******************************************************************************/

//! Custom errors

use std::env::VarError;

use thiserror::Error;

use crate::utils::apierror::AsStatusCode;

/// Error when an environment error is missing
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ConfEnvError {
    #[error("missing expected env var {var_name}")]
    MissingEnvVar {
        /// The original error
        #[source]
        original: VarError,
        /// The name of the variable
        var_name: String,
    },

    #[error("failed to extract `web domain` from REGISTRY_WEB_PUBLIC_URI env")]
    WebPublicUri,
}
impl AsStatusCode for ConfEnvError {}
