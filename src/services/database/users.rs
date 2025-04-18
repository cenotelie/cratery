/*******************************************************************************
 * Copyright (c) 2024 Cénotélie Opérations SAS (cenotelie.fr)
 ******************************************************************************/

//! Service for persisting information in the database
//! API related to the management of users and authentication

use std::future::Future;

use chrono::Local;

use super::Database;
use crate::model::auth::{
    Authentication, AuthenticationPrincipal, OAuthToken, ROLE_ADMIN, RegistryUserToken, RegistryUserTokenWithSecret, TokenKind,
    TokenUsage, find_field_in_blob,
};
use crate::model::cargo::RegistryUser;
use crate::model::config::Configuration;
use crate::model::namegen::generate_name;
use crate::utils::apierror::{
    ApiError, error_conflict, error_forbidden, error_invalid_request, error_not_found, error_unauthorized, specialize,
};
use crate::utils::token::{check_hash, generate_token, hash_token};

impl Database {
    /// Retrieves a user profile
    pub async fn get_user_profile(&self, uid: i64) -> Result<RegistryUser, ApiError> {
        let maybe_row = sqlx::query_as!(
            RegistryUser,
            "SELECT id, isActive AS is_active, email, login, name, roles FROM RegistryUser WHERE id = $1",
            uid
        )
        .fetch_optional(&mut *self.transaction.borrow().await)
        .await?;
        maybe_row.ok_or_else(error_not_found)
    }

    /// Attempts to login using an OAuth code
    pub async fn login_with_oauth_code(&self, configuration: &Configuration, code: &str) -> Result<RegistryUser, ApiError> {
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
            .await?;
        if !response.status().is_success() {
            return Err(specialize(error_unauthorized(), String::from("authentication failed")));
        }
        let body = response.bytes().await?;
        let token = serde_json::from_slice::<OAuthToken>(&body)?;

        // retrieve the user profile
        let response = client
            .get(&configuration.oauth_userinfo_uri)
            .header("authorization", format!("Bearer {}", token.access_token))
            .send()
            .await?;
        if !response.status().is_success() {
            return Err(specialize(error_unauthorized(), String::from("authentication failed")));
        }
        let body = response.bytes().await?;
        let user_info = serde_json::from_slice::<serde_json::Value>(&body)?;
        let email = find_field_in_blob(&user_info, &configuration.oauth_userinfo_path_email).ok_or_else(error_unauthorized)?;

        // resolve the user
        let row = sqlx::query!(
            "SELECT id, isActive AS is_active, login, name, roles FROM RegistryUser WHERE email = $1 LIMIT 1",
            email
        )
        .fetch_optional(&mut *self.transaction.borrow().await)
        .await?;
        if let Some(row) = row {
            if !row.is_active {
                return Err(specialize(error_unauthorized(), String::from("inactive user")));
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
            .await?
            .count;
        let mut login = email[..email.find('@').unwrap()].to_string();
        while sqlx::query!("SELECT COUNT(id) AS count FROM RegistryUser WHERE login = $1", login)
            .fetch_one(&mut *self.transaction.borrow().await)
            .await?
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
        .await?
        .id;
        Ok(RegistryUser {
            id,
            is_active: true,
            email: email.to_string(),
            name: login.to_string(),
            login,
            roles: roles.to_string(),
        })
    }

    /// Gets the known users
    pub async fn get_users(&self) -> Result<Vec<RegistryUser>, ApiError> {
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
    ) -> Result<RegistryUser, ApiError> {
        let row = sqlx::query!("SELECT login, roles FROM RegistryUser WHERE id = $1 LIMIT 1", target.id)
            .fetch_optional(&mut *self.transaction.borrow().await)
            .await?
            .ok_or_else(error_not_found)?;
        let old_roles = row.roles;
        if !can_admin && target.roles != old_roles {
            // not admin and changing roles
            return Err(specialize(error_forbidden(), String::from("only admins can change roles")));
        }
        if can_admin && target.id == principal_uid && target.roles.split(',').all(|role| role.trim() != ROLE_ADMIN) {
            // admin and removing admin role from self
            return Err(specialize(error_forbidden(), String::from("admins cannot remove themselves")));
        }
        if target.login.is_empty() {
            return Err(specialize(error_invalid_request(), String::from("login cannot be empty")));
        }
        if row.login != target.login {
            // check that the new login is available
            if sqlx::query!("SELECT COUNT(id) AS count FROM RegistryUser WHERE login = $1", target.login)
                .fetch_one(&mut *self.transaction.borrow().await)
                .await?
                .count
                != 0
            {
                return Err(specialize(
                    error_conflict(),
                    String::from("the specified login is not available"),
                ));
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
        .await?;
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
    pub async fn reactivate_user(&self, target: &str) -> Result<(), ApiError> {
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
    pub async fn get_tokens(&self, uid: i64) -> Result<Vec<RegistryUserToken>, ApiError> {
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
    ) -> Result<RegistryUserTokenWithSecret, ApiError> {
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
    pub async fn revoke_token(&self, uid: i64, token_id: i64) -> Result<(), ApiError> {
        sqlx::query!("DELETE FROM RegistryUserToken WHERE user = $1 AND id = $2", uid, token_id)
            .execute(&mut *self.transaction.borrow().await)
            .await?;
        Ok(())
    }

    /// Checks an authentication request with a token
    pub async fn check_token<F, FUT>(&self, login: &str, token_secret: &str, on_usage: &F) -> Result<Authentication, ApiError>
    where
        F: Fn(TokenUsage) -> FUT + Sync,
        FUT: Future<Output = ()>,
    {
        if let Some(auth) = self.check_token_global(login, token_secret, &on_usage).await? {
            return Ok(auth);
        }
        if let Some(auth) = self.check_token_user(login, token_secret, &on_usage).await? {
            return Ok(auth);
        }
        Err(error_unauthorized())
    }

    /// Checks whether the information provided is a user token
    async fn check_token_user<F, FUT>(
        &self,
        login: &str,
        token_secret: &str,
        on_usage: &F,
    ) -> Result<Option<Authentication>, ApiError>
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
    ) -> Result<Option<Authentication>, ApiError>
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
    pub async fn update_token_last_usage(&self, event: &TokenUsage) -> Result<(), ApiError> {
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
