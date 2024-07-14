/*******************************************************************************
 * Copyright (c) 2024 Cénotélie Opérations SAS (cenotelie.fr)
 ******************************************************************************/

//! Objects related to authentication

use serde_derive::{Deserialize, Serialize};

/// An OAuth access token
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct OAuthToken {
    /// The access token
    pub access_token: String,
    /// The type of token
    pub token_type: String,
    /// The expiration time
    pub expires_in: Option<i64>,
    /// The refresh token
    pub refresh_token: Option<String>,
    /// The grant scope
    pub scope: Option<String>,
}

/// Finds a field in a JSON blob
pub fn find_field_in_blob<'v>(blob: &'v serde_json::Value, path: &str) -> Option<&'v str> {
    let mut last = blob;
    for item in path.split('.') {
        last = last.as_object()?.get(item)?;
    }
    last.as_str()
}
