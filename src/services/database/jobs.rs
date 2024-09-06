/*******************************************************************************
 * Copyright (c) 2024 Cénotélie Opérations SAS (cenotelie.fr)
 ******************************************************************************/

//! Service for persisting information in the database
//! API related to jobs

use chrono::Local;

use super::Database;
use crate::model::docs::{DocGenJob, DocGenJobState, DocGenTrigger};
use crate::model::JobCrate;
use crate::utils::apierror::{error_not_found, ApiError};
use crate::utils::comma_sep_to_vec;

impl<'c> Database<'c> {
    /// Gets the documentation generation jobs
    pub async fn get_docgen_jobs(&self) -> Result<Vec<DocGenJob>, ApiError> {
        let rows = sqlx::query!(
            "SELECT id, package, version, targets, state,
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
                targets: comma_sep_to_vec(&row.targets),
                state: DocGenJobState::from(row.state),
                queued_on: row.queued_on,
                started_on: row.started_on,
                finished_on: row.finished_on,
                last_update: row.last_update,
                trigger: DocGenTrigger::from((
                    row.trigger_event,
                    if row.trigger_user < 0 {
                        None
                    } else {
                        Some(self.get_user_profile(row.trigger_user).await?)
                    },
                )),
            });
        }
        Ok(jobs)
    }

    /// Gets a single documentation job
    pub async fn get_docgen_job(&self, job_id: i64) -> Result<DocGenJob, ApiError> {
        let row = sqlx::query!(
            "SELECT id, package, version, targets, state,
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
            targets: comma_sep_to_vec(&row.targets),
            state: DocGenJobState::from(row.state),
            queued_on: row.queued_on,
            started_on: row.started_on,
            finished_on: row.finished_on,
            last_update: row.last_update,
            trigger: DocGenTrigger::from((
                row.trigger_event,
                if row.trigger_user < 0 {
                    None
                } else {
                    Some(self.get_user_profile(row.trigger_user).await?)
                },
            )),
        })
    }

    /// Creates and queue a documentation generation job
    pub async fn create_docgen_job(&self, spec: &JobCrate, trigger: &DocGenTrigger) -> Result<DocGenJob, ApiError> {
        // look for already existing queued job
        let state_value = DocGenJobState::Queued.value();
        let row = sqlx::query!(
            "SELECT id, package, version, targets, state,
            queuedOn AS queued_on, startedOn AS started_on, finishedOn AS finished_on, lastUpdate AS last_update,
            triggerUser AS trigger_user, triggerEvent AS trigger_event
            FROM DocGenJob
            WHERE state = $1 AND package = $2 AND version = $3
            ORDER BY id DESC
            LIMIT 1",
            state_value,
            spec.name,
            spec.version
        )
        .fetch_optional(&mut *self.transaction.borrow().await)
        .await?;
        if let Some(row) = row {
            // there is already a queued job, return this one
            return Ok(DocGenJob {
                id: row.id,
                package: row.package,
                version: row.version,
                targets: comma_sep_to_vec(&row.targets),
                state: DocGenJobState::from(row.state),
                queued_on: row.queued_on,
                started_on: row.started_on,
                finished_on: row.finished_on,
                last_update: row.last_update,
                trigger: DocGenTrigger::from((
                    row.trigger_event,
                    if row.trigger_user < 0 {
                        None
                    } else {
                        Some(self.get_user_profile(row.trigger_user).await?)
                    },
                )),
            });
        }

        let trigger_event = trigger.value();
        let trigger_user = trigger.by().map_or(-1, |u| u.id);
        let now = Local::now().naive_local();
        let targets = spec.targets.join(",");
        let state_value = DocGenJobState::Queued.value();
        let job_id = sqlx::query!(
            "INSERT INTO DocGenJob (
            package, version, targets, state,
            queuedOn, startedOn, finishedOn, lastUpdate,
            triggerUser, triggerEvent, output
        ) VALUES (
            $1, $2, $3, $4,
            $5, $5, $5, $5,
            $6, $7, ''
        ) RETURNING id",
            spec.name,
            spec.version,
            targets,
            state_value,
            now,
            trigger_user,
            trigger_event,
        )
        .fetch_one(&mut *self.transaction.borrow().await)
        .await?
        .id;
        Ok(DocGenJob {
            id: job_id,
            package: spec.name.clone(),
            version: spec.version.clone(),
            targets: spec.targets.clone(),
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
            "SELECT id, package, version, targets, state,
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
            targets: comma_sep_to_vec(&row.targets),
            state: DocGenJobState::from(row.state),
            queued_on: row.queued_on,
            started_on: row.started_on,
            finished_on: row.finished_on,
            last_update: row.last_update,
            trigger: DocGenTrigger::from((
                row.trigger_event,
                if row.trigger_user < 0 {
                    None
                } else {
                    Some(self.get_user_profile(row.trigger_user).await?)
                },
            )),
        }))
    }

    /// Updates an existing job
    pub async fn update_docgen_job(&self, job_id: i64, state: DocGenJobState) -> Result<(), ApiError> {
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
