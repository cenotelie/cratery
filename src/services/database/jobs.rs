/*******************************************************************************
 * Copyright (c) 2024 Cénotélie Opérations SAS (cenotelie.fr)
 ******************************************************************************/

//! Service for persisting information in the database
//! API related to jobs

use chrono::Local;
use thiserror::Error;

use super::Database;
use super::users::UserError;
use crate::model::docs::{DocGenJob, DocGenJobSpec, DocGenJobState, DocGenTrigger};
use crate::utils::apierror::{ApiError, AsStatusCode, error_not_found};
use crate::utils::comma_sep_to_vec;

#[derive(Debug, Error)]
pub enum DocGenError {
    #[error("request docgen job for {spec_package}-{spec_version}-{spec_target} with state {state}")]
    SqlxSelectJob {
        #[source]
        source: sqlx::Error,
        state: i64,
        spec_package: String,
        spec_version: String,
        spec_target: String,
    },

    #[error("failed to get user profile associated to a DocGenJob")]
    UserProfile(#[source] UserError),

    #[error("failed to Insert a DocGenJob for {spec_package}-{spec_version}-{spec_target}.")]
    SqlxInsertJob {
        source: sqlx::Error,
        spec_package: String,
        spec_version: String,
        spec_target: String,
    },
}

impl AsStatusCode for DocGenError {}

impl Database {
    /// Gets the documentation generation jobs
    pub async fn get_docgen_jobs(&self) -> Result<Vec<DocGenJob>, ApiError> {
        let rows = sqlx::query!(
            "SELECT id, package, version, target, useNative AS usenative, capabilities, state,
            queuedOn AS queued_on, startedOn AS started_on, finishedOn AS finished_on, lastUpdate AS last_update,
            triggerUser AS trigger_user, triggerEvent AS trigger_event
            FROM DocGenJob
            ORDER BY id DESC"
        )
        .fetch_all(&mut *self.transaction.borrow().await)
        .await?;
        let mut jobs = Vec::with_capacity(rows.len());
        for row in rows {
            jobs.push(DocGenJob {
                id: row.id,
                package: row.package,
                version: row.version,
                target: row.target,
                use_native: row.usenative,
                capabilities: comma_sep_to_vec(&row.capabilities),
                state: DocGenJobState::from(row.state),
                queued_on: row.queued_on,
                started_on: row.started_on,
                finished_on: row.finished_on,
                last_update: row.last_update,
                trigger: DocGenTrigger::from((
                    row.trigger_event,
                    if let Some(uid) = row.trigger_user {
                        Some(self.get_user_profile(uid).await?)
                    } else {
                        None
                    },
                )),
            });
        }
        Ok(jobs)
    }

    /// Gets a single documentation job
    pub async fn get_docgen_job(&self, job_id: i64) -> Result<DocGenJob, ApiError> {
        let row = sqlx::query!(
            "SELECT id, package, version, target, useNative AS usenative, capabilities, state,
            queuedOn AS queued_on, startedOn AS started_on, finishedOn AS finished_on, lastUpdate AS last_update,
            triggerUser AS trigger_user, triggerEvent AS trigger_event
            FROM DocGenJob
            WHERE id = $1
            LIMIT 1",
            job_id
        )
        .fetch_optional(&mut *self.transaction.borrow().await)
        .await?
        .ok_or_else(error_not_found)?;
        Ok(DocGenJob {
            id: row.id,
            package: row.package,
            version: row.version,
            target: row.target,
            use_native: row.usenative,
            capabilities: comma_sep_to_vec(&row.capabilities),
            state: DocGenJobState::from(row.state),
            queued_on: row.queued_on,
            started_on: row.started_on,
            finished_on: row.finished_on,
            last_update: row.last_update,
            trigger: DocGenTrigger::from((
                row.trigger_event,
                if let Some(uid) = row.trigger_user {
                    Some(self.get_user_profile(uid).await?)
                } else {
                    None
                },
            )),
        })
    }

