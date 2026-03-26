use crate::errors::{AppError, AppResult};
use crate::execution::{parse_wsl_unc_path, spawn_command};
use crate::models::LaunchProfile;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use tokio::process::{Child, Command};
use tokio::sync::Mutex;

#[derive(Clone)]
pub struct ProcessRegistry {
    inner: Arc<Mutex<HashMap<String, Child>>>,
}

impl ProcessRegistry {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub async fn start(&self, installation_id: &str, profile: &LaunchProfile) -> AppResult<()> {
        let mut map = self.inner.lock().await;
        if map.contains_key(installation_id) {
            return Err(AppError::Process(
                "process already running for installation".to_string(),
            ));
        }
        let cwd = profile.cwd.as_deref().map(Path::new);
        let child = if profile.env.as_ref().is_some_and(|env| !env.is_empty()) {
            let program_is_wsl = parse_wsl_unc_path(Path::new(&profile.command)).is_some();
            let cwd_is_wsl = cwd.and_then(parse_wsl_unc_path).is_some();
            if program_is_wsl || cwd_is_wsl {
                return Err(AppError::Process(
                    "launch profiles with custom environment variables are not supported for WSL-backed installations"
                        .to_string(),
                ));
            }
            let mut command = Command::new(&profile.command);
            command.args(&profile.args);
            if let Some(cwd) = &profile.cwd {
                command.current_dir(cwd);
            }
            if let Some(env) = &profile.env {
                command.envs(env);
            }
            command
                .spawn()
                .map_err(|e| AppError::Process(e.to_string()))?
        } else {
            spawn_command(&profile.command, &profile.args, cwd)
                .map_err(|e| AppError::Process(e.to_string()))?
        };
        map.insert(installation_id.to_string(), child);
        Ok(())
    }

    pub async fn stop(&self, installation_id: &str) -> AppResult<bool> {
        let mut map = self.inner.lock().await;
        let child = match map.get_mut(installation_id) {
            Some(child) => child,
            None => return Ok(false),
        };
        child
            .start_kill()
            .map_err(|e| AppError::Process(e.to_string()))?;
        let mut child = map
            .remove(installation_id)
            .ok_or_else(|| AppError::Process("process registry lost managed child".to_string()))?;
        drop(map);
        if let Err(err) = child.wait().await {
            let mut map = self.inner.lock().await;
            map.insert(installation_id.to_string(), child);
            return Err(AppError::Process(err.to_string()));
        }
        Ok(true)
    }

    pub async fn restart(&self, installation_id: &str, profile: &LaunchProfile) -> AppResult<()> {
        let _ = self.stop(installation_id).await?;
        self.start(installation_id, profile).await
    }
}
