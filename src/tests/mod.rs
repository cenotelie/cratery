/*******************************************************************************
 * Copyright (c) 2024 Cénotélie Opérations SAS (cenotelie.fr)
 ******************************************************************************/

//! Tests

use std::future::Future;
use std::sync::Arc;

use chrono::Local;
use tokio::runtime::Builder;

use crate::application::Application;
use crate::utils::apierror::ApiError;
use crate::utils::axum::auth::{AuthData, Token};
use crate::utils::token::{generate_token, hash_token};

pub mod mocks;
pub mod security;

/// Wrapper for async tests
pub fn async_test<F, FUT>(payload: F) -> Result<(), ApiError>
where
    F: FnOnce(Arc<Application>, AuthData) -> FUT,
    FUT: Future<Output = Result<(), ApiError>>,
{
    let runtime = Builder::new_current_thread().enable_all().build()?;
    runtime.block_on(async move {
        let application = Application::launch::<mocks::MockService>().await?;
        println!("data_dir={}", &application.configuration.data_dir);
        setup_add_admin(&application).await?;
        let token_secret = setup_create_token(&application, 1, true, true).await?;
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

/// Adds an admin user
pub async fn setup_add_admin(application: &Application) -> Result<(), ApiError> {
    application.db_transaction_write("setup_add_admin", |app| async move {
        sqlx::query("INSERT INTO RegistryUser (isActive, email, login, name, roles) VALUES (TRUE, 'admin', 'admin', 'admin', 'admin')").execute(&mut *app.database.transaction.borrow().await).await?;
        Ok::<(), ApiError>(())
    }).await?;
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
            .execute(&mut *app.database.transaction.borrow().await).await?;
            Ok::<(), ApiError>(())
        }
    }).await?;
    Ok(token_secret)
}
