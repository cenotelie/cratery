/*******************************************************************************
 * Copyright (c) 2024 Cénotélie Opérations SAS (cenotelie.fr)
 ******************************************************************************/

//! Tests about security checks

use super::{async_test, setup_create_user_inactive};
use crate::application::Application;
use crate::model::auth::ROLE_ADMIN;
use crate::tests::{setup_create_token, setup_create_user, ADMIN_NAME, ADMIN_UID};
use crate::utils::apierror::ApiError;
use crate::utils::axum::auth::{AuthData, Token};

/// Creates authentication data for the admin in read-only
async fn create_auth_admin_ro(application: &Application) -> Result<AuthData, ApiError> {
    let admin_ro_token = setup_create_token(application, ADMIN_UID, false, false).await?;
    Ok(AuthData::from(Token {
        id: ADMIN_NAME.to_string(),
        secret: admin_ro_token,
    }))
}

const USER_UID: i64 = 2;
const USER_NAME: &str = "user";

/// Creates an authentication for a user in read-only
async fn create_auth_user_ro(application: &Application) -> Result<AuthData, ApiError> {
    setup_create_user(application, USER_NAME, "").await?;
    let user_token = setup_create_token(application, USER_UID, false, false).await?;
    Ok(AuthData::from(Token {
        id: String::from("user"),
        secret: user_token,
    }))
}

#[test]
fn test_basic_auth_admin_token() -> Result<(), ApiError> {
    async_test(|application, admin_auth| async move {
        let authentication = application.authenticate(&admin_auth).await?;
        assert_eq!(1, authentication.uid()?);
        assert!(authentication.can_write);
        assert!(authentication.can_admin);
        Ok(())
    })
}

#[test]
fn test_inactive_no_auth() -> Result<(), ApiError> {
    async_test(|application, _admin_auth| async move {
        setup_create_user_inactive(&application, "user", "").await?;
        let token = setup_create_token(&application, 2, false, false).await?;
        assert!(application
            .get_current_user(&AuthData::from(Token {
                id: String::from("user"),
                secret: token,
            }))
            .await
            .is_err());
        Ok(())
    })
}

#[test]
fn test_get_registry_information_needs_auth() -> Result<(), ApiError> {
    async_test(|application, admin_auth| async move {
        assert!(application.get_registry_information(&AuthData::default()).await.is_err());
        assert!(application.get_registry_information(&admin_auth).await.is_ok());
        // test with read-only token
        assert!(application
            .get_registry_information(&create_auth_user_ro(&application).await?)
            .await
            .is_ok());
        Ok(())
    })
}

#[test]
fn test_get_get_current_user() -> Result<(), ApiError> {
    async_test(|application, admin_auth| async move {
        assert!(application.get_current_user(&AuthData::default()).await.is_err());
        assert!(application.get_current_user(&admin_auth).await.is_ok());
        // test user without admin
        let data = application
            .get_current_user(&create_auth_user_ro(&application).await?)
            .await?;
        assert_eq!(data.id, USER_UID);
        assert_eq!(&data.name, USER_NAME);
        assert_eq!(&data.login, USER_NAME);
        assert_eq!(&data.email, USER_NAME);
        assert!(data.is_active);
        assert_eq!(&data.roles, "");

        // test admin in read-only
        let data = application
            .get_current_user(&create_auth_admin_ro(&application).await?)
            .await?;
        assert_eq!(data.id, 1);
        assert_eq!(&data.name, ADMIN_NAME);
        assert_eq!(&data.login, ADMIN_NAME);
        assert_eq!(&data.email, ADMIN_NAME);
        assert!(data.is_active);
        assert_eq!(&data.roles, ROLE_ADMIN);

        Ok(())
    })
}

#[test]
fn test_get_users_admin_only() -> Result<(), ApiError> {
    async_test(|application, admin_auth| async move {
        assert!(application.get_users(&AuthData::default()).await.is_err());
        assert!(application.get_users(&admin_auth).await.is_ok());
        // test user without admin
        assert!(application
            .get_users(&create_auth_user_ro(&application).await?)
            .await
            .is_err());
        // test admin in read-only
        assert!(application
            .get_users(&create_auth_admin_ro(&application).await?)
            .await
            .is_err());
        Ok(())
    })
}
