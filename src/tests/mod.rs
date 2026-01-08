/*******************************************************************************
 * Copyright (c) 2024 Cénotélie Opérations SAS (cenotelie.fr)
 ******************************************************************************/

//! Tests

use std::future::Future;
use std::sync::Arc;

use chrono::Local;
use tokio::runtime::Builder;

use crate::application::Application;
use crate::model::auth::ROLE_ADMIN;
use crate::services::ServiceProvider;
use crate::utils::apierror::ApiError;
use crate::utils::axum::auth::{AuthData, Token};
use crate::utils::token::{generate_token, hash_token};

pub mod mocks;
pub mod security;

pub const ADMIN_UID: i64 = 1;
pub const ADMIN_NAME: &str = "admin";

/// Wrapper for async tests
pub fn async_test<F, FUT>(payload: F) -> Result<(), ApiError>
where
    F: FnOnce(Arc<Application>, AuthData) -> FUT,
    FUT: Future<Output = Result<(), ApiError>>,
{
    let runtime = Builder::new_current_thread().enable_all().build()?;
    runtime.block_on(async move {
        let application = Application::launch::<mocks::MockService>(mocks::MockService::get_configuration().await?).await?;
        println!("data_dir={}", &application.configuration.data_dir);
        // create the first user ad admin and its token
        setup_create_admin(&application, ADMIN_NAME).await?;
        let token_secret = setup_create_token(&application, ADMIN_UID, true, true).await?;
        let admin_auth = AuthData::from(Token {
            id: String::from("admin"),
            secret: token_secret,
        });
        let r = payload(application.clone(), admin_auth).await;
        tokio::fs::remove_dir_all(&application.configuration.data_dir).await.unwrap();
        r
    })?;
    Ok(())
}

pub async fn setup_create_admin(application: &Application, name: &str) -> Result<(), ApiError> {
    setup_create_user(application, name, ROLE_ADMIN).await
}

pub async fn setup_create_user(application: &Application, name: &str, roles: &str) -> Result<(), ApiError> {
    setup_create_user_base(application, name, true, roles).await
}

pub async fn setup_create_user_inactive(application: &Application, name: &str, roles: &str) -> Result<(), ApiError> {
    setup_create_user_base(application, name, false, roles).await
}

pub async fn setup_create_user_base(
    application: &Application,
    name: &str,
    is_active: bool,
    roles: &str,
) -> Result<(), ApiError> {
    application
        .db_transaction_write("setup_add_admin", |app| async move {
            sqlx::query("INSERT INTO RegistryUser (isActive, email, login, name, roles) VALUES ($2, $1, $1, $1, $3)")
                .bind(name)
                .bind(is_active)
                .bind(roles)
                .execute(&mut *app.database.transaction.borrow().await)
                .await
        })
        .await?;
    Ok(())
}

pub async fn setup_create_token(
    application: &Application,
    uid: i64,
    can_write: bool,
    can_admin: bool,
) -> Result<String, ApiError> {
    let token_name = generate_token(16);
    let token_secret = generate_token(16);
    application.db_transaction_write("setup_create_token", {
        let token_secret = hash_token(&token_secret);
        |app| async move {
            sqlx::query("INSERT INTO RegistryUserToken (user, name, token, lastUsed, canWrite, canAdmin) VALUES ($1, $2, $3, $4, $5, $6)")
            .bind(uid)
            .bind(token_name)
            .bind(token_secret)
            .bind(Local::now().naive_local())
            .bind(can_write)
            .bind(can_admin)
            .execute(&mut *app.database.transaction.borrow().await).await
        }
    }).await?;
    Ok(token_secret)
}
