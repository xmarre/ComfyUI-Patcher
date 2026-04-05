use crate::errors::{AppError, AppResult};
use crate::models::{
    FrontendSettings, Installation, InstallationDetail, LaunchProfile, ManagedRepo, OperationKind,
    OperationRecord, OperationStatus, RepoCheckpoint, RepoKind, TargetKind, TrackedBaseTarget,
    TrackedRepoState,
};
use crate::util::{new_id, now_rfc3339};
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use std::path::{Path, PathBuf};

#[derive(Clone)]
pub struct Database {
    path: PathBuf,
    logs_dir: PathBuf,
}

impl Database {
    pub fn new(app_data_dir: &Path) -> AppResult<Self> {
        let db_dir = app_data_dir.join("state");
        let logs_dir = app_data_dir.join("logs");
        std::fs::create_dir_all(&db_dir)?;
        std::fs::create_dir_all(&logs_dir)?;
        let path = db_dir.join("comfyui-patcher.sqlite3");
        let db = Self { path, logs_dir };
        db.init()?;
        Ok(db)
    }

    fn connect(&self) -> AppResult<Connection> {
        Ok(Connection::open(&self.path)?)
    }

    fn init(&self) -> AppResult<()> {
        let conn = self.connect()?;
        conn.execute_batch(
            r#"
            PRAGMA journal_mode = WAL;
            CREATE TABLE IF NOT EXISTS installations (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                comfy_root TEXT NOT NULL,
                python_exe TEXT NOT NULL,
                custom_nodes_dir TEXT NOT NULL,
                launch_profile_json TEXT,
                frontend_settings_json TEXT,
                detected_env_kind TEXT NOT NULL,
                is_git_repo INTEGER NOT NULL,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS managed_repos (
                id TEXT PRIMARY KEY,
                installation_id TEXT NOT NULL,
                kind TEXT NOT NULL,
                display_name TEXT NOT NULL,
                local_path TEXT NOT NULL,
                canonical_remote TEXT,
                current_head_sha TEXT,
                current_branch TEXT,
                is_detached INTEGER NOT NULL,
                is_dirty INTEGER NOT NULL,
                tracked_target_kind TEXT,
                tracked_target_input TEXT,
                tracked_target_resolved_sha TEXT,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );
            CREATE UNIQUE INDEX IF NOT EXISTS idx_managed_repos_installation_local_path
            ON managed_repos (installation_id, local_path);
            CREATE TABLE IF NOT EXISTS operations (
                id TEXT PRIMARY KEY,
                installation_id TEXT NOT NULL,
                repo_id TEXT,
                kind TEXT NOT NULL,
                status TEXT NOT NULL,
                requested_input TEXT,
                log_file TEXT NOT NULL,
                error_message TEXT,
                checkpoint_id TEXT,
                created_at TEXT NOT NULL,
                started_at TEXT,
                finished_at TEXT
            );
            CREATE TABLE IF NOT EXISTS repo_checkpoints (
                id TEXT PRIMARY KEY,
                repo_id TEXT NOT NULL,
                operation_id TEXT NOT NULL,
                old_head_sha TEXT NOT NULL,
                old_branch TEXT,
                old_is_detached INTEGER NOT NULL,
                has_tracked_target_snapshot INTEGER NOT NULL DEFAULT 0,
                old_tracked_target_kind TEXT,
                old_tracked_target_input TEXT,
                old_tracked_target_resolved_sha TEXT,
                stash_created INTEGER NOT NULL,
                stash_ref TEXT,
                created_at TEXT NOT NULL
            );
            "#,
        )?;
        ensure_installation_frontend_settings_column(&conn)?;
        ensure_repo_checkpoint_tracking_columns(&conn)?;
        let has_duplicate_roots = conn.query_row(
            "SELECT EXISTS(
                 SELECT 1
                 FROM installations
                 GROUP BY comfy_root
                 HAVING COUNT(*) > 1
                 LIMIT 1
             )",
            [],
            |row| row.get::<_, i64>(0),
        )? != 0;
        if !has_duplicate_roots {
            conn.execute(
                "CREATE UNIQUE INDEX IF NOT EXISTS idx_installations_comfy_root
                 ON installations (comfy_root)",
                [],
            )?;
        }
        Ok(())
    }

