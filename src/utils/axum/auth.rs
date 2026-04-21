/*******************************************************************************
 * Copyright (c) 2022 Cénotélie Opérations SAS (cenotelie.fr)
******************************************************************************/

//! Authentication management

use std::borrow::Cow;
use std::sync::Arc;

use axum::RequestPartsExt;
use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use base64::Engine;
use base64::prelude::BASE64_STANDARD;
use cookie::time::OffsetDateTime;
use cookie::{Cookie, CookieJar, Expiration, Key, SameSite};

use super::extractors::Cookies;
use crate::model::auth::Authentication;

/// An authentication token
#[derive(Debug, Clone)]
pub struct Token {
    pub id: String,
    pub secret: String,
}

impl Token {
    /// Try to parse a token, expected an HTTP Basic auth scheme
    fn try_parse(input: &str) -> Option<Self> {
        let parts: Vec<&str> = input.trim().split_ascii_whitespace().collect();
        if parts.len() == 2 && parts[0] == "Basic" {
            let Ok(decoded) = BASE64_STANDARD.decode(parts[1]) else {
                return None;
            };
            let Ok(decoded) = String::from_utf8(decoded) else {
                return None;
            };
            let parts: Vec<&str> = decoded.split(':').collect();
            if parts.len() == 2 {
                Some(Self {
                    id: parts[0].to_string(),
                    secret: parts[1].to_string(),
                })
            } else {
                None
            }
        } else {
            None
        }
    }
}

/// Trait for an axum state that is able to provide a key for cookies
pub trait AxumStateForCookies {
    /// Gets the domain to use for cookies
    fn get_domain(&self) -> Cow<'static, str> {
        Cow::Borrowed("localhost")
    }

    /// The name of the cookie
    fn get_id_cookie_name(&self) -> Cow<'static, str> {
        Cow::Borrowed("cenotelie-user")
    }

    /// Gets the cookie key
    fn get_cookie_key(&self) -> &Key;
}

/// Authentication data for a request
pub struct AuthData {
    /// The domain to use for cookies
    cookie_domain: Cow<'static, str>,
    /// The name for the identifying cookie
    cookie_id_name: Cow<'static, str>,
    /// The keys for cookies
    cookie_key: Key,
    /// The cookie manager
    pub cookie_jar: CookieJar,
    /// The authentication token, if any
    pub token: Option<Token>,
}

impl Default for AuthData {
    fn default() -> Self {
        Self {
            cookie_domain: Cow::Borrowed("localhost"),
            cookie_id_name: Cow::Borrowed("cratery"),
            cookie_key: Key::from(&[0; 64]),
            cookie_jar: CookieJar::default(),
            token: None,
        }
    }
}

impl From<Token> for AuthData {
    fn from(token: Token) -> Self {
        Self {
            cookie_domain: Cow::Borrowed("localhost"),
            cookie_id_name: Cow::Borrowed("cratery"),
            cookie_key: Key::from(&[0; 64]),
            cookie_jar: CookieJar::default(),
            token: Some(token),
        }
    }
}

impl<S> FromRequestParts<Arc<S>> for AuthData
where
    S: AxumStateForCookies + Send + Sync,
{
    type Rejection = ();

    async fn from_request_parts(parts: &mut Parts, state: &Arc<S>) -> Result<Self, Self::Rejection> {
        let cookie_key = state.get_cookie_key().clone();
        let cookie_jar = parts.extract::<Cookies>().await?.0;
        let token = parts
            .headers
            .get("authorization")
            .and_then(|header| header.to_str().ok().and_then(Token::try_parse));
        Ok(Self {
            cookie_domain: state.get_domain(),
            cookie_id_name: state.get_id_cookie_name(),
            cookie_key,
            cookie_jar,
            token,
        })
    }
}

impl AuthData {
    /// Creates a cookie
    fn build_cookie<'data>(
        domain: &str,
        name: Cow<'data, str>,
        value: Cow<'data, str>,
        is_delete_flag: bool,
    ) -> Cookie<'static> {
        let is_local = domain == "localhost";
        let mut builder = Cookie::build((name.into_owned(), value.into_owned()))
            .domain(domain.to_string())
            .path("/")
            .same_site(SameSite::Strict)
            .secure(!is_local)
            .http_only(true);
        if is_delete_flag {
            builder = builder.expires(Expiration::DateTime(OffsetDateTime::UNIX_EPOCH));
        }
        builder.build()
    }

    /// Makes a private cookie by encrypting it using the private cookie jar
    /// Returns the encrypted cooke
    fn make_private_cookie(&mut self, name: &str, cookie: Cookie<'static>) -> Cookie<'static> {
        self.cookie_jar.private_mut(&self.cookie_key).add(cookie);
        self.cookie_jar.get(name).unwrap().clone()
    }

    /// Creates a cookie to be returned on the HTTP response
    pub fn create_cookie(&mut self, name: &str, value: &str, is_private: bool) -> Cookie<'static> {
        let cookie = Self::build_cookie(&self.cookie_domain, Cow::Borrowed(name), Cow::Borrowed(value), false);
        if is_private {
            self.make_private_cookie(name, cookie)
        } else {
            cookie
        }
    }

    /// Creates an expired cookie to be return on the HTTP response to unset it
    pub fn create_expired_cookie(&mut self, name: &str, is_private: bool) -> Cookie<'static> {
        let cookie = Self::build_cookie(&self.cookie_domain, Cow::Borrowed(name), Cow::Borrowed(""), true);
        if is_private {
            self.make_private_cookie(name, cookie)
        } else {
            cookie
        }
    }

    /// Creates an identification cookie to be returned on the HTTP response
    ///
    /// # Panics
    ///
    /// Panic when the value cannot be serialized to JSON
    pub fn create_id_cookie(&mut self, value: &Authentication) -> Cookie<'static> {
        self.create_cookie(&self.cookie_id_name.clone(), &serde_json::to_string(value).unwrap(), true)
    }

    /// Creates an expired identification cookie to be returned on the HTTP response to unset it
    pub fn create_expired_id_cookie(&mut self) -> Cookie<'static> {
        self.create_expired_cookie(&self.cookie_id_name.clone(), true)
    }

    /// Try to authenticate this request
    ///
    /// # Errors
    ///
    /// Propagates the error from the `check_token` callback.
    pub fn try_authenticate_cookie(&self) -> Result<Option<Authentication>, serde_json::Error> {
        // try the cookie
        self.cookie_jar
            .private(&self.cookie_key)
            .get(&self.cookie_id_name)
            .map(|cookie| serde_json::from_str(cookie.value()))
            .transpose()
    }
}
