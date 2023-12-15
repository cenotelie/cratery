//! Module for the application

use cenotelie_lib_apierror::{error_forbidden, error_invalid_request, error_not_found, specialize, ApiError};
use chrono::Local;

use crate::objects::{
    AuthenticatedUser, CrateUploadData, CrateUploadResult, DocsGenerationJob, OwnersQueryResult, RegistryUser,
    SearchResultCrate, SearchResults, SearchResultsMeta, YesNoMsgResult, YesNoResult,
};

use super::Application;

impl<'c> Application<'c> {
    /// Search for crates
    pub async fn search(&self, query: &str, per_page: Option<usize>) -> Result<SearchResults, ApiError> {
        let per_page = match per_page {
            None => 10,
            Some(value) if value > 100 => 100,
            Some(value) => value,
        };
        let pattern = format!("%{query}%");
        let rows = sqlx::query!("SELECT name From Package WHERE name LIKE $1", pattern)
            .fetch_all(&mut *self.transaction.borrow().await)
            .await?;
        let mut crates = Vec::new();
        for row_name in rows {
            let row = sqlx::query!("SELECT version, description FROM PackageVersion WHERE package = $1 AND yanked = FALSE ORDER BY id DESC LIMIT 1", row_name.name).fetch_optional(&mut *self.transaction.borrow().await).await?;
            if let Some(row) = row {
                crates.push(SearchResultCrate {
                    name: row_name.name,
                    max_version: row.version,
                    description: row.description,
                });
            }
        }
        let total = crates.len();
        Ok(SearchResults {
            crates: if per_page > crates.len() {
                crates.into_iter().take(per_page).collect()
            } else {
                crates
            },
            meta: SearchResultsMeta { total },
        })
    }

    /// Publish a crate
    #[allow(clippy::similar_names)]
    pub async fn publish(
        &self,
        authenticated_user: &AuthenticatedUser,
        package: &CrateUploadData,
    ) -> Result<CrateUploadResult, ApiError> {
        let uid = self.check_is_user(&authenticated_user.principal).await?;
        if !authenticated_user.can_write {
            return Err(specialize(
                error_forbidden(),
                String::from("writing is forbidden for this authentication"),
            ));
        }
        let warnings = package.metadata.validate()?;
        let lowercase = package.metadata.name.to_ascii_lowercase();
        let row = sqlx::query!(
            "SELECT upload FROM PackageVersion WHERE package = $1 AND version = $2 LIMIT 1",
            package.metadata.name,
            package.metadata.vers
        )
        .fetch_optional(&mut *self.transaction.borrow().await)
        .await?;
        if let Some(row) = row {
            return Err(specialize(
                error_invalid_request(),
                format!(
                    "Package {} already exists in version {}, uploaded on {}",
                    &package.metadata.name, &package.metadata.vers, row.upload
                ),
            ));
        }
        // check whether the package already exists
        let row = sqlx::query!("SELECT name FROM Package WHERE lowercase = $1 LIMIT 1", lowercase)
            .fetch_optional(&mut *self.transaction.borrow().await)
            .await?;
        if let Some(row) = row {
            // check this is the same package
            if row.name != lowercase {
                return Err(specialize(
                    error_invalid_request(),
                    format!("A package named {} already exists", row.name),
                ));
            }
            // check the ownership
            let rows = sqlx::query!("SELECT owner FROM PackageOwner WHERE package = $1", package.metadata.name,)
                .fetch_all(&mut *self.transaction.borrow().await)
                .await?;
            if rows.into_iter().all(|r| r.owner != uid) {
                return Err(specialize(
                    error_forbidden(),
                    String::from("User is not an owner of this package"),
                ));
            }
        } else {
            // create the package
            sqlx::query!(
                "INSERT INTO Package (name, lowercase) VALUES ($1, $2)",
                package.metadata.name,
                lowercase
            )
            .execute(&mut *self.transaction.borrow().await)
            .await?;
            // add the principal as owner
            sqlx::query!(
                "INSERT INTO PackageOwner (package, owner) VALUES ($1, $2)",
                package.metadata.name,
                uid
            )
            .execute(&mut *self.transaction.borrow().await)
            .await?;
        }
        let now = Local::now().naive_local();
        // create the version
        let description = package.metadata.description.as_ref().map_or("", std::string::String::as_str);
        sqlx::query!(
            "INSERT INTO PackageVersion (package, version, description, upload, uploadedBy, yanked, hasDocs) VALUES ($1, $2, $3, $4, $5, false, false)",
            package.metadata.name,
            package.metadata.vers,
            description,
            now,
            uid
        )
        .execute(&mut *self.transaction.borrow().await)
        .await?;
        Ok(warnings)
    }

