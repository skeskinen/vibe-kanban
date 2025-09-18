use std::{collections::HashMap, sync::Arc};

use anyhow::Error as AnyhowError;
use async_trait::async_trait;
use axum::response::sse::Event;
use db::{
    DBService,
    models::{
        execution_process::{ExecutionProcess, ExecutionProcessRunReason, ExecutionProcessStatus},
        task::{Task, TaskStatus},
        task_attempt::{TaskAttempt, TaskAttemptError},
    },
};
use executors::executors::ExecutorError;
use futures::{StreamExt, TryStreamExt};
use git2::Error as Git2Error;
use services::services::{
    auth::{AuthError, AuthService},
    config::{Config, ConfigError},
    container::{ContainerError, ContainerService},
    events::{EventError, EventService},
    file_search_cache::FileSearchCache,
    filesystem::{FilesystemError, FilesystemService},
    filesystem_watcher::FilesystemWatcherError,
    git::{GitService, GitServiceError},
    image::{ImageError, ImageService},
    pr_monitor::PrMonitorService,
    worktree_manager::WorktreeError,
};
use sqlx::{Error as SqlxError, types::Uuid};
use thiserror::Error;
use tokio::sync::RwLock;
use utils::msg_store::MsgStore;

#[derive(Debug, Error)]
pub enum DeploymentError {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Sqlx(#[from] SqlxError),
    #[error(transparent)]
    Git2(#[from] Git2Error),
    #[error(transparent)]
    GitServiceError(#[from] GitServiceError),
    #[error(transparent)]
    FilesystemWatcherError(#[from] FilesystemWatcherError),
    #[error(transparent)]
    TaskAttempt(#[from] TaskAttemptError),
    #[error(transparent)]
    Container(#[from] ContainerError),
    #[error(transparent)]
    Executor(#[from] ExecutorError),
    #[error(transparent)]
    Auth(#[from] AuthError),
    #[error(transparent)]
    Image(#[from] ImageError),
    #[error(transparent)]
    Filesystem(#[from] FilesystemError),
    #[error(transparent)]
    Worktree(#[from] WorktreeError),
    #[error(transparent)]
    Event(#[from] EventError),
    #[error(transparent)]
    Config(#[from] ConfigError),
    #[error(transparent)]
    Other(#[from] AnyhowError),
}

#[async_trait]
pub trait Deployment: Clone + Send + Sync + 'static {
    async fn new() -> Result<Self, DeploymentError>;

    fn shared_types() -> Vec<String>;

    fn config(&self) -> &Arc<RwLock<Config>>;

    fn db(&self) -> &DBService;

    fn container(&self) -> &impl ContainerService;

    fn auth(&self) -> &AuthService;

    fn git(&self) -> &GitService;

    fn image(&self) -> &ImageService;

    fn filesystem(&self) -> &FilesystemService;

    fn msg_stores(&self) -> &Arc<RwLock<HashMap<Uuid, Arc<MsgStore>>>>;

    fn events(&self) -> &EventService;

    fn file_search_cache(&self) -> &Arc<FileSearchCache>;

    async fn spawn_pr_monitor_service(&self) -> tokio::task::JoinHandle<()> {
        let db = self.db().clone();
        let config = self.config().clone();
        PrMonitorService::spawn(db, config).await
    }

    /// Cleanup executions marked as running in the db, call at startup
    async fn cleanup_orphan_executions(&self) -> Result<(), DeploymentError> {
        let running_processes = ExecutionProcess::find_running(&self.db().pool).await?;
        for process in running_processes {
            tracing::info!(
                "Found orphaned execution process {} for task attempt {}",
                process.id,
                process.task_attempt_id
            );
            // Update the execution process status first
            if let Err(e) = ExecutionProcess::update_completion(
                &self.db().pool,
                process.id,
                ExecutionProcessStatus::Failed,
                None, // No exit code for orphaned processes
            )
            .await
            {
                tracing::error!(
                    "Failed to update orphaned execution process {} status: {}",
                    process.id,
                    e
                );
                continue;
            }
            // Capture after-head commit OID (best-effort)
            if let Ok(Some(task_attempt)) =
                TaskAttempt::find_by_id(&self.db().pool, process.task_attempt_id).await
                && let Some(container_ref) = task_attempt.container_ref
            {
                let wt = std::path::PathBuf::from(container_ref);
                if let Ok(head) = self.git().get_head_info(&wt) {
                    let _ = ExecutionProcess::update_after_head_commit(
                        &self.db().pool,
                        process.id,
                        &head.oid,
                    )
                    .await;
                }
            }
            // Process marked as failed
            tracing::info!("Marked orphaned execution process {} as failed", process.id);
            // Update task status to InReview for coding agent and setup script failures
            if matches!(
                process.run_reason,
                ExecutionProcessRunReason::CodingAgent
                    | ExecutionProcessRunReason::SetupScript
                    | ExecutionProcessRunReason::CleanupScript
            ) && let Ok(Some(task_attempt)) =
                TaskAttempt::find_by_id(&self.db().pool, process.task_attempt_id).await
                && let Ok(Some(task)) = task_attempt.parent_task(&self.db().pool).await
                && let Err(e) =
                    Task::update_status(&self.db().pool, task.id, TaskStatus::InReview).await
            {
                tracing::error!(
                    "Failed to update task status to InReview for orphaned attempt: {}",
                    e
                );
            }
        }
        Ok(())
    }

    /// Backfill before_head_commit for legacy execution processes.
    /// Rules:
    /// - If a process has after_head_commit and missing before_head_commit,
    ///   then set before_head_commit to the previous process's after_head_commit.
    /// - If there is no previous process, set before_head_commit to the base branch commit.
    async fn backfill_before_head_commits(&self) -> Result<(), DeploymentError> {
        let pool = &self.db().pool;
        let rows = ExecutionProcess::list_missing_before_context(pool).await?;
        for row in rows {
            // Skip if no after commit at all (shouldn't happen due to WHERE)
            // Prefer previous process after-commit if present
            let mut before = row.prev_after_head_commit.clone();

            // Fallback to base branch commit OID
            if before.is_none() {
                let repo_path =
                    std::path::Path::new(row.git_repo_path.as_deref().unwrap_or_default());
                match self
                    .git()
                    .get_branch_oid(repo_path, row.base_branch.as_str())
                {
                    Ok(oid) => before = Some(oid),
                    Err(e) => {
                        tracing::warn!(
                            "Backfill: Failed to resolve base branch OID for attempt {} (branch {}): {}",
                            row.task_attempt_id,
                            row.base_branch,
                            e
                        );
                    }
                }
            }

            if let Some(before_oid) = before
                && let Err(e) =
                    ExecutionProcess::update_before_head_commit(pool, row.id, &before_oid).await
            {
                tracing::warn!(
                    "Backfill: Failed to update before_head_commit for process {}: {}",
                    row.id,
                    e
                );
            }
        }

        Ok(())
    }

    async fn stream_events(
        &self,
    ) -> futures::stream::BoxStream<'static, Result<Event, std::io::Error>> {
        self.events()
            .msg_store()
            .history_plus_stream()
            .map_ok(|m| m.to_sse_event())
            .boxed()
    }
}