    /// Creates and queue a single documentation job
    pub async fn create_docgen_job(&self, spec: &DocGenJobSpec, trigger: &DocGenTrigger) -> Result<DocGenJob, DocGenError> {
        // look for already existing queued job
        let state_value = DocGenJobState::Queued.value();
        let row = sqlx::query!(
            "SELECT id, package, version, target, useNative AS usenative, capabilities, state,
            queuedOn AS queued_on, startedOn AS started_on, finishedOn AS finished_on, lastUpdate AS last_update,
            triggerUser AS trigger_user, triggerEvent AS trigger_event
            FROM DocGenJob
            WHERE state = $1 AND package = $2 AND version = $3 AND target = $4
            ORDER BY id DESC
            LIMIT 1",
            state_value,
            spec.package,
            spec.version,
            spec.target,
        )
        .fetch_optional(&mut *self.transaction.borrow().await)
        .await
        .map_err(|source| DocGenError::SqlxSelectJob {
            source,
            state: state_value,
            spec_package: spec.package.clone(),
            spec_version: spec.version.clone(),
            spec_target: spec.target.clone(),
        })?;
        if let Some(row) = row {
            // there is already a queued job, return this one
            return Ok(DocGenJob {
                id: row.id,
                package: row.package,
                version: row.version,
                target: row.target,
                use_native: row.usenative,
                capabilities: comma_sep_to_vec(&row.capabilities),
                state: DocGenJobState::from(row.state),
                queued_on: row.queued_on,
                started_on: row.started_on,
                finished_on: row.finished_on,
                last_update: row.last_update,
                trigger: DocGenTrigger::from((
                    row.trigger_event,
                    if let Some(uid) = row.trigger_user {
                        Some(self.get_user_profile(uid).await.map_err(DocGenError::UserProfile)?)
                    } else {
                        None
                    },
                )),
            });
        }

        let capabilities = spec.capabilities.join(",");
        let trigger_event = trigger.value();
        let trigger_user = trigger.by().map(|u| u.id);
        let now = Local::now().naive_local();
        let state_value = DocGenJobState::Queued.value();
        let job_id = sqlx::query!(
            "INSERT INTO DocGenJob (
            package, version, target, useNative, capabilities, state,
            queuedOn, startedOn, finishedOn, lastUpdate,
            triggerUser, triggerEvent, output
        ) VALUES (
            $1, $2, $3, $4, $5, $6,
            $7, $7, $7, $7,
            $8, $9, ''
        ) RETURNING id",
            spec.package,
            spec.version,
            spec.target,
            spec.use_native,
            capabilities,
            state_value,
            now,
            trigger_user,
            trigger_event,
        )
        .fetch_one(&mut *self.transaction.borrow().await)
        .await
        .map_err(|source| DocGenError::SqlxInsertJob {
            source,
            spec_package: spec.package.clone(),
            spec_version: spec.version.clone(),
            spec_target: spec.target.clone(),
        })?
        .id;
        Ok(DocGenJob {
            id: job_id,
            package: spec.package.clone(),
            version: spec.version.clone(),
            target: spec.target.clone(),
            use_native: spec.use_native,
            capabilities: spec.capabilities.clone(),
            state: DocGenJobState::Queued,
            queued_on: now,
            started_on: now,
            finished_on: now,
            last_update: now,
            trigger: trigger.clone(),
        })
    }

    /// Attempts to get the next available job
    pub async fn get_next_docgen_job(&self) -> Result<Option<DocGenJob>, ApiError> {
        let state_value = DocGenJobState::Queued.value();
        let row = sqlx::query!(
            "SELECT id, package, version, target, useNative AS usenative, capabilities, state,
            queuedOn AS queued_on, startedOn AS started_on, finishedOn AS finished_on, lastUpdate AS last_update,
            triggerUser AS trigger_user, triggerEvent AS trigger_event
            FROM DocGenJob
            WHERE state = $1
            ORDER BY id
            LIMIT 1",
            state_value
        )
        .fetch_optional(&mut *self.transaction.borrow().await)
        .await?;
        let Some(row) = row else { return Ok(None) };
        Ok(Some(DocGenJob {
            id: row.id,
            package: row.package,
            version: row.version,
            target: row.target,
            use_native: row.usenative,
            capabilities: comma_sep_to_vec(&row.capabilities),
            state: DocGenJobState::from(row.state),
            queued_on: row.queued_on,
            started_on: row.started_on,
            finished_on: row.finished_on,
            last_update: row.last_update,
            trigger: DocGenTrigger::from((
                row.trigger_event,
                if let Some(uid) = row.trigger_user {
                    Some(self.get_user_profile(uid).await?)
                } else {
                    None
                },
            )),
        }))
    }

    /// Updates an existing job
    pub async fn update_docgen_job(&self, job_id: i64, state: DocGenJobState) -> Result<(), sqlx::Error> {
        let now = Local::now().naive_local();
        let state_value = state.value();
        if state == DocGenJobState::Working {
            sqlx::query!(
                "UPDATE DocGenJob SET state = $2, startedOn = $3, lastUpdate = $3 WHERE id = $1",
                job_id,
                state_value,
                now
            )
            .execute(&mut *self.transaction.borrow().await)
            .await?;
        } else if state.is_final() {
            // final state
            sqlx::query!(
                "UPDATE DocGenJob SET state = $2, finishedOn = $3, lastUpdate = $3 WHERE id = $1",
                job_id,
                state_value,
                now
            )
            .execute(&mut *self.transaction.borrow().await)
            .await?;
        } else {
            sqlx::query!(
                "UPDATE DocGenJob SET state = $2, lastUpdate = $3 WHERE id = $1",
                job_id,
                state_value,
                now
            )
            .execute(&mut *self.transaction.borrow().await)
            .await?;
        }
        Ok(())
    }
}