    /// Checks that a package exists
    pub async fn check_package_exists(&self, package: &str, version: &str) -> Result<(), ApiError> {
        let row = sqlx::query!(
            "SELECT id FROM PackageVersion WHERE package = $1 AND version = $2 LIMIT 1",
            package,
            version
        )
        .fetch_optional(&mut *self.transaction.borrow().await)
        .await?;
        if row.is_none() {
            Err(error_not_found())
        } else {
            Ok(())
        }
    }

    /// Checks the ownership of a package
    async fn check_package_ownership(&self, authenticated_user: &AuthenticatedUser, package: &str) -> Result<i64, ApiError> {
        let uid = self.check_is_user(&authenticated_user.principal).await?;
        if self.check_is_admin(uid).await.is_ok() {
            return Ok(uid);
        }
        let row = sqlx::query!(
            "SELECT id from PackageOwner WHERE package = $1 AND owner = $2 LIMIT 1",
            package,
            uid
        )
        .fetch_optional(&mut *self.transaction.borrow().await)
        .await?;
        match row {
            Some(_) => Ok(uid),
            None => Err(specialize(
                error_forbidden(),
                String::from("User is not an owner of this package"),
            )),
        }
    }

    /// Yank a crate version
    pub async fn yank(
        &self,
        authenticated_user: &AuthenticatedUser,
        package: &str,
        version: &str,
    ) -> Result<YesNoResult, ApiError> {
        if !authenticated_user.can_write {
            return Err(specialize(
                error_forbidden(),
                String::from("writing is forbidden for this authentication"),
            ));
        }
        self.check_package_ownership(authenticated_user, package).await?;
        let row = sqlx::query!(
            "SELECT yanked FROM PackageVersion WHERE package = $1 AND version = $2 LIMIT 1",
            package,
            version
        )
        .fetch_optional(&mut *self.transaction.borrow().await)
        .await?;
        match row {
            None => Err(specialize(
                error_invalid_request(),
                format!("Version {version} of crate {package} does not exist"),
            )),
            Some(row) => {
                if row.yanked {
                    Err(specialize(
                        error_invalid_request(),
                        format!("Version {version} of crate {package} is already yanked"),
                    ))
                } else {
                    sqlx::query!(
                        "UPDATE PackageVersion SET yanked = TRUE WHERE package = $1 AND version = $2",
                        package,
                        version
                    )
                    .execute(&mut *self.transaction.borrow().await)
                    .await?;
                    Ok(YesNoResult::new())
                }
            }
        }
    }

    /// Unyank a crate version
    pub async fn unyank(
        &self,
        authenticated_user: &AuthenticatedUser,
        package: &str,
        version: &str,
    ) -> Result<YesNoResult, ApiError> {
        if !authenticated_user.can_write {
            return Err(specialize(
                error_forbidden(),
                String::from("writing is forbidden for this authentication"),
            ));
        }
        self.check_package_ownership(authenticated_user, package).await?;
        let row = sqlx::query!(
            "SELECT yanked FROM PackageVersion WHERE package = $1 AND version = $2 LIMIT 1",
            package,
            version
        )
        .fetch_optional(&mut *self.transaction.borrow().await)
        .await?;
        match row {
            None => Err(specialize(
                error_invalid_request(),
                format!("Version {version} of crate {package} does not exist"),
            )),
            Some(row) => {
                if row.yanked {
                    sqlx::query!(
                        "UPDATE PackageVersion SET yanked = FALSE WHERE package = $1 AND version = $2",
                        package,
                        version
                    )
                    .execute(&mut *self.transaction.borrow().await)
                    .await?;
                    Ok(YesNoResult::new())
                } else {
                    Err(specialize(
                        error_invalid_request(),
                        format!("Version {version} of crate {package} is not yanked"),
                    ))
                }
            }
        }
    }

