/*******************************************************************************
 * Copyright (c) 2021 Cénotélie Opérations SAS (cenotelie.fr)
******************************************************************************/

//! Module for signing S3 queries

use chrono::{Local, NaiveDateTime};
use data_encoding::HEXLOWER;
use reqwest::header::{HeaderMap, HeaderValue};
use ring::digest::{Context, SHA256};
use ring::hmac::{self, HMAC_SHA256};

use super::S3Params;

/// Computes the SHA256 digest of bytes
pub fn sha256(buffer: &[u8]) -> String {
    let mut context = Context::new(&SHA256);
    context.update(buffer);
    let digest = context.finish();
    HEXLOWER.encode(digest.as_ref())
}

/// Computes the HMAC-SHA256 for a message
fn hmac_sha256(key: &[u8], message: &[u8]) -> hmac::Tag {
    let key = hmac::Key::new(HMAC_SHA256, key);
    hmac::sign(&key, message)
}

/// Signs a request
pub fn sign_request(
    params: &S3Params,
    method: &str,
    path: &str,
    query: &[(String, String)],
    headers: &mut HeaderMap,
    payload_hash: &str,
) {
    let now = Local::now().naive_utc();

    // add inital headers
    headers.insert(
        "x-amz-date",
        HeaderValue::from_str(&now.format("%Y%m%dT%H%M%SZ").to_string()).unwrap(),
    );
    headers.insert("x-amz-content-sha256", HeaderValue::from_str(payload_hash).unwrap());

    let canonical_hash = signing_get_canonical_request_hash(method, path, query, headers, payload_hash);
    let string_to_sign = signing_get_string_to_sign(
        now,
        &params.region,
        params.service.as_ref().map_or("s3", std::convert::AsRef::as_ref),
        &canonical_hash,
    );
    let signing_key = signing_get_key(
        &params.secret_key,
        now,
        &params.region,
        params.service.as_ref().map_or("s3", std::convert::AsRef::as_ref),
    );
    let signature = signing_do_sign(&string_to_sign, signing_key);

    let names: Vec<String> = headers.iter().map(|(name, _)| name.as_str().to_lowercase()).collect();
    headers.insert(
        reqwest::header::AUTHORIZATION,
        HeaderValue::from_str(&format!(
            "AWS4-HMAC-SHA256 Credential={}/{}/{}/s3/aws4_request, SignedHeaders={}, Signature={}",
            &params.access_key,
            now.format("%Y%m%d"),
            &params.region,
            names.join(";"),
            signature
        ))
        .unwrap(),
    );
}

/// Builds the canonical request
fn signing_get_canonical_request_hash(
    method: &str,
    path: &str,
    query: &[(String, String)],
    headers: &mut HeaderMap,
    payload_hash: &str,
) -> String {
    let mut headers = headers
        .iter()
        .map(|(name, value)| (name.as_str().to_lowercase(), value.to_str().unwrap().to_string()))
        .collect::<Vec<_>>();
    headers.sort_unstable_by(|(n1, _), (n2, _)| n1.cmp(n2));

    let mut parts = vec![method.to_string(), path.to_string()];
    if query.is_empty() {
        parts.push(String::default());
    } else {
        let mut vars: Vec<String> = query
            .iter()
            .map(|(k, v)| {
                let k = urlencoding::encode(k);
                let v = urlencoding::encode(v);
                format!("{k}={v}")
            })
            .collect();
        vars.sort_unstable();
        let result = vars.join("&");
        parts.push(result);
    }
    for (name, value) in &headers {
        parts.push(format!("{name}:{value}"));
    }
    parts.push(String::default());
    parts.push(headers.iter().map(|(name, _)| name.clone()).collect::<Vec<_>>().join(";"));
    parts.push(payload_hash.to_string());
    let canonical_request = parts.join("\n");
    sha256(canonical_request.as_bytes())
}

/// Builds the string to be signed
fn signing_get_string_to_sign(now: NaiveDateTime, region: &str, service: &str, canonical_hash: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256\n{}\n{}/{}/{}/aws4_request\n{}",
        now.format("%Y%m%dT%H%M%SZ"),
        now.format("%Y%m%d"),
        region,
        service,
        canonical_hash
    )
}

/// Gets the signing key
fn signing_get_key(secret_key: &str, now: NaiveDateTime, region: &str, service: &str) -> hmac::Tag {
    let date_yyyymmdd = now.format("%Y%m%d").to_string();
    let secret_key = format!("AWS4{secret_key}");
    let date_key = hmac_sha256(secret_key.as_bytes(), date_yyyymmdd.as_bytes());
    let date_region_key = hmac_sha256(date_key.as_ref(), region.as_bytes());
    let date_region_service_key = hmac_sha256(date_region_key.as_ref(), service.as_bytes());
    hmac_sha256(date_region_service_key.as_ref(), b"aws4_request")
}

/// Signs the final string with the derived key
fn signing_do_sign(string_to_sign: &str, signing_key: hmac::Tag) -> String {
    HEXLOWER.encode(hmac_sha256(signing_key.as_ref(), string_to_sign.as_bytes()).as_ref())
}
