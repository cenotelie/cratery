/*******************************************************************************
 * Copyright (c) 2024 Cénotélie Opérations SAS (cenotelie.fr)
 ******************************************************************************/

//! Objects related to authentication

use chrono::NaiveDateTime;
use serde_derive::{Deserialize, Serialize};

/// Represents the possible access for an authenticated user
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct AuthenticatedUser {
    /// The uid for the user
    pub uid: i64,
    /// The principal (email of the user)
    pub principal: String,
    /// Whether a crate can be uploaded
    #[serde(rename = "canWrite")]
    pub can_write: bool,
    /// Whether administration can be done
    #[serde(rename = "canAdmin")]
    pub can_admin: bool,
}

/// A token for a registry user
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct RegistryUserToken {
    /// The unique identifier
    pub id: i64,
    /// The token name
    pub name: String,
    /// The last time the token was used
    #[serde(rename = "lastUsed")]
    pub last_used: NaiveDateTime,
    /// Whether a crate can be uploaded using this token
    #[serde(rename = "canWrite")]
    pub can_write: bool,
    /// Whether administration can be done using this token through the API
    #[serde(rename = "canAdmin")]
    pub can_admin: bool,
}

/// A token for a registry user
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct RegistryUserTokenWithSecret {
    /// The unique identifier
    pub id: i64,
    /// The token name
    pub name: String,
    /// The value for the token
    pub secret: String,
    /// The last time the token was used
    #[serde(rename = "lastUsed")]
    pub last_used: NaiveDateTime,
    /// Whether a crate can be uploaded using this token
    #[serde(rename = "canWrite")]
    pub can_write: bool,
    /// Whether administration can be done using this token through the API
    #[serde(rename = "canAdmin")]
    pub can_admin: bool,
}

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