    /// Gets the packages that need documentation generation
    pub async fn get_undocumented_packages(&self) -> Result<Vec<DocsGenerationJob>, ApiError> {
        let rows = sqlx::query!("SELECT package, version FROM PackageVersion WHERE hasDocs = FALSE ORDER BY id")
            .fetch_all(&mut *self.transaction.borrow().await)
            .await?;
        Ok(rows
            .into_iter()
            .map(|row| DocsGenerationJob {
                crate_name: row.package,
                crate_version: row.version,
            })
            .collect())
    }

    /// Sets a package as having documentation
    pub async fn _set_package_documented(&self, package: &str, version: &str) -> Result<(), ApiError> {
        sqlx::query!(
            "UPDATE PackageVersion SET hasDocs = TRUE WHERE package = $1 AND version = $2",
            package,
            version
        )
        .execute(&mut *self.transaction.borrow().await)
        .await?;
        Ok(())
    }

    /// Gets the list of owners for a package
    pub async fn get_owners(
        &self,
        authenticated_user: &AuthenticatedUser,
        package: &str,
    ) -> Result<OwnersQueryResult, ApiError> {
        self.check_is_user(&authenticated_user.principal).await?;
        let users = sqlx::query_as!(RegistryUser, "SELECT RegistryUser.id, isActive AS is_active, email, login, name, roles FROM RegistryUser INNER JOIN PackageOwner ON PackageOwner.owner = RegistryUser.id WHERE package = $1", package)
            .fetch_all(&mut *self.transaction.borrow().await).await?;
        Ok(OwnersQueryResult { users })
    }

    /// Add owners to a package
    pub async fn add_owners(
        &self,
        authenticated_user: &AuthenticatedUser,
        package: &str,
        new_users: &[String],
    ) -> Result<YesNoMsgResult, ApiError> {
        if !authenticated_user.can_admin {
            return Err(specialize(
                error_forbidden(),
                String::from("administration is forbidden for this authentication"),
            ));
        }
        // check access
        self.check_package_ownership(authenticated_user, package).await?;
        // get all current owners
        let rows = sqlx::query!("SELECT owner FROM PackageOwner WHERE package = $1", package,)
            .fetch_all(&mut *self.transaction.borrow().await)
            .await?;
        // add new users
        let mut added = Vec::new();
        for new_user in new_users {
            let new_uid = self.check_is_user(new_user).await?;
            if rows.iter().all(|r| r.owner != new_uid) {
                // not already an owner
                sqlx::query!("INSERT INTO PackageOwner (package, owner) VALUES ($1, $2)", package, new_uid)
                    .execute(&mut *self.transaction.borrow().await)
                    .await?;
                added.push(new_user.as_str());
            }
        }
        let msg = format!(
            "User(s) {} has(-ve) been invited to be an owner of crate {}",
            added.join(", "),
            package
        );
        Ok(YesNoMsgResult::new(msg))
    }

    /// Remove owners from a package
    pub async fn remove_owners(
        &self,
        authenticated_user: &AuthenticatedUser,
        package: &str,
        old_users: &[String],
    ) -> Result<YesNoResult, ApiError> {
        if !authenticated_user.can_admin {
            return Err(specialize(
                error_forbidden(),
                String::from("administration is forbidden for this authentication"),
            ));
        }
        // check access
        self.check_package_ownership(authenticated_user, package).await?;
        // get all current owners
        let rows = sqlx::query!("SELECT owner FROM PackageOwner WHERE package = $1", package,)
            .fetch_all(&mut *self.transaction.borrow().await)
            .await?;
        let mut current_owners: Vec<i64> = rows.into_iter().map(|r| r.owner).collect();
        // remove old users
        for old_user in old_users {
            let old_uid = self.check_is_user(old_user).await?;
            let index = current_owners.iter().enumerate().find(|(_, &x)| x == old_uid).map(|(i, _)| i);
            if let Some(index) = index {
                if current_owners.len() == 1 {
                    // cannot remove the last one
                    return Err(specialize(error_invalid_request(), String::from("Cannot remove all owners")));
                }
                // not already an owner
                sqlx::query!("DELETE FROM PackageOwner WHERE package = $1 AND owner = $2", package, old_uid)
                    .execute(&mut *self.transaction.borrow().await)
                    .await?;
                current_owners.remove(index);
            }
        }
        Ok(YesNoResult::new())
    }
}