    pub fn update_installation(
        &self,
        installation_id: &str,
        name: &str,
        python_exe: &str,
        launch_profile: Option<&LaunchProfile>,
        frontend_settings: Option<&FrontendSettings>,
        detected_env_kind: &str,
        is_git_repo: bool,
    ) -> AppResult<Installation> {
        let conn = self.connect()?;
        let launch_profile_json = launch_profile.map(serde_json::to_string).transpose()?;
        let frontend_settings_json = frontend_settings.map(serde_json::to_string).transpose()?;
        conn.execute(
            "UPDATE installations
             SET name = ?2,
                 python_exe = ?3,
                 launch_profile_json = ?4,
                 frontend_settings_json = ?5,
                 detected_env_kind = ?6,
                 is_git_repo = ?7,
                 updated_at = ?8
             WHERE id = ?1",
            params![
                installation_id,
                name,
                python_exe,
                launch_profile_json,
                frontend_settings_json,
                detected_env_kind,
                is_git_repo as i64,
                now_rfc3339()
            ],
        )?;
        self.get_installation(installation_id)?
            .ok_or_else(|| AppError::Db("failed to reload installation".to_string()))
    }

    pub fn upsert_installation_by_root(
        &self,
        name: &str,
        comfy_root: &str,
        python_exe: Option<&str>,
        custom_nodes_dir: &str,
        launch_profile: Option<&LaunchProfile>,
        frontend_settings: Option<&FrontendSettings>,
        detected_env_kind: Option<&str>,
        is_git_repo: bool,
    ) -> AppResult<Installation> {
        let mut conn = self.connect()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let now = now_rfc3339();
        let existing = {
            let mut stmt = tx.prepare(
                "SELECT id, name, comfy_root, python_exe, custom_nodes_dir, launch_profile_json, frontend_settings_json, detected_env_kind, is_git_repo, created_at, updated_at
                 FROM installations
                 WHERE comfy_root = ?1
                 ORDER BY updated_at DESC, created_at DESC
                 LIMIT 1",
            )?;
            stmt.query_row(params![comfy_root], map_installation)
                .optional()?
        };

        let installation_id = if let Some(existing) = existing {
            let final_python_exe = python_exe.unwrap_or(existing.python_exe.as_str());
            let final_launch_profile = launch_profile.or(existing.launch_profile.as_ref());
            let final_launch_profile_json = final_launch_profile
                .map(serde_json::to_string)
                .transpose()?;
            let final_frontend_settings = frontend_settings.or(existing.frontend_settings.as_ref());
            let final_frontend_settings_json = final_frontend_settings
                .map(serde_json::to_string)
                .transpose()?;
            let final_detected_env_kind =
                detected_env_kind.unwrap_or(existing.detected_env_kind.as_str());
            tx.execute(
                "UPDATE installations
                 SET name = ?2,
                     python_exe = ?3,
                     launch_profile_json = ?4,
                     frontend_settings_json = ?5,
                     detected_env_kind = ?6,
                     is_git_repo = ?7,
                     updated_at = ?8
                 WHERE id = ?1",
                params![
                    existing.id,
                    name,
                    final_python_exe,
                    final_launch_profile_json,
                    final_frontend_settings_json,
                    final_detected_env_kind,
                    is_git_repo as i64,
                    now
                ],
            )?;
            existing.id
        } else {
            let python_exe = python_exe.ok_or_else(|| {
                AppError::InvalidInput(
                    "python executable must be provided for new installations".to_string(),
                )
            })?;
            let detected_env_kind = detected_env_kind.ok_or_else(|| {
                AppError::InvalidInput(
                    "detected environment kind must be provided for new installations".to_string(),
                )
            })?;
            let launch_profile_json = launch_profile.map(serde_json::to_string).transpose()?;
            let frontend_settings_json = frontend_settings.map(serde_json::to_string).transpose()?;
            let installation_id = new_id();
            tx.execute(
                "INSERT INTO installations
                 (id, name, comfy_root, python_exe, custom_nodes_dir, launch_profile_json, frontend_settings_json, detected_env_kind, is_git_repo, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?10)",
                params![
                    installation_id,
                    name,
                    comfy_root,
                    python_exe,
                    custom_nodes_dir,
                    launch_profile_json,
                    frontend_settings_json,
                    detected_env_kind,
                    is_git_repo as i64,
                    now
                ],
            )?;
            installation_id
        };

        tx.commit()?;
        self.get_installation(&installation_id)?
            .ok_or_else(|| AppError::Db("failed to reload installation".to_string()))
    }

