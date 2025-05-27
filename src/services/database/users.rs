/*******************************************************************************
 * Copyright (c) 2024 Cénotélie Opérations SAS (cenotelie.fr)
 ******************************************************************************/

//! Service for persisting information in the database
//! API related to the management of users and authentication

use std::future::Future;

use axum::http::StatusCode;
use chrono::Local;
use thiserror::Error;

use super::Database;
use crate::application::AuthenticationError;
use crate::model::auth::{
    Authentication, AuthenticationPrincipal, OAuthToken, ROLE_ADMIN, RegistryUserToken, RegistryUserTokenWithSecret, TokenKind,
    TokenUsage, find_field_in_blob,
};
use crate::model::cargo::RegistryUser;
use crate::model::config::Configuration;
use crate::model::namegen::generate_name;
use crate::utils::apierror::{ApiError, AsStatusCode, error_forbidden, error_not_found, specialize};
use crate::utils::token::{check_hash, generate_token, hash_token};

#[derive(Debug, Error)]
pub enum UserError {
    #[error("failed to execute sql request to get user profile for `{uid}`")]
    SqlxGetUserProfile {
        #[source]
        source: sqlx::Error,
        uid: i64,
    },

    #[error("user with uid `{uid}` not found")]
    UserNotFound { uid: i64 },
}

impl AsStatusCode for UserError {}

