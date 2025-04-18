/*******************************************************************************
 * Copyright (c) 2022 Cénotélie Opérations SAS (cenotelie.fr)
******************************************************************************/

//! Custom extractors for Axum

use std::fmt;
use std::net::{IpAddr, SocketAddr};
use std::ops::{Deref, DerefMut};

use axum::RequestPartsExt;
use axum::extract::{ConnectInfo, FromRequestParts};
use axum::http::request::Parts;
use base64::Engine;
use base64::prelude::BASE64_URL_SAFE;
use cookie::{Cookie, CookieJar};
use serde::Deserialize;
use serde::de::Visitor;

/// The client for the request, if any
#[derive(Debug, Clone)]
pub struct ClientIp(pub Option<IpAddr>);

impl<S> FromRequestParts<S> for ClientIp
where
    S: Send + Sync,
{
    type Rejection = ();

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        if let Some(forwarded) = parts.headers.get("x-forwarded-for")
            && let Ok(forwarded) = forwarded.to_str()
            && let Some(Ok(client_ip)) = forwarded.split(',').next().map(str::trim).map(str::parse)
        {
            return Ok(Self(Some(client_ip)));
        }
        match parts.extract::<ConnectInfo<SocketAddr>>().await {
            Ok(ConnectInfo(addr)) => Ok(Self(Some(addr.ip()))),
            Err(_) => Ok(Self(None)),
        }
    }
}

impl fmt::Display for ClientIp {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self.0 {
            None => write!(f, "--"),
            Some(ip) => write!(f, "{ip}"),
        }
    }
}

/// A matched argument encoded in base64
#[derive(Debug, Clone, Default)]
pub struct Base64(pub String);

impl Deref for Base64 {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for Base64 {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl<'de> Deserialize<'de> for Base64 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserializer.deserialize_string(Base64Visitor())
    }
}

struct Base64Visitor();

impl<'de> Visitor<'de> for Base64Visitor {
    type Value = Base64;

    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        write!(formatter, "a base64 string")
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        match BASE64_URL_SAFE.decode(value).map(String::from_utf8) {
            Ok(Ok(s)) => Ok(Base64(s)),
            _ => Err(serde::de::Error::invalid_type(serde::de::Unexpected::Str(value), &self)),
        }
    }

    fn visit_borrowed_str<E>(self, v: &'de str) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        self.visit_str(v)
    }

    fn visit_string<E>(self, v: String) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        self.visit_str(&v)
    }
}

/// Cookies on a request
#[derive(Debug, Clone)]
pub struct Cookies(pub CookieJar);

impl<S> FromRequestParts<S> for Cookies
where
    S: Send + Sync,
{
    type Rejection = ();

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        let mut jar = CookieJar::new();
        for cookie in &parts.headers.get_all("cookie") {
            if let Ok(cookie_value) = cookie.to_str() {
                for part in cookie_value.split(';') {
                    if let Ok(cookie) = Cookie::parse_encoded(part.trim()) {
                        jar.add(cookie.into_owned());
                    }
                }
            }
        }
        Ok(Self(jar))
    }
}