    pub fn list_installations(&self) -> AppResult<Vec<Installation>> {
        let conn = self.connect()?;
        let mut stmt = conn.prepare(
            "SELECT id, name, comfy_root, python_exe, custom_nodes_dir, launch_profile_json, frontend_settings_json, detected_env_kind, is_git_repo, created_at, updated_at
             FROM installations
             ORDER BY name ASC",
        )?;
        let rows = stmt.query_map([], map_installation)?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    pub fn get_installation(&self, id: &str) -> AppResult<Option<Installation>> {
        let conn = self.connect()?;
        let mut stmt = conn.prepare(
            "SELECT id, name, comfy_root, python_exe, custom_nodes_dir, launch_profile_json, frontend_settings_json, detected_env_kind, is_git_repo, created_at, updated_at
             FROM installations WHERE id = ?1",
        )?;
        stmt.query_row(params![id], map_installation)
            .optional()
            .map_err(Into::into)
    }

    pub fn get_installation_by_root(&self, comfy_root: &str) -> AppResult<Option<Installation>> {
        let conn = self.connect()?;
        let mut stmt = conn.prepare(
            "SELECT id, name, comfy_root, python_exe, custom_nodes_dir, launch_profile_json, frontend_settings_json, detected_env_kind, is_git_repo, created_at, updated_at
             FROM installations WHERE comfy_root = ?1
             ORDER BY updated_at DESC, created_at DESC
             LIMIT 1",
        )?;
        stmt.query_row(params![comfy_root], map_installation)
            .optional()
            .map_err(Into::into)
    }

    pub fn delete_installation(&self, installation_id: &str) -> AppResult<()> {
        let conn = self.connect()?;
        let tx = conn.unchecked_transaction()?;
        let mut log_stmt = tx.prepare("SELECT log_file FROM operations WHERE installation_id = ?1")?;
        let log_rows = log_stmt.query_map(params![installation_id], |row| row.get::<_, String>(0))?;
        let mut log_files = Vec::new();
        for row in log_rows {
            log_files.push(row?);
        }
        drop(log_stmt);
        tx.execute(
            "DELETE FROM repo_checkpoints
             WHERE operation_id IN (
                 SELECT id FROM operations WHERE installation_id = ?1
             )",
            params![installation_id],
        )?;
        tx.execute("DELETE FROM operations WHERE installation_id = ?1", params![installation_id])?;
        tx.execute("DELETE FROM managed_repos WHERE installation_id = ?1", params![installation_id])?;
        tx.execute("DELETE FROM installations WHERE id = ?1", params![installation_id])?;
        tx.commit()?;

        for log_file in log_files {
            let _ = std::fs::remove_file(log_file);
        }
        Ok(())
    }

    pub fn get_installation_detail(&self, installation_id: &str) -> AppResult<InstallationDetail> {
        let installation = self
            .get_installation(installation_id)?
            .ok_or_else(|| AppError::NotFound("installation not found".to_string()))?;
        let repos = self.list_repos_by_installation(installation_id)?;
        let mut core_repo = None;
        let mut frontend_repo = None;
        let mut custom_node_repos = Vec::new();
        for repo in repos {
            match repo.kind {
                RepoKind::Core => core_repo = Some(repo),
                RepoKind::Frontend => frontend_repo = Some(repo),
                RepoKind::CustomNode => custom_node_repos.push(repo),
            }
        }
        Ok(InstallationDetail {
            installation,
            core_repo,
            frontend_repo,
            custom_node_repos,
            is_running: false,
        })
    }

    pub fn upsert_repo(
        &self,
        installation_id: &str,
        kind: RepoKind,
        display_name: &str,
        local_path: &str,
        canonical_remote: Option<&str>,
        current_head_sha: Option<&str>,
        current_branch: Option<&str>,
        is_detached: bool,
        is_dirty: bool,
    ) -> AppResult<ManagedRepo> {
        let conn = self.connect()?;
        let now = now_rfc3339();
        conn.execute(
            "INSERT INTO managed_repos
             (id, installation_id, kind, display_name, local_path, canonical_remote, current_head_sha, current_branch, is_detached, is_dirty,
              tracked_target_kind, tracked_target_input, tracked_target_resolved_sha, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, NULL, NULL, NULL, ?11, ?11)
             ON CONFLICT(installation_id, local_path) DO UPDATE SET
                 kind = excluded.kind,
                 display_name = excluded.display_name,
                 canonical_remote = excluded.canonical_remote,
                 current_head_sha = excluded.current_head_sha,
                 current_branch = excluded.current_branch,
                 is_detached = excluded.is_detached,
                 is_dirty = excluded.is_dirty,
                 updated_at = excluded.updated_at",
            params![
                new_id(),
                installation_id,
                serde_json::to_string(&kind)?,
                display_name,
                local_path,
                canonical_remote,
                current_head_sha,
                current_branch,
                is_detached as i64,
                is_dirty as i64,
                now
            ],
        )?;
        let repo_id: String = conn.query_row(
            "SELECT id FROM managed_repos WHERE installation_id = ?1 AND local_path = ?2",
            params![installation_id, local_path],
            |row| row.get(0),
        )?;
        self.get_repo(&repo_id)?
            .ok_or_else(|| AppError::Db("failed to reload repo".to_string()))
    }

    pub fn delete_repo(&self, repo_id: &str) -> AppResult<()> {
        let conn = self.connect()?;
        conn.execute("DELETE FROM managed_repos WHERE id = ?1", params![repo_id])?;
        Ok(())
    }

    pub fn delete_checkpoint(&self, checkpoint_id: &str) -> AppResult<()> {
        let conn = self.connect()?;
        conn.execute(
            "DELETE FROM repo_checkpoints WHERE id = ?1",
            params![checkpoint_id],
        )?;
        Ok(())
    }

    pub fn update_repo_state(
        &self,
        repo_id: &str,
        canonical_remote: Option<&str>,
        current_head_sha: Option<&str>,
        current_branch: Option<&str>,
        is_detached: bool,
        is_dirty: bool,
    ) -> AppResult<()> {
        let conn = self.connect()?;
        conn.execute(
            "UPDATE managed_repos
             SET canonical_remote = ?2, current_head_sha = ?3, current_branch = ?4,
                 is_detached = ?5, is_dirty = ?6, updated_at = ?7
             WHERE id = ?1",
            params![
                repo_id,
                canonical_remote,
                current_head_sha,
                current_branch,
                is_detached as i64,
                is_dirty as i64,
                now_rfc3339()
            ],
        )?;
        Ok(())
    }

    pub fn set_repo_tracked_state(
        &self,
        repo_id: &str,
        tracked_state: Option<&TrackedRepoState>,
        resolved_sha: Option<&str>,
    ) -> AppResult<()> {
        let conn = self.connect()?;
        let target_kind_json = tracked_state
            .map(|value| serde_json::to_string(&value.base.target_kind))
            .transpose()?;
        let target_input_json = tracked_state
            .map(serde_json::to_string)
            .transpose()?;
        conn.execute(
            "UPDATE managed_repos
             SET tracked_target_kind = ?2, tracked_target_input = ?3, tracked_target_resolved_sha = ?4, updated_at = ?5
             WHERE id = ?1",
            params![
                repo_id,
                target_kind_json,
                target_input_json,
                resolved_sha,
                now_rfc3339()
            ],
        )?;
        Ok(())
    }

    pub fn restore_repo_tracked_target(
        &self,
        repo_id: &str,
        target_kind: Option<&TargetKind>,
        target_input: Option<&str>,
        resolved_sha: Option<&str>,
    ) -> AppResult<()> {
        let conn = self.connect()?;
        let target_kind_json = target_kind
            .map(serde_json::to_string)
            .transpose()?;
        conn.execute(
            "UPDATE managed_repos
             SET tracked_target_kind = ?2, tracked_target_input = ?3, tracked_target_resolved_sha = ?4, updated_at = ?5
             WHERE id = ?1",
            params![
                repo_id,
                target_kind_json,
                target_input,
                resolved_sha,
                now_rfc3339()
            ],
        )?;
        Ok(())
    }

    pub fn get_repo(&self, repo_id: &str) -> AppResult<Option<ManagedRepo>> {
        let conn = self.connect()?;
        let mut stmt = conn.prepare(
            "SELECT id, installation_id, kind, display_name, local_path, canonical_remote, current_head_sha, current_branch,
                    is_detached, is_dirty, tracked_target_kind, tracked_target_input, tracked_target_resolved_sha, created_at, updated_at
             FROM managed_repos WHERE id = ?1",
        )?;
        stmt.query_row(params![repo_id], map_repo)
            .optional()
            .map_err(Into::into)
    }

    pub fn list_repos_by_installation(&self, installation_id: &str) -> AppResult<Vec<ManagedRepo>> {
        let conn = self.connect()?;
        let mut stmt = conn.prepare(
            "SELECT id, installation_id, kind, display_name, local_path, canonical_remote, current_head_sha, current_branch,
                    is_detached, is_dirty, tracked_target_kind, tracked_target_input, tracked_target_resolved_sha, created_at, updated_at
             FROM managed_repos WHERE installation_id = ?1 ORDER BY kind, display_name",
        )?;
        let rows = stmt.query_map(params![installation_id], map_repo)?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    pub fn create_operation(
        &self,
        installation_id: &str,
        repo_id: Option<&str>,
        kind: OperationKind,
        requested_input: Option<&str>,
    ) -> AppResult<OperationRecord> {
        let conn = self.connect()?;
        let id = new_id();
        let log_file = self.logs_dir.join(format!("{id}.log"));
        std::fs::write(&log_file, "")?;
        let now = now_rfc3339();
        conn.execute(
            "INSERT INTO operations
             (id, installation_id, repo_id, kind, status, requested_input, log_file, error_message, checkpoint_id, created_at, started_at, finished_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, NULL, NULL, ?8, NULL, NULL)",
            params![
                id,
                installation_id,
                repo_id,
                serde_json::to_string(&kind)?,
                serde_json::to_string(&OperationStatus::Queued)?,
                requested_input,
                log_file.to_string_lossy().to_string(),
                now
            ],
        )?;
        self.get_operation(&id)?
            .ok_or_else(|| AppError::Db("failed to reload operation".to_string()))
    }

    pub fn set_operation_running(&self, operation_id: &str) -> AppResult<()> {
        let conn = self.connect()?;
        conn.execute(
            "UPDATE operations SET status = ?2, started_at = ?3 WHERE id = ?1",
            params![
                operation_id,
                serde_json::to_string(&OperationStatus::Running)?,
                now_rfc3339()
            ],
        )?;
        Ok(())
    }

    pub fn finish_operation(
        &self,
        operation_id: &str,
        status: OperationStatus,
        error_message: Option<&str>,
        checkpoint_id: Option<&str>,
    ) -> AppResult<()> {
        let conn = self.connect()?;
        conn.execute(
            "UPDATE operations SET status = ?2, error_message = ?3, checkpoint_id = ?4, finished_at = ?5 WHERE id = ?1",
            params![
                operation_id,
                serde_json::to_string(&status)?,
                error_message,
                checkpoint_id,
                now_rfc3339()
            ],
        )?;
        Ok(())
    }

    pub fn get_operation(&self, id: &str) -> AppResult<Option<OperationRecord>> {
        let conn = self.connect()?;
        let mut stmt = conn.prepare(
            "SELECT id, installation_id, repo_id, kind, status, requested_input, log_file, error_message, checkpoint_id, created_at, started_at, finished_at
             FROM operations WHERE id = ?1",
        )?;
        stmt.query_row(params![id], map_operation)
            .optional()
            .map_err(Into::into)
    }

    pub fn list_operations(
        &self,
        installation_id: Option<&str>,
    ) -> AppResult<Vec<OperationRecord>> {
        let conn = self.connect()?;
        let sql = if installation_id.is_some() {
            "SELECT id, installation_id, repo_id, kind, status, requested_input, log_file, error_message, checkpoint_id, created_at, started_at, finished_at
             FROM operations WHERE installation_id = ?1 ORDER BY created_at DESC LIMIT 100"
        } else {
            "SELECT id, installation_id, repo_id, kind, status, requested_input, log_file, error_message, checkpoint_id, created_at, started_at, finished_at
             FROM operations ORDER BY created_at DESC LIMIT 100"
        };
        let mut stmt = conn.prepare(sql)?;
        let mut out = Vec::new();
        if let Some(installation_id) = installation_id {
            let rows = stmt.query_map(params![installation_id], map_operation)?;
            for row in rows {
                out.push(row?);
            }
        } else {
            let rows = stmt.query_map([], map_operation)?;
            for row in rows {
                out.push(row?);
            }
        }
        Ok(out)
    }

    pub fn has_in_flight_background_operations(&self) -> AppResult<bool> {
        let conn = self.connect()?;
        let queued = serde_json::to_string(&OperationStatus::Queued)?;
        let running = serde_json::to_string(&OperationStatus::Running)?;
        let start = serde_json::to_string(&OperationKind::StartInstallation)?;
        let stop = serde_json::to_string(&OperationKind::StopInstallation)?;
        let restart = serde_json::to_string(&OperationKind::RestartInstallation)?;
        let exists = conn.query_row(
            "SELECT EXISTS(
                 SELECT 1
                 FROM operations
                 WHERE status IN (?1, ?2)
                   AND kind NOT IN (?3, ?4, ?5)
                 LIMIT 1
             )",
            params![queued, running, start, stop, restart],
            |row| row.get::<_, i64>(0),
        )? != 0;
        Ok(exists)
    }

    pub fn append_operation_log(&self, operation_id: &str, line: &str) -> AppResult<()> {
        use std::io::Write;

        let op = self
            .get_operation(operation_id)?
            .ok_or_else(|| AppError::NotFound("operation not found".to_string()))?;
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&op.log_file)?;
        file.write_all(line.as_bytes())?;
        if !line.ends_with('\n') {
            file.write_all(b"\n")?;
        }
        Ok(())
    }

