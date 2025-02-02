/*******************************************************************************
 * Copyright (c) 2024 Cénotélie Opérations SAS (cenotelie.fr)
******************************************************************************/

//! Utility APIs for token generation and management

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use data_encoding::HEXLOWER;
use rand::distributions::Standard;
use rand::{thread_rng, Rng};
use ring::digest::{Context, SHA256};

use super::apierror::{error_unauthorized, ApiError};

/// Generates a token
#[must_use]
pub fn generate_token(length: usize) -> String {
    let bytes_count = length * 3 / 4;
    let rng = thread_rng();
    let bytes = rng.sample_iter::<u8, _>(Standard).take(bytes_count).collect::<Vec<_>>();
    STANDARD.encode(&bytes)
}

/// Computes the SHA256 digest of bytes
#[must_use]
pub fn sha256(buffer: &[u8]) -> String {
    let mut context = Context::new(&SHA256);
    context.update(buffer);
    let digest = context.finish();
    HEXLOWER.encode(digest.as_ref())
}

/// Hashes a token secret
#[must_use]
pub fn hash_token(input: &str) -> String {
    sha256(input.as_bytes())
}

/// Checks a token hash
pub fn check_hash(token: &str, hashed: &str) -> Result<(), ApiError> {
    let matches = hashed == sha256(token.as_bytes());
    if matches {
        Ok(())
    } else {
        Err(error_unauthorized())
    }
}
