/*******************************************************************************
 * Copyright (c) 2024 Cénotélie Opérations SAS (cenotelie.fr)
 ******************************************************************************/

//! Service for persisting information in the database
//! API related to the management of packages (crates)

use std::collections::hash_map::Entry;
use std::collections::HashMap;

use byteorder::ByteOrder;
use chrono::{Datelike, Duration, Local, NaiveDateTime};
use futures::StreamExt;
use semver::Version;

use super::Database;
use crate::model::auth::Authentication;
use crate::model::cargo::{
    CrateUploadData, CrateUploadResult, IndexCrateMetadata, OwnersQueryResult, RegistryUser, SearchResultCrate, SearchResults,
    SearchResultsMeta, YesNoMsgResult, YesNoResult,
};
use crate::model::packages::CrateInfoVersion;
use crate::model::stats::{DownloadStats, SERIES_LENGTH};
use crate::model::{CrateVersion, CrateVersionDepsCheckState, JobCrate};
use crate::utils::apierror::{error_forbidden, error_invalid_request, error_not_found, specialize, ApiError};
use crate::utils::comma_sep_to_vec;

impl<'c> Database<'c> {
    /// Search for crates
    pub async fn search_crates(&self, query: &str, per_page: Option<usize>) -> Result<SearchResults, ApiError> {
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

    /// Gets the last version number for a package
    pub async fn get_crate_last_version(&self, package: &str) -> Result<String, ApiError> {
        let row = sqlx::query!(
            "SELECT version, description FROM PackageVersion WHERE package = $1 AND yanked = FALSE ORDER BY id DESC LIMIT 1",
            package
        )
        .fetch_optional(&mut *self.transaction.borrow().await)
        .await?
        .ok_or_else(error_not_found)?;
        Ok(row.version)
    }

    /// Gets all the data about versions of a crate
    pub async fn get_crate_versions(
        &self,
        package: &str,
        versions_in_index: Vec<IndexCrateMetadata>,
    ) -> Result<Vec<CrateInfoVersion>, ApiError> {
        let rows = sqlx::query!(
            "SELECT version, upload, uploadedBy AS uploaded_by,
                    hasDocs AS has_docs, docGenAttempted AS doc_gen_attempted,
                    downloadCount AS download_count,
                    depsLastCheck AS deps_last_check, depsHasOutdated AS deps_has_outdated, depsHasCVEs AS deps_has_cves
            FROM PackageVersion WHERE package = $1 ORDER BY id",
            package
        )
        .fetch_all(&mut *self.transaction.borrow().await)
        .await?;
        let mut result = Vec::new();
        for index_data in versions_in_index {
            if let Some(row) = rows.iter().find(|row| row.version == index_data.vers) {
                let uploaded_by = self.get_user_profile(row.uploaded_by).await?;
                result.push(CrateInfoVersion {
                    index: index_data,
                    upload: row.upload,
                    uploaded_by,
                    has_docs: row.has_docs,
                    doc_gen_attempted: row.doc_gen_attempted,
                    download_count: row.download_count,
                    deps_last_check: row.deps_last_check,
                    deps_has_outdated: row.deps_has_outdated,
                    deps_has_cves: row.deps_has_cves,
                });
            }
        }
        Ok(result)
    }

    /// Publish a crate
    #[allow(clippy::similar_names)]
    pub async fn publish_crate_version(
        &self,
        authenticated_user: &Authentication,
        package: &CrateUploadData,
    ) -> Result<CrateUploadResult, ApiError> {
        if !authenticated_user.can_write {
            return Err(specialize(
                error_forbidden(),
                String::from("writing is forbidden for this authentication"),
            ));
        }
        let uid = authenticated_user.uid()?;
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
                "INSERT INTO Package (name, lowercase, targets) VALUES ($1, $2, '')",
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
        let description = package.metadata.description.as_ref().map_or("", String::as_str);
        sqlx::query!(
            "INSERT INTO PackageVersion (package, version, description, upload, uploadedBy, yanked, hasDocs, docGenAttempted, downloadCount, downloads, depsLastCheck, depsHasOutdated, depsHasCVEs) VALUES ($1, $2, $3, $4, $5, false, false, false, 0, NULL, 0, false, false)",
            package.metadata.name,
            package.metadata.vers,
            description,
            now,
            uid,
        )
        .execute(&mut *self.transaction.borrow().await)
        .await?;
        Ok(warnings)
    }

    /// Checks that a package exists
    pub async fn check_crate_exists(&self, package: &str, version: &str) -> Result<(), ApiError> {
        let _row = sqlx::query!(
            "SELECT id FROM PackageVersion WHERE package = $1 AND version = $2 LIMIT 1",
            package,
            version
        )
        .fetch_optional(&mut *self.transaction.borrow().await)
        .await?
        .ok_or_else(error_not_found)?;
        Ok(())
    }

    /// Checks the ownership of a package
    async fn check_crate_ownership(&self, authenticated_user: &Authentication, package: &str) -> Result<i64, ApiError> {
        let uid = authenticated_user.uid()?;
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
    pub async fn yank_crate_version(
        &self,
        authenticated_user: &Authentication,
        package: &str,
        version: &str,
    ) -> Result<YesNoResult, ApiError> {
        if !authenticated_user.can_write {
            return Err(specialize(
                error_forbidden(),
                String::from("writing is forbidden for this authentication"),
            ));
        }
        self.check_crate_ownership(authenticated_user, package).await?;
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
    pub async fn unyank_crate_version(
        &self,
        authenticated_user: &Authentication,
        package: &str,
        version: &str,
    ) -> Result<YesNoResult, ApiError> {
        if !authenticated_user.can_write {
            return Err(specialize(
                error_forbidden(),
                String::from("writing is forbidden for this authentication"),
            ));
        }
        self.check_crate_ownership(authenticated_user, package).await?;
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
    pub async fn get_undocumented_crates(&self) -> Result<Vec<JobCrate>, ApiError> {
        let rows = sqlx::query!(
            "SELECT package, version, targets
            FROM PackageVersion
            INNER JOIN Package ON PackageVersion.package = Package.name
            WHERE hasDocs = FALSE AND docGenAttempted = FALSE ORDER BY id"
        )
        .fetch_all(&mut *self.transaction.borrow().await)
        .await?;
        Ok(rows
            .into_iter()
            .map(|row| JobCrate {
                name: row.package,
                version: row.version,
                targets: comma_sep_to_vec(&row.targets),
            })
            .collect())
    }

    /// Sets a package as having documentation
    pub async fn set_crate_documentation(&self, package: &str, version: &str, has_docs: bool) -> Result<(), ApiError> {
        sqlx::query!(
            "UPDATE PackageVersion SET docGenAttempted = TRUE, hasDocs = $3 WHERE package = $1 AND version = $2",
            package,
            version,
            has_docs
        )
        .execute(&mut *self.transaction.borrow().await)
        .await?;
        Ok(())
    }

    /// Force the re-generation for the documentation of a package
    pub async fn regen_crate_version_doc(
        &self,
        authenticated_user: &Authentication,
        package: &str,
        version: &str,
    ) -> Result<(), ApiError> {
        if !authenticated_user.can_write {
            return Err(specialize(
                error_forbidden(),
                String::from("writing is forbidden for this authentication"),
            ));
        }
        self.check_crate_ownership(authenticated_user, package).await?;
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
            Some(_row) => {
                sqlx::query!(
                    "UPDATE PackageVersion SET docGenAttempted = FALSE, hasDocs = FALSE WHERE package = $1 AND version = $2",
                    package,
                    version
                )
                .execute(&mut *self.transaction.borrow().await)
                .await?;

                Ok(())
            }
        }
    }

    /// Gets the packages that need to have their dependencies analyzed
    /// Those are the latest version of each crate
    pub async fn get_unanalyzed_crates(&self, deps_stale_analysis: i64) -> Result<Vec<JobCrate>, ApiError> {
        let now = Local::now().naive_local();
        let from = now - Duration::minutes(deps_stale_analysis);
        let heads = self.get_crates_version_heads().await?;
        Ok(heads
            .into_iter()
            .filter_map(|element| {
                if element.deps_last_check < from {
                    Some(element.into())
                } else {
                    None
                }
            })
            .collect())
    }

    /// Gets all the packages that are outdated while also being the latest version
    pub async fn get_crates_outdated_heads(&self) -> Result<Vec<CrateVersion>, ApiError> {
        let heads = self.get_crates_version_heads().await?;
        Ok(heads
            .into_iter()
            .filter_map(|element| {
                if element.deps_has_outdated {
                    Some(element.into())
                } else {
                    None
                }
            })
            .collect())
    }

    /// Gets all the lastest version of crates, filtering out yanked and pre-release versions
    async fn get_crates_version_heads(&self) -> Result<Vec<CrateVersionDepsCheckState>, ApiError> {
        struct Elem {
            semver: Version,
            version: String,
            deps_has_outdated: bool,
            deps_last_check: NaiveDateTime,
            targets: String,
        }
        let mut cache = HashMap::<String, Elem>::new();
        let transaction = &mut *self.transaction.borrow().await;
        let mut stream = sqlx::query!(
            "SELECT package, version, depsHasOutdated AS has_outdated, depsLastCheck AS last_check, targets
            FROM PackageVersion
            INNER JOIN Package ON PackageVersion.package = Package.name
            WHERE yanked = FALSE"
        )
        .fetch(transaction);
        while let Some(row) = stream.next().await {
            let row = row?;
            let name = row.package;
            let semver = row.version.parse::<Version>()?;
            if semver.pre.is_empty() {
                // not a pre-release
                match cache.entry(name.clone()) {
                    Entry::Vacant(entry) => {
                        entry.insert(Elem {
                            semver,
                            version: row.version,
                            deps_has_outdated: row.has_outdated,
                            deps_last_check: row.last_check,
                            targets: row.targets,
                        });
                    }
                    Entry::Occupied(mut entry) => {
                        if semver > entry.get().semver {
                            entry.insert(Elem {
                                semver,
                                version: row.version,
                                deps_has_outdated: row.has_outdated,
                                deps_last_check: row.last_check,
                                targets: row.targets,
                            });
                        }
                    }
                }
            }
        }
        Ok(cache
            .into_iter()
            .map(|(name, elem)| CrateVersionDepsCheckState {
                name,
                version: elem.version,
                deps_has_outdated: elem.deps_has_outdated,
                deps_last_check: elem.deps_last_check,
                targets: elem.targets,
            })
            .collect())
    }

    /// Saves the dependency analysis of a crate
    /// Returns the previous values
    pub async fn set_crate_deps_analysis(
        &self,
        package: &str,
        version: &str,
        has_outdated: bool,
        has_cves: bool,
    ) -> Result<(bool, bool), ApiError> {
        let now = Local::now().naive_local();
        let row = sqlx::query!(
            "SELECT depsHasOutdated AS deps_has_outdated, depsHasCVEs AS deps_has_cves
            FROM PackageVersion
            WHERE package = $1 AND version = $2
            LIMIT 1",
            package,
            version
        )
        .fetch_optional(&mut *self.transaction.borrow().await)
        .await?
        .ok_or_else(error_not_found)?;
        let deps_has_outdated = row.deps_has_outdated;
        let deps_has_cves = row.deps_has_cves;
        sqlx::query!(
            "UPDATE PackageVersion SET depsLastCheck = $3, depsHasOutdated = $4, depsHasCVEs = $5 WHERE package = $1 AND version = $2",
            package,
            version,
            now,
            has_outdated,
            has_cves
        )
        .execute(&mut *self.transaction.borrow().await)
        .await?;
        Ok((deps_has_outdated, deps_has_cves))
    }

    /// Increments the counter of downloads for a crate version
    pub async fn increment_crate_version_dl_count(&self, package: &str, version: &str) -> Result<(), ApiError> {
        let row = sqlx::query!(
            "SELECT downloads FROM PackageVersion WHERE package = $1 AND version = $2 LIMIT 1",
            package,
            version
        )
        .fetch_optional(&mut *self.transaction.borrow().await)
        .await?
        .ok_or_else(error_not_found)?;
        let mut downloads = row.downloads.unwrap_or_else(|| vec![0; size_of::<u32>() * SERIES_LENGTH]);
        let day_index = (Local::now().naive_local().ordinal0() as usize % SERIES_LENGTH) * size_of::<u32>();
        let count = byteorder::NativeEndian::read_u32(&downloads[day_index..]);
        byteorder::NativeEndian::write_u32(&mut downloads[day_index..], count + 1);

        sqlx::query!(
            "UPDATE PackageVersion SET downloadCount = downloadCount + 1, downloads = $3 WHERE package = $1 AND version = $2",
            package,
            version,
            downloads
        )
        .execute(&mut *self.transaction.borrow().await)
        .await?;
        Ok(())
    }

    /// Gets the download statistics for a crate
    pub async fn get_crate_dl_stats(&self, package: &str) -> Result<DownloadStats, ApiError> {
        let rows = sqlx::query!("SELECT version, downloads FROM PackageVersion WHERE package = $1", package)
            .fetch_all(&mut *self.transaction.borrow().await)
            .await?;
        let mut stats = DownloadStats::new();
        for row in rows {
            stats.add_version(row.version, row.downloads.as_deref());
        }
        stats.finalize();
        Ok(stats)
    }

    /// Gets the list of owners for a package
    pub async fn get_crate_owners(&self, package: &str) -> Result<OwnersQueryResult, ApiError> {
        let users = sqlx::query_as!(RegistryUser, "SELECT RegistryUser.id, isActive AS is_active, email, login, name, roles FROM RegistryUser INNER JOIN PackageOwner ON PackageOwner.owner = RegistryUser.id WHERE package = $1", package)
            .fetch_all(&mut *self.transaction.borrow().await).await?;
        Ok(OwnersQueryResult { users })
    }

    /// Add owners to a package
    pub async fn add_crate_owners(
        &self,
        authenticated_user: &Authentication,
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
        self.check_crate_ownership(authenticated_user, package).await?;
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
    pub async fn remove_crate_owners(
        &self,
        authenticated_user: &Authentication,
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
        self.check_crate_ownership(authenticated_user, package).await?;
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

    /// Gets the targets for a crate
    pub async fn get_crate_targets(&self, package: &str) -> Result<Vec<String>, ApiError> {
        let row = sqlx::query!("SELECT targets FROM Package WHERE name = $1 LIMIT 1", package)
            .fetch_optional(&mut *self.transaction.borrow().await)
            .await?
            .ok_or_else(error_not_found)?;
        Ok(comma_sep_to_vec(&row.targets))
    }

    /// Sets the targets for a crate
    pub async fn set_crate_targets(
        &self,
        authenticated_user: &Authentication,
        package: &str,
        targets: &[String],
    ) -> Result<(), ApiError> {
        if !authenticated_user.can_admin {
            return Err(specialize(
                error_forbidden(),
                String::from("administration is forbidden for this authentication"),
            ));
        }
        // check access
        self.check_crate_ownership(authenticated_user, package).await?;
        let targets = targets.join(",");
        sqlx::query!("UPDATE Package SET targets = $2 WHERE name = $1", package, targets)
            .execute(&mut *self.transaction.borrow().await)
            .await?;
        Ok(())
    }
}