    pub fn get_operation_log(&self, operation_id: &str) -> AppResult<String> {
        let op = self
            .get_operation(operation_id)?
            .ok_or_else(|| AppError::NotFound("operation not found".to_string()))?;
        Ok(std::fs::read_to_string(op.log_file).unwrap_or_default())
    }

    pub fn create_checkpoint(
        &self,
        repo_id: &str,
        operation_id: &str,
        old_head_sha: &str,
        old_branch: Option<&str>,
        old_is_detached: bool,
        has_tracked_target_snapshot: bool,
        old_tracked_target_kind: Option<&TargetKind>,
        old_tracked_target_input: Option<&str>,
        old_tracked_target_resolved_sha: Option<&str>,
        stash_created: bool,
        stash_ref: Option<&str>,
    ) -> AppResult<RepoCheckpoint> {
        let conn = self.connect()?;
        let id = new_id();
        let now = now_rfc3339();
        let old_tracked_target_kind_json = old_tracked_target_kind
            .map(serde_json::to_string)
            .transpose()?;
        conn.execute(
            "INSERT INTO repo_checkpoints
             (id, repo_id, operation_id, old_head_sha, old_branch, old_is_detached,
              has_tracked_target_snapshot, old_tracked_target_kind, old_tracked_target_input,
              old_tracked_target_resolved_sha, stash_created, stash_ref, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
            params![
                id,
                repo_id,
                operation_id,
                old_head_sha,
                old_branch,
                old_is_detached as i64,
                has_tracked_target_snapshot as i64,
                old_tracked_target_kind_json,
                old_tracked_target_input,
                old_tracked_target_resolved_sha,
                stash_created as i64,
                stash_ref,
                now
            ],
        )?;
        self.get_checkpoint(&id)?
            .ok_or_else(|| AppError::Db("failed to reload checkpoint".to_string()))
    }

    pub fn update_checkpoint_stash(
        &self,
        checkpoint_id: &str,
        stash_created: bool,
        stash_ref: Option<&str>,
    ) -> AppResult<()> {
        let conn = self.connect()?;
        conn.execute(
            "UPDATE repo_checkpoints SET stash_created = ?2, stash_ref = ?3 WHERE id = ?1",
            params![checkpoint_id, stash_created as i64, stash_ref],
        )?;
        Ok(())
    }

    pub fn get_checkpoint(&self, checkpoint_id: &str) -> AppResult<Option<RepoCheckpoint>> {
        let conn = self.connect()?;
        let mut stmt = conn.prepare(
            "SELECT id, repo_id, operation_id, old_head_sha, old_branch, old_is_detached,
                    has_tracked_target_snapshot, old_tracked_target_kind, old_tracked_target_input,
                    old_tracked_target_resolved_sha, stash_created, stash_ref, created_at
             FROM repo_checkpoints WHERE id = ?1",
        )?;
        stmt.query_row(params![checkpoint_id], map_checkpoint)
            .optional()
            .map_err(Into::into)
    }

    pub fn list_checkpoints(&self, repo_id: &str) -> AppResult<Vec<RepoCheckpoint>> {
        let conn = self.connect()?;
        let mut stmt = conn.prepare(
            "SELECT id, repo_id, operation_id, old_head_sha, old_branch, old_is_detached,
                    has_tracked_target_snapshot, old_tracked_target_kind, old_tracked_target_input,
                    old_tracked_target_resolved_sha, stash_created, stash_ref, created_at
             FROM repo_checkpoints WHERE repo_id = ?1 ORDER BY created_at DESC",
        )?;
        let rows = stmt.query_map(params![repo_id], map_checkpoint)?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    pub fn latest_checkpoint(&self, repo_id: &str) -> AppResult<Option<RepoCheckpoint>> {
        let conn = self.connect()?;
        let mut stmt = conn.prepare(
            "SELECT id, repo_id, operation_id, old_head_sha, old_branch, old_is_detached,
                    has_tracked_target_snapshot, old_tracked_target_kind, old_tracked_target_input,
                    old_tracked_target_resolved_sha, stash_created, stash_ref, created_at
             FROM repo_checkpoints WHERE repo_id = ?1 ORDER BY created_at DESC LIMIT 1",
        )?;
        stmt.query_row(params![repo_id], map_checkpoint)
            .optional()
            .map_err(Into::into)
    }
}

fn map_installation(row: &rusqlite::Row<'_>) -> rusqlite::Result<Installation> {
    let launch_json: Option<String> = row.get(5)?;
    let frontend_settings_json: Option<String> = row.get(6)?;
    Ok(Installation {
        id: row.get(0)?,
        name: row.get(1)?,
        comfy_root: row.get(2)?,
        python_exe: row.get(3)?,
        custom_nodes_dir: row.get(4)?,
        launch_profile: launch_json
            .map(|json| serde_json::from_str(&json))
            .transpose()
            .map_err(to_sql_err)?,
        frontend_settings: frontend_settings_json
            .map(|json| serde_json::from_str(&json))
            .transpose()
            .map_err(to_sql_err)?,
        detected_env_kind: row.get(7)?,
        is_git_repo: row.get::<_, i64>(8)? != 0,
        created_at: row.get(9)?,
        updated_at: row.get(10)?,
    })
}

fn map_repo(row: &rusqlite::Row<'_>) -> rusqlite::Result<ManagedRepo> {
    let kind_json: String = row.get(2)?;
    let tracked_kind_json: Option<String> = row.get(10)?;
    let canonical_remote: Option<String> = row.get(5)?;
    let tracked_target_input: Option<String> = row.get(11)?;
    let tracked_target_resolved_sha: Option<String> = row.get(12)?;
    let tracked_target_kind = tracked_kind_json
        .map(|json| serde_json::from_str(&json))
        .transpose()
        .map_err(to_sql_err)?;
    Ok(ManagedRepo {
        id: row.get(0)?,
        installation_id: row.get(1)?,
        kind: serde_json::from_str(&kind_json).map_err(to_sql_err)?,
        display_name: row.get(3)?,
        local_path: row.get(4)?,
        canonical_remote: canonical_remote.clone(),
        current_head_sha: row.get(6)?,
        current_branch: row.get(7)?,
        is_detached: row.get::<_, i64>(8)? != 0,
        is_dirty: row.get::<_, i64>(9)? != 0,
        tracked_target_kind: tracked_target_kind.clone(),
        tracked_target_input: tracked_target_input.clone(),
        tracked_target_resolved_sha: tracked_target_resolved_sha.clone(),
        tracked_state: parse_tracked_repo_state(
            tracked_target_kind.as_ref(),
            tracked_target_input.as_deref(),
            tracked_target_resolved_sha.as_deref(),
            canonical_remote.as_deref(),
        )?,
        created_at: row.get(13)?,
        updated_at: row.get(14)?,
    })
}

fn parse_tracked_repo_state(
    tracked_kind: Option<&TargetKind>,
    tracked_input: Option<&str>,
    tracked_resolved_sha: Option<&str>,
    canonical_remote: Option<&str>,
) -> rusqlite::Result<Option<TrackedRepoState>> {
    let tracked_input = match tracked_input {
        Some(tracked_input) => tracked_input.trim(),
        None => return Ok(None),
    };
    if tracked_input.is_empty() {
        return Ok(None);
    }

    if tracked_input.starts_with('{') {
        return serde_json::from_str::<TrackedRepoState>(tracked_input)
            .map(Some)
            .map_err(to_sql_err);
    }

    if let Ok(parsed) = serde_json::from_str::<TrackedRepoState>(tracked_input) {
        return Ok(Some(parsed));
    }

    let tracked_kind = match tracked_kind {
        Some(tracked_kind) => tracked_kind,
        None => return Ok(None),
    };
    if matches!(tracked_kind, TargetKind::Pr) {
        return Ok(None);
    }

    let canonical_repo_url = match canonical_remote {
        Some(canonical_remote) => canonical_remote.to_string(),
        None => return Ok(None),
    };
    Ok(Some(TrackedRepoState {
        version: 1,
        base: TrackedBaseTarget {
            source_input: tracked_input.to_string(),
            target_kind: tracked_kind.clone(),
            canonical_repo_url,
            checkout_ref: tracked_input.to_string(),
            resolved_sha: tracked_resolved_sha.map(ToOwned::to_owned),
            summary_label: tracked_input.to_string(),
        },
        overlays: Vec::new(),
        materialized_branch: None,
    }))
}

fn map_operation(row: &rusqlite::Row<'_>) -> rusqlite::Result<OperationRecord> {
    let kind_json: String = row.get(3)?;
    let status_json: String = row.get(4)?;
    Ok(OperationRecord {
        id: row.get(0)?,
        installation_id: row.get(1)?,
        repo_id: row.get(2)?,
        kind: serde_json::from_str(&kind_json).map_err(to_sql_err)?,
        status: serde_json::from_str(&status_json).map_err(to_sql_err)?,
        requested_input: row.get(5)?,
        log_file: row.get(6)?,
        error_message: row.get(7)?,
        checkpoint_id: row.get(8)?,
        created_at: row.get(9)?,
        started_at: row.get(10)?,
        finished_at: row.get(11)?,
    })
}

fn map_checkpoint(row: &rusqlite::Row<'_>) -> rusqlite::Result<RepoCheckpoint> {
    let old_tracked_target_kind_json: Option<String> = row.get(7)?;
    Ok(RepoCheckpoint {
        id: row.get(0)?,
        repo_id: row.get(1)?,
        operation_id: row.get(2)?,
        old_head_sha: row.get(3)?,
        old_branch: row.get(4)?,
        old_is_detached: row.get::<_, i64>(5)? != 0,
        has_tracked_target_snapshot: row.get::<_, i64>(6)? != 0,
        old_tracked_target_kind: old_tracked_target_kind_json
            .map(|json| serde_json::from_str(&json))
            .transpose()
            .map_err(to_sql_err)?,
        old_tracked_target_input: row.get(8)?,
        old_tracked_target_resolved_sha: row.get(9)?,
        stash_created: row.get::<_, i64>(10)? != 0,
        stash_ref: row.get(11)?,
        created_at: row.get(12)?,
    })
}

fn ensure_installation_frontend_settings_column(conn: &Connection) -> AppResult<()> {
    let mut stmt = conn.prepare("PRAGMA table_info(installations)")?;
    let rows = stmt.query_map([], |row| row.get::<_, String>(1))?;
    let mut columns = Vec::new();
    for row in rows {
        columns.push(row?);
    }
    drop(stmt);

    if !columns.iter().any(|existing| existing == "frontend_settings_json") {
        conn.execute(
            "ALTER TABLE installations ADD COLUMN frontend_settings_json TEXT",
            [],
        )?;
    }
    Ok(())
}

fn ensure_repo_checkpoint_tracking_columns(conn: &Connection) -> AppResult<()> {
    let mut stmt = conn.prepare("PRAGMA table_info(repo_checkpoints)")?;
    let rows = stmt.query_map([], |row| row.get::<_, String>(1))?;
    let mut columns = Vec::new();
    for row in rows {
        columns.push(row?);
    }
    drop(stmt);

    for (column_name, sql_type) in [
        (
            "has_tracked_target_snapshot",
            "INTEGER NOT NULL DEFAULT 0",
        ),
        ("old_tracked_target_kind", "TEXT"),
        ("old_tracked_target_input", "TEXT"),
        ("old_tracked_target_resolved_sha", "TEXT"),
    ] {
        if !columns.iter().any(|existing| existing == column_name) {
            conn.execute(
                &format!("ALTER TABLE repo_checkpoints ADD COLUMN {column_name} {sql_type}"),
                [],
            )?;
        }
    }
    Ok(())
}

fn to_sql_err(err: serde_json::Error) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(err))
}
