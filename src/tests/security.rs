/*******************************************************************************
 * Copyright (c) 2024 Cénotélie Opérations SAS (cenotelie.fr)
 ******************************************************************************/

//! Tests about security checks

use super::async_test;
use crate::utils::apierror::ApiError;

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
