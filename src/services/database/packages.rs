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
use crate::model::cargo::{
    CrateUploadData, CrateUploadResult, IndexCrateMetadata, OwnersQueryResult, RegistryUser, SearchResultCrate, SearchResults,
    SearchResultsMeta, YesNoMsgResult, YesNoResult,
};
use crate::model::deps::{DepsAnalysisJobSpec, DepsAnalysisState};
use crate::model::docs::DocGenJobSpec;
use crate::model::packages::{CrateInfo, CrateInfoTarget, CrateInfoVersion, CrateInfoVersionDocs};
use crate::model::stats::{DownloadStats, SERIES_LENGTH};
use crate::model::CrateVersion;
use crate::utils::apierror::{error_invalid_request, error_not_found, specialize, ApiError};
use crate::utils::comma_sep_to_vec;

impl Database {
    /// Search for crates
    pub async fn search_crates(
        &self,
        query: &str,
        per_page: Option<usize>,
        deprecated: Option<bool>,
    ) -> Result<SearchResults, ApiError> {
        let per_page = match per_page {
            None => 10,
            Some(value) if value > 100 => 100,
            Some(value) => value,
        };
        let pattern = format!("%{query}%");
        let deprecated_value = deprecated.unwrap_or_default();
        let deprecated_short_circuit = deprecated.is_none(); // short-cirtcuit to true if no input
        let rows = sqlx::query!(
            "SELECT name, isDeprecated AS is_deprecated From Package WHERE name LIKE $1 AND (isDeprecated = $2 OR $3)",
            pattern,
            deprecated_value,
            deprecated_short_circuit
        )
        .fetch_all(&mut *self.transaction.borrow().await)
        .await?;
        let mut crates = Vec::new();
        for row_name in rows {
            let row = sqlx::query!("SELECT version, description FROM PackageVersion WHERE package = $1 AND yanked = FALSE ORDER BY id DESC LIMIT 1", row_name.name).fetch_optional(&mut *self.transaction.borrow().await).await?;
            if let Some(row) = row {
                crates.push(SearchResultCrate {
                    name: row_name.name,
                    max_version: row.version,
                    is_deprecated: row_name.is_deprecated,
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

    /// Gets whether the database does not contain any package at all
    pub async fn get_is_empty(&self) -> Result<bool, ApiError> {
        Ok(sqlx::query!("SELECT id FROM PackageVersion LIMIT 1")
            .fetch_optional(&mut *self.transaction.borrow().await)
            .await?
            .is_none())
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

    /// Gets all the data about a crate
    pub async fn get_crate_info(
        &self,
        package: &str,
        versions_in_index: Vec<IndexCrateMetadata>,
    ) -> Result<CrateInfo, ApiError> {
        let row = sqlx::query!(
            "SELECT isDeprecated AS is_deprecated, canRemove AS can_remove, targets, nativeTargets AS nativetargets, capabilities FROM Package WHERE name = $1 LIMIT 1",
            package
        )
        .fetch_optional(&mut *self.transaction.borrow().await)
        .await?
        .ok_or_else(error_not_found)?;
        let is_deprecated = row.is_deprecated;
        let can_remove = row.can_remove;
        let targets = comma_sep_to_vec(&row.targets);
        let native_targets = comma_sep_to_vec(&row.nativetargets);
        let capabilities = comma_sep_to_vec(&row.capabilities);

        let rows = sqlx::query!(
            "SELECT version, upload, uploadedBy AS uploaded_by,
                    downloadCount AS download_count,
                    depsLastCheck AS deps_last_check, depsHasOutdated AS deps_has_outdated, depsHasCVEs AS deps_has_cves
            FROM PackageVersion WHERE package = $1 ORDER BY id",
            package
        )
        .fetch_all(&mut *self.transaction.borrow().await)
        .await?;
        let mut versions = Vec::new();
        for index_data in versions_in_index {
            if let Some(row) = rows.iter().find(|row| row.version == index_data.vers) {
                let uploaded_by = self.get_user_profile(row.uploaded_by).await?;
                versions.push(CrateInfoVersion {
                    index: index_data,
                    upload: row.upload,
                    uploaded_by,
                    download_count: row.download_count,
                    deps_last_check: row.deps_last_check,
                    deps_has_outdated: row.deps_has_outdated,
                    deps_has_cves: row.deps_has_cves,
                    docs: Vec::new(),
                });
            }
        }
        let rows = sqlx::query!(
            "SELECT version, target, isAttempted AS is_attempted, isPresent AS is_present
            FROM PackageVersionDocs
            WHERE package = $1 ORDER BY id",
            package
        )
        .fetch_all(&mut *self.transaction.borrow().await)
        .await?;
        for row in rows {
            if let Some(version) = versions.iter_mut().find(|v| v.index.vers == row.version) {
                version.docs.push(CrateInfoVersionDocs {
                    target: row.target,
                    is_attempted: row.is_attempted,
                    is_present: row.is_present,
                });
            }
        }
        Ok(CrateInfo {
            metadata: None,
            is_deprecated,
            can_remove,
            versions,
            targets: targets
                .into_iter()
                .map(|target| CrateInfoTarget {
                    docs_use_native: native_targets.contains(&target),
                    target,
                })
                .collect(),
            capabilities,
        })
    }

    /// Publish a crate
    #[allow(clippy::similar_names)]
    pub async fn publish_crate_version(&self, uid: i64, package: &CrateUploadData) -> Result<CrateUploadResult, ApiError> {
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
            self.check_is_crate_manager(uid, &package.metadata.name).await?;
        } else {
            // create the package
            sqlx::query!(
                "INSERT INTO Package (name, lowercase, targets, nativeTargets, capabilities, isDeprecated, canRemove) VALUES ($1, $2, '', '', '', FALSE, FALSE)",
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
            "INSERT INTO PackageVersion (package, version, description, upload, uploadedBy, yanked, downloadCount, downloads, depsLastCheck, depsHasOutdated, depsHasCVEs) VALUES ($1, $2, $3, $4, $5, false, 0, NULL, 0, false, false)",
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

    /// Completely removes a version from the registry
    pub async fn remove_crate_version(&self, package: &str, version: &str) -> Result<(), ApiError> {
        // check whether this is allowed
        let can_remove = sqlx::query!(
            "SELECT canRemove AS can_remove FROM Package WHERE lowercase = $1 LIMIT 1",
            package
        )
        .fetch_optional(&mut *self.transaction.borrow().await)
        .await?
        .is_some_and(|r| r.can_remove);
        if !can_remove {
            return Err(specialize(
                error_invalid_request(),
                format!("Package {package} does not allow removing versions",),
            ));
        }
        // check version exists
        let row = sqlx::query!(
            "SELECT id FROM PackageVersion WHERE package = $1 AND version = $2 LIMIT 1",
            package,
            version
        )
        .fetch_optional(&mut *self.transaction.borrow().await)
        .await?;
        if row.is_none() {
            return Err(specialize(
                error_not_found(),
                format!("Package {package}, version {version} not found",),
            ));
        }
        sqlx::query!(
            "DELETE FROM PackageVersion WHERE package = $1 AND version = $2",
            package,
            version
        )
        .execute(&mut *self.transaction.borrow().await)
        .await?;
        sqlx::query!(
            "DELETE FROM PackageVersionDocs WHERE package = $1 AND version = $2",
            package,
            version
        )
        .execute(&mut *self.transaction.borrow().await)
        .await?;

        Ok(())
    }

    /// Yank a crate version
    pub async fn yank_crate_version(&self, package: &str, version: &str) -> Result<YesNoResult, ApiError> {
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
    pub async fn unyank_crate_version(&self, package: &str, version: &str) -> Result<YesNoResult, ApiError> {
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
    pub async fn get_undocumented_crates(&self, default_target: &str) -> Result<Vec<DocGenJobSpec>, ApiError> {
        struct PackageData {
            targets: Vec<CrateInfoTarget>,
            capabilities: Vec<String>,
            versions: Vec<VersionData>,
        }
        struct VersionData {
            version: String,
            docs: u64,
        }
        let mut packages: HashMap<String, PackageData> = HashMap::new();

        let mut transaction = self.transaction.borrow().await;
        // retrieve all package versions and associated targets
        {
            let mut stream = sqlx::query!(
                "SELECT package, version, targets, nativeTargets AS nativetargets, capabilities
                FROM PackageVersion INNER JOIN Package ON PackageVersion.package = Package.name"
            )
            .fetch(&mut *transaction);
            while let Some(Ok(row)) = stream.next().await {
                let data = packages.entry(row.package).or_insert_with(|| {
                    let native_targets = comma_sep_to_vec(&row.nativetargets);
                    PackageData {
                        targets: if row.targets.is_empty() {
                            vec![CrateInfoTarget {
                                target: default_target.to_string(),
                                docs_use_native: true,
                            }]
                        } else {
                            comma_sep_to_vec(&row.targets)
                                .into_iter()
                                .map(|target| CrateInfoTarget {
                                    docs_use_native: native_targets.contains(&target),
                                    target,
                                })
                                .collect()
                        },
                        capabilities: comma_sep_to_vec(&row.capabilities),
                        versions: Vec::new(),
                    }
                });
                data.versions.push(VersionData {
                    version: row.version,
                    docs: 0,
                });
            }
        }
        // find all present or attempted docs (not missing)
        {
            let mut stream = sqlx::query!(
                "SELECT package, version, target
                FROM PackageVersionDocs
                WHERE isPresent = TRUE OR isAttempted = TRUE"
            )
            .fetch(&mut *transaction);
            while let Some(Ok(row)) = stream.next().await {
                if let Some(data) = packages.get_mut(&row.package) {
                    let target_index = data.targets.iter().position(|info| info.target == row.target);
                    let version_data = data.versions.iter_mut().find(|d| d.version == row.version);
                    if let (Some(target_index), Some(version_data)) = (target_index, version_data) {
                        version_data.docs |= 1 << target_index;
                    }
                }
            }
        }
        // aggregate results
        let mut jobs = Vec::new();
        for (package, data) in packages {
            for version in data.versions {
                for (index, info) in data.targets.iter().enumerate() {
                    let is_missing = (version.docs & (1 << index)) == 0;
                    if is_missing {
                        jobs.push(DocGenJobSpec {
                            package: package.clone(),
                            version: version.version.clone(),
                            target: info.target.clone(),
                            use_native: info.docs_use_native,
                            capabilities: data.capabilities.clone(),
                        });
                    }
                }
            }
        }
        Ok(jobs)
    }

    /// Sets a package as having documentation
    pub async fn set_crate_documentation(
        &self,
        package: &str,
        version: &str,
        target: &str,
        is_attempted: bool,
        is_present: bool,
    ) -> Result<(), ApiError> {
        let is_missing = sqlx::query!(
            "SELECT id FROM PackageVersionDocs WHERE package = $1 AND version = $2 AND target = $3 LIMIT 1",
            package,
            version,
            target
        )
        .fetch_optional(&mut *self.transaction.borrow().await)
        .await?
        .is_none();
        if is_missing {
            sqlx::query!(
                "INSERT INTO PackageVersionDocs (package, version, target, isAttempted, isPresent) VALUES ($1, $2, $3, $4, $5)",
                package,
                version,
                target,
                is_attempted,
                is_present
            )
            .execute(&mut *self.transaction.borrow().await)
            .await?;
        } else {
            sqlx::query!("UPDATE PackageVersionDocs SET isAttempted = $4, isPresent = $5 WHERE package = $1 AND version = $2 AND target = $3",
            package, version, target, is_attempted, is_present
        ).execute(&mut *self.transaction.borrow().await).await?;
        }
        Ok(())
    }

    /// Force the re-generation for the documentation of a package
    pub async fn regen_crate_version_doc(
        &self,
        package: &str,
        version: &str,
        default_target: &str,
    ) -> Result<Vec<CrateInfoTarget>, ApiError> {
        self.check_crate_exists(package, version).await?;

        let row = sqlx::query!(
            "SELECT targets, nativeTargets AS nativetargets FROM Package WHERE name = $1 LIMIT 1",
            package
        )
        .fetch_optional(&mut *self.transaction.borrow().await)
        .await?
        .ok_or_else(error_not_found)?;
        let targets = comma_sep_to_vec(&row.targets);
        let native_targets = comma_sep_to_vec(&row.nativetargets);
        let targets = targets
            .into_iter()
            .map(|target| CrateInfoTarget {
                docs_use_native: native_targets.contains(&target),
                target,
            })
            .collect::<Vec<_>>();
        let targets = if targets.is_empty() {
            vec![CrateInfoTarget {
                target: default_target.to_string(),
                docs_use_native: true,
            }]
        } else {
            targets
        };

        for info in &targets {
            self.set_crate_documentation(package, version, &info.target, false, false)
                .await?;
        }
        Ok(targets)
    }

    /// Gets the packages that need to have their dependencies analyzed
    /// Those are the latest version of each crate
    pub async fn get_unanalyzed_crates(&self, deps_stale_analysis: i64) -> Result<Vec<DepsAnalysisJobSpec>, ApiError> {
        let now = Local::now().naive_local();
        let from = now - Duration::minutes(deps_stale_analysis);
        let heads = self.get_crates_version_heads().await?;
        Ok(heads
            .into_iter()
            .filter_map(|element| {
                if !element.is_deprecated && element.deps_last_check < from {
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
                if !element.is_deprecated && element.deps_has_outdated {
                    Some(element.into())
                } else {
                    None
                }
            })
            .collect())
    }

    /// Gets all the lastest version of crates, filtering out yanked and pre-release versions
    async fn get_crates_version_heads(&self) -> Result<Vec<DepsAnalysisState>, ApiError> {
        struct Elem {
            semver: Version,
            version: String,
            is_deprecated: bool,
            deps_has_outdated: bool,
            deps_last_check: NaiveDateTime,
            targets: String,
        }
        let mut cache = HashMap::<String, Elem>::new();
        let transaction = &mut *self.transaction.borrow().await;
        let mut stream = sqlx::query!(
            "SELECT package, version, isDeprecated AS is_deprecated, depsHasOutdated AS has_outdated, depsLastCheck AS last_check, targets
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
                            is_deprecated: row.is_deprecated,
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
                                is_deprecated: row.is_deprecated,
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
            .map(|(package, elem)| DepsAnalysisState {
                package,
                version: elem.version,
                is_deprecated: elem.is_deprecated,
                deps_has_outdated: elem.deps_has_outdated,
                deps_last_check: elem.deps_last_check,
                targets: comma_sep_to_vec(&elem.targets),
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
        let mut stats = DownloadStats::default();
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
    pub async fn add_crate_owners(&self, package: &str, new_users: &[String]) -> Result<YesNoMsgResult, ApiError> {
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
    pub async fn remove_crate_owners(&self, package: &str, old_users: &[String]) -> Result<YesNoResult, ApiError> {
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
    pub async fn get_crate_targets(&self, package: &str) -> Result<Vec<CrateInfoTarget>, ApiError> {
        let row = sqlx::query!(
            "SELECT targets, nativeTargets AS nativetargets FROM Package WHERE name = $1 LIMIT 1",
            package
        )
        .fetch_optional(&mut *self.transaction.borrow().await)
        .await?
        .ok_or_else(error_not_found)?;
        let targets = comma_sep_to_vec(&row.targets);
        let native_targets = comma_sep_to_vec(&row.nativetargets);
        Ok(targets
            .into_iter()
            .map(|target| CrateInfoTarget {
                docs_use_native: native_targets.contains(&target),
                target,
            })
            .collect())
    }

    /// Sets the targets for a crate
    pub async fn set_crate_targets(&self, package: &str, targets: &[CrateInfoTarget]) -> Result<Vec<DocGenJobSpec>, ApiError> {
        let old_targets = self.get_crate_targets(package).await?;
        let added_targets = targets
            .iter()
            .filter_map(|info| {
                if old_targets.iter().any(|t| t.target == info.target) {
                    None
                } else {
                    Some(info.clone())
                }
            })
            .collect::<Vec<_>>();

        let new_targets = targets.iter().map(|info| info.target.as_str()).collect::<Vec<_>>();
        let new_targets = new_targets.join(",");
        let native_targets = targets
            .iter()
            .filter_map(|info| {
                if info.docs_use_native {
                    Some(info.target.as_str())
                } else {
                    None
                }
            })
            .collect::<Vec<_>>();
        let native_targets = native_targets.join(",");
        sqlx::query!(
            "UPDATE Package SET targets = $2, nativeTargets = $3 WHERE name = $1",
            package,
            new_targets,
            native_targets
        )
        .execute(&mut *self.transaction.borrow().await)
        .await?;

        // get versions
        let capabilities = comma_sep_to_vec(
            &sqlx::query!("SELECT capabilities FROM Package WHERE name = $1 LIMIT 1", package)
                .fetch_one(&mut *self.transaction.borrow().await)
                .await?
                .capabilities,
        );
        let rows = sqlx::query!("SELECT version FROM PackageVersion WHERE package = $1", package)
            .fetch_all(&mut *self.transaction.borrow().await)
            .await?;
        let mut jobs = Vec::new();
        for row in rows {
            for info in &added_targets {
                jobs.push(DocGenJobSpec {
                    package: package.to_string(),
                    version: row.version.clone(),
                    target: info.target.clone(),
                    use_native: info.docs_use_native,
                    capabilities: capabilities.clone(),
                });
            }
        }
        Ok(jobs)
    }

    /// Gets the required capabilities for a crate
    pub async fn get_crate_required_capabilities(&self, package: &str) -> Result<Vec<String>, ApiError> {
        let row = sqlx::query!("SELECT capabilities FROM Package WHERE name = $1 LIMIT 1", package)
            .fetch_optional(&mut *self.transaction.borrow().await)
            .await?
            .ok_or_else(error_not_found)?;
        Ok(comma_sep_to_vec(&row.capabilities))
    }

    /// Sets the required capabilities for a crate
    pub async fn set_crate_required_capabilities(&self, package: &str, capabilities: &[String]) -> Result<(), ApiError> {
        let _ = self.get_crate_required_capabilities(package).await?;
        let capabilities = capabilities.join(",");
        sqlx::query!("UPDATE Package SET capabilities = $2 WHERE name = $1", package, capabilities)
            .execute(&mut *self.transaction.borrow().await)
            .await?;
        Ok(())
    }

    /// Sets the deprecation status on a crate
    pub async fn set_crate_deprecation(&self, package: &str, deprecated: bool) -> Result<(), ApiError> {
        sqlx::query!("UPDATE Package SET isDeprecated = $2 WHERE name = $1", package, deprecated)
            .execute(&mut *self.transaction.borrow().await)
            .await?;
        Ok(())
    }

    /// Sets whether a crate can have versions completely removed
    pub async fn set_crate_can_can_remove(&self, package: &str, can_remove: bool) -> Result<(), ApiError> {
        sqlx::query!("UPDATE Package SET canRemove = $2 WHERE name = $1", package, can_remove)
            .execute(&mut *self.transaction.borrow().await)
            .await?;
        Ok(())
    }
}