#[derive(Debug, Error)]
pub enum UpdateUserError {
    #[error("failed to execute request for get login and roles on DB")]
    SqlLoginAndRoles(#[source] sqlx::Error),

    #[error("user with id `{uid}`not found")]
    UserNotFound { uid: i64 },

    #[error("only admins can change roles")]
    OnlyAdminCanChangeRoles,

    #[error("admins cannot remove themselves")]
    AdminCantRemoveThemselves,

    #[error("login cannot be empty")]
    LoginCannotBeEmpty,

    #[error("failed to execute request for count RegistryUser for login")]
    CountUserForLogin(#[source] sqlx::Error),

    #[error("the specified login `{login}` is not found in db")]
    LoginNotAvailable { login: String },

    #[error("failed to execute db request to update user")]
    UpdateUserSqlx(#[source] sqlx::Error),

    #[error("user can't update user")]
    Authentication(#[from] AuthenticationError),
}

impl AsStatusCode for UpdateUserError {
    fn status_code(&self) -> StatusCode {
        match self {
            Self::SqlLoginAndRoles(_) | Self::CountUserForLogin(_) | Self::UpdateUserSqlx(_) | Self::Authentication(_) => {
                StatusCode::INTERNAL_SERVER_ERROR
            }

            Self::AdminCantRemoveThemselves | Self::OnlyAdminCanChangeRoles => StatusCode::FORBIDDEN,
            Self::LoginCannotBeEmpty => StatusCode::BAD_REQUEST,
            Self::UserNotFound { .. } => StatusCode::NOT_FOUND,

            Self::LoginNotAvailable { .. } => StatusCode::CONFLICT,
        }
    }
}

#[derive(Debug, Error)]
pub enum OAuthLoginError {
    #[error("failed to retrieve token from '{oauth_token_uri}'")]
    GetRetrieveToken {
        source: reqwest::Error,
        oauth_token_uri: String,
    },

    #[error("failed to get retrieve token response")]
    RetrieveTokenResponse { source: reqwest::Error },

    #[error("failed to parse retrieve token response")]
    ParseRetrieveTokenResponse { source: serde_json::Error, body: String },

    #[error("failed to retried token for authentication: {status} '{body}'")]
    RetrieveTokenFailed { status: StatusCode, body: String },

    #[error("failed to autentify at '{uri}': {status}")]
    RetrieveUserProfile { status: StatusCode, uri: String },

    #[error("failed to retrieve user profile from '{oauth_userinfo_uri}'")]
    GetUserProfile {
        source: reqwest::Error,
        oauth_userinfo_uri: String,
    },

    #[error("failed to get user profile response")]
    GetUserProfileResponse { source: reqwest::Error },

    #[error("failed to parse get user profile response")]
    ParseGetUserProfileResponse { source: serde_json::Error, body: String },

    #[error("failed to execute user database request")]
    UserDatabaseRequest(#[source] sqlx::Error),

    #[error("inactive user")]
    InactiveUser,

    #[error("no email in returned user info:\n{0}")]
    EmailMissingInUserInfo(String),
}
impl AsStatusCode for OAuthLoginError {
    fn status_code(&self) -> StatusCode {
        match self {
            Self::EmailMissingInUserInfo(_)
            | Self::RetrieveTokenFailed { .. }
            | Self::RetrieveUserProfile { .. }
            | Self::InactiveUser => StatusCode::UNAUTHORIZED,

            Self::GetRetrieveToken { .. }
            | Self::RetrieveTokenResponse { .. }
            | Self::ParseRetrieveTokenResponse { .. }
            | Self::GetUserProfile { .. }
            | Self::GetUserProfileResponse { .. }
            | Self::ParseGetUserProfileResponse { .. }
            | Self::UserDatabaseRequest(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
}

impl Database {
    /// Retrieves a user profile
    pub async fn get_user_profile(&self, uid: i64) -> Result<RegistryUser, UserError> {
        let maybe_row = sqlx::query_as!(
            RegistryUser,
            "SELECT id, isActive AS is_active, email, login, name, roles FROM RegistryUser WHERE id = $1",
            uid
        )
        .fetch_optional(&mut *self.transaction.borrow().await)
        .await
        .map_err(|source| UserError::SqlxGetUserProfile { source, uid })?;
        maybe_row.ok_or(UserError::UserNotFound { uid })
    }

    /// Attempts to login using an OAuth code
    #[expect(clippy::too_many_lines)]
    pub async fn login_with_oauth_code(
        &self,
        configuration: &Configuration,
        code: &str,
    ) -> Result<RegistryUser, OAuthLoginError> {
        let client = reqwest::Client::new();
        // retrieve the token
        let response = client
            .post(&configuration.oauth_token_uri)
            .form(&[
                ("grant_type", "authorization_code"),
                ("code", code),
                ("redirect_uri", &configuration.oauth_callback_uri),
                ("client_id", &configuration.oauth_client_id),
                ("client_secret", &configuration.oauth_client_secret),
            ])
            .header(reqwest::header::ACCEPT, "application/json")
            .send()
            .await
            .map_err(|source| OAuthLoginError::GetRetrieveToken {
                source,
                oauth_token_uri: configuration.oauth_token_uri.as_str().into(),
            })?;
        if !response.status().is_success() {
            let status = response.status();
            let body = response.bytes().await.unwrap();
            return Err(OAuthLoginError::RetrieveTokenFailed {
                status,
                body: String::from_utf8_lossy(&body).into(),
            });
        }
        let body = response
            .bytes()
            .await
            .map_err(|source| OAuthLoginError::RetrieveTokenResponse { source })?;
        let token =
            serde_json::from_slice::<OAuthToken>(&body).map_err(|source| OAuthLoginError::ParseRetrieveTokenResponse {
                source,
                body: String::from_utf8_lossy(&body).into(),
            })?;

        // retrieve the user profile
        let response = client
            .get(&configuration.oauth_userinfo_uri)
            .header("authorization", format!("Bearer {}", token.access_token))
            .send()
            .await
            .map_err(|source| OAuthLoginError::GetUserProfile {
                source,
                oauth_userinfo_uri: configuration.oauth_userinfo_uri.as_str().into(),
            })?;
        if !response.status().is_success() {
            let status = response.status();
            return Err(OAuthLoginError::RetrieveUserProfile {
                status,
                uri: configuration.oauth_userinfo_uri.as_str().into(),
            });
        }
        let body = response
            .bytes()
            .await
            .map_err(|source| OAuthLoginError::GetUserProfileResponse { source })?;
        let user_info = serde_json::from_slice::<serde_json::Value>(&body).map_err(|source| {
            OAuthLoginError::ParseGetUserProfileResponse {
                source,
                body: String::from_utf8_lossy(&body).into(),
            }
        })?;
        let email = find_field_in_blob(&user_info, &configuration.oauth_userinfo_path_email)
            .ok_or_else(|| OAuthLoginError::EmailMissingInUserInfo(user_info.to_string()))?;

        // resolve the user
        let row = sqlx::query!(
            "SELECT id, isActive AS is_active, login, name, roles FROM RegistryUser WHERE email = $1 LIMIT 1",
            email
        )
        .fetch_optional(&mut *self.transaction.borrow().await)
        .await
        .map_err(OAuthLoginError::UserDatabaseRequest)?;
        if let Some(row) = row {
            if !row.is_active {
                return Err(OAuthLoginError::InactiveUser);
            }
            // already exists
            return Ok(RegistryUser {
                id: row.id,
                is_active: true,
                email: email.to_string(),
                login: row.login,
                name: row.name,
                roles: row.roles,
            });
        }
        // create the user
        let count = sqlx::query!("SELECT COUNT(id) AS count FROM RegistryUser")
            .fetch_one(&mut *self.transaction.borrow().await)
            .await
            .map_err(OAuthLoginError::UserDatabaseRequest)?
            .count;
        let mut login = email[..email.find('@').unwrap()].to_string();
        while sqlx::query!("SELECT COUNT(id) AS count FROM RegistryUser WHERE login = $1", login)
            .fetch_one(&mut *self.transaction.borrow().await)
            .await
            .map_err(OAuthLoginError::UserDatabaseRequest)?
            .count
            != 0
        {
            login = generate_name();
        }
        let full_name = find_field_in_blob(&user_info, &configuration.oauth_userinfo_path_fullname).unwrap_or(&login);
        let roles = if count == 0 { ROLE_ADMIN } else { "" };
        let id = sqlx::query!(
            "INSERT INTO RegistryUser (isActive, email, login, name, roles) VALUES (TRUE, $1, $2, $3, $4) RETURNING id",
            email,
            login,
            full_name,
            roles
        )
        .fetch_one(&mut *self.transaction.borrow().await)
        .await
        .map_err(OAuthLoginError::UserDatabaseRequest)?
        .id;
        Ok(RegistryUser {
            id,
            is_active: true,
            email: email.to_string(),
            name: login.clone(),
            login,
            roles: roles.to_string(),
        })
    }

    /// Gets the known users
    pub async fn get_users(&self) -> Result<Vec<RegistryUser>, sqlx::Error> {
        let rows = sqlx::query_as!(
            RegistryUser,
            "SELECT id, isActive AS is_active, email, login, name, roles FROM RegistryUser ORDER BY login",
        )
        .fetch_all(&mut *self.transaction.borrow().await)
        .await?;
        Ok(rows)
    }

    /// Updates the information of a user
    pub async fn update_user(
        &self,
        principal_uid: i64,
        target: &RegistryUser,
        can_admin: bool,
    ) -> Result<RegistryUser, UpdateUserError> {
        let row = sqlx::query!("SELECT login, roles FROM RegistryUser WHERE id = $1 LIMIT 1", target.id)
            .fetch_optional(&mut *self.transaction.borrow().await)
            .await
            .map_err(UpdateUserError::SqlLoginAndRoles)?
            .ok_or_else(|| UpdateUserError::UserNotFound { uid: target.id })?;
        let old_roles = row.roles;
        if !can_admin && target.roles != old_roles {
            // not admin and changing roles
            return Err(UpdateUserError::OnlyAdminCanChangeRoles);
        }
        if can_admin && target.id == principal_uid && target.roles.split(',').all(|role| role.trim() != ROLE_ADMIN) {
            // admin and removing admin role from self
            return Err(UpdateUserError::AdminCantRemoveThemselves);
        }
        if target.login.is_empty() {
            return Err(UpdateUserError::LoginCannotBeEmpty);
        }
        if row.login != target.login {
            // check that the new login is available
            if sqlx::query!("SELECT COUNT(id) AS count FROM RegistryUser WHERE login = $1", target.login)
                .fetch_one(&mut *self.transaction.borrow().await)
                .await
                .map_err(UpdateUserError::CountUserForLogin)?
                .count
                != 0
            {
                return Err(UpdateUserError::LoginNotAvailable {
                    login: target.login.clone(),
                });
            }
        }
        sqlx::query!(
            "UPDATE RegistryUser SET login = $2, name = $3, roles = $4 WHERE id = $1",
            target.id,
            target.login,
            target.name,
            target.roles
        )
        .execute(&mut *self.transaction.borrow().await)
        .await
        .map_err(UpdateUserError::UpdateUserSqlx)?;
        Ok(target.clone())
    }

    /// Attempts to deactivate a user
    pub async fn deactivate_user(&self, principal_uid: i64, target: &str) -> Result<(), ApiError> {
        let target_uid = self.check_is_user(target).await?;
        if principal_uid == target_uid {
            // cannot deactivate self
            return Err(specialize(error_forbidden(), String::from("cannot self deactivate")));
        }
        sqlx::query!("UPDATE RegistryUser SET isActive = FALSE WHERE id = $1", target_uid)
            .execute(&mut *self.transaction.borrow().await)
            .await?;
        Ok(())
    }

    /// Attempts to re-activate a user
    pub async fn reactivate_user(&self, target: &str) -> Result<(), sqlx::Error> {
        sqlx::query!("UPDATE RegistryUser SET isActive = TRUE WHERE email = $1", target)
            .execute(&mut *self.transaction.borrow().await)
            .await?;
        Ok(())
    }

    /// Attempts to delete a user
    pub async fn delete_user(&self, principal_uid: i64, target: &str) -> Result<(), ApiError> {
        let target_uid = sqlx::query!("SELECT id FROM RegistryUser WHERE email = $1", target)
            .fetch_optional(&mut *self.transaction.borrow().await)
            .await?
            .ok_or_else(error_not_found)?
            .id;
        if principal_uid == target_uid {
            return Err(specialize(error_forbidden(), String::from("cannot delete self")));
        }
        sqlx::query!("DELETE FROM RegistryUserToken WHERE user = $1", target_uid)
            .execute(&mut *self.transaction.borrow().await)
            .await?;
        sqlx::query!("DELETE FROM PackageOwner WHERE owner = $1", target_uid)
            .execute(&mut *self.transaction.borrow().await)
            .await?;
        sqlx::query!("DELETE FROM RegistryUser WHERE id = $1", target_uid)
            .execute(&mut *self.transaction.borrow().await)
            .await?;
        Ok(())
    }

    /// Gets the tokens for a user
    pub async fn get_tokens(&self, uid: i64) -> Result<Vec<RegistryUserToken>, sqlx::Error> {
        let rows = sqlx::query!(
            "SELECT id, name, lastUsed AS last_used, canWrite AS can_write, canAdmin AS can_admin FROM RegistryUserToken WHERE user = $1 ORDER BY id",
            uid
        )
        .fetch_all(&mut *self.transaction.borrow().await)
        .await?;
        Ok(rows
            .into_iter()
            .map(|row| RegistryUserToken {
                id: row.id,
                name: row.name,
                last_used: row.last_used,
                can_write: row.can_write,
                can_admin: row.can_admin,
            })
            .collect())
    }

    /// Creates a token for the current user
    pub async fn create_token(
        &self,
        uid: i64,
        name: &str,
        can_write: bool,
        can_admin: bool,
    ) -> Result<RegistryUserTokenWithSecret, sqlx::Error> {
        let token_secret = generate_token(64);
        let token_hash = hash_token(&token_secret);
        let now = Local::now().naive_local();
        let id = sqlx::query!(
            "INSERT INTO RegistryUserToken (user, name, token, lastUsed, canWrite, canAdmin) VALUES ($1, $2, $3, $4, $5, $6) RETURNING id",
            uid,
            name,
            token_hash,
            now,
            can_write,
            can_admin
        )
        .fetch_one(&mut *self.transaction.borrow().await)
        .await?
        .id;
        Ok(RegistryUserTokenWithSecret {
            id,
            name: name.to_string(),
            secret: token_secret,
            last_used: now,
            can_write,
            can_admin,
        })
    }

    /// Revoke a previous token
    pub async fn revoke_token(&self, uid: i64, token_id: i64) -> Result<(), sqlx::Error> {
        sqlx::query!("DELETE FROM RegistryUserToken WHERE user = $1 AND id = $2", uid, token_id)
            .execute(&mut *self.transaction.borrow().await)
            .await?;
        Ok(())
    }

    /// Checks an authentication request with a token
    pub async fn check_token<F, FUT>(
        &self,
        login: &str,
        token_secret: &str,
        on_usage: &F,
    ) -> Result<Authentication, AuthenticationError>
    where
        F: Fn(TokenUsage) -> FUT + Sync,
        FUT: Future<Output = ()>,
    {
        if let Some(auth) = self
            .check_token_global(login, token_secret, &on_usage)
            .await
            .map_err(AuthenticationError::GlobalToken)?
        {
            return Ok(auth);
        }
        if let Some(auth) = self
            .check_token_user(login, token_secret, &on_usage)
            .await
            .map_err(AuthenticationError::UserToken)?
        {
            return Ok(auth);
        }
        Err(AuthenticationError::Unauthorized)
    }

    /// Checks whether the information provided is a user token
    async fn check_token_user<F, FUT>(
        &self,
        login: &str,
        token_secret: &str,
        on_usage: &F,
    ) -> Result<Option<Authentication>, sqlx::Error>
    where
        F: Fn(TokenUsage) -> FUT + Sync,
        FUT: Future<Output = ()>,
    {
        let rows = sqlx::query!(
            "SELECT RegistryUser.id AS uid, email, RegistryUserToken.id, token, canWrite AS can_write, canAdmin AS can_admin
            FROM RegistryUser INNER JOIN RegistryUserToken ON RegistryUser.id = RegistryUserToken.user
            WHERE isActive = TRUE AND login = $1",
            login
        )
        .fetch_all(&mut *self.transaction.borrow().await)
        .await?;
        for row in rows {
            if check_hash(token_secret, &row.token).is_ok() {
                let now = Local::now().naive_local();
                on_usage(TokenUsage {
                    kind: TokenKind::User,
                    token_id: row.id,
                    timestamp: now,
                })
                .await;
                return Ok(Some(Authentication {
                    principal: AuthenticationPrincipal::User {
                        uid: row.uid,
                        email: row.email,
                    },
                    can_write: row.can_write,
                    can_admin: row.can_admin,
                }));
            }
        }
        Ok(None)
    }

    /// Checks whether the information provided is for a global token
    async fn check_token_global<F, FUT>(
        &self,
        login: &str,
        token_secret: &str,
        on_usage: &F,
    ) -> Result<Option<Authentication>, sqlx::Error>
    where
        F: Fn(TokenUsage) -> FUT + Sync,
        FUT: Future<Output = ()>,
    {
        let row = sqlx::query!("SELECT id, token FROM RegistryGlobalToken WHERE name = $1 LIMIT 1", login)
            .fetch_optional(&mut *self.transaction.borrow().await)
            .await?;
        let Some(row) = row else { return Ok(None) };
        if check_hash(token_secret, &row.token).is_ok() {
            let now = Local::now().naive_local();
            on_usage(TokenUsage {
                kind: TokenKind::Registry,
                token_id: row.id,
                timestamp: now,
            })
            .await;
            Ok(Some(Authentication::new_service(login.to_string())))
        } else {
            Ok(None)
        }
    }

    /// Updates the last usage of a token
    pub async fn update_token_last_usage(&self, event: &TokenUsage) -> Result<(), sqlx::Error> {
        if event.kind == TokenKind::User {
            sqlx::query!(
                "UPDATE RegistryUserToken SET lastUsed = $2 WHERE id = $1",
                event.token_id,
                event.timestamp
            )
            .execute(&mut *self.transaction.borrow().await)
            .await?;
        } else if event.kind == TokenKind::Registry {
            sqlx::query!(
                "UPDATE RegistryGlobalToken SET lastUsed = $2 WHERE id = $1",
                event.token_id,
                event.timestamp
            )
            .execute(&mut *self.transaction.borrow().await)
            .await?;
        }
        Ok(())
    }
}
