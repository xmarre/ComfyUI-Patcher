use crate::errors::{AppError, AppResult};
use crate::models::LaunchProfile;
use std::collections::HashMap;
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
            return Err(AppError::Process("process already running for installation".to_string()));
        }
        let mut command = Command::new(&profile.command);
        command.args(&profile.args);
        if let Some(cwd) = &profile.cwd {
            command.current_dir(cwd);
        }
        if let Some(env) = &profile.env {
            command.envs(env);
        }
        let child = command.spawn()?;
        map.insert(installation_id.to_string(), child);
        Ok(())
    }

    pub async fn stop(&self, installation_id: &str) -> AppResult<()> {
        let mut map = self.inner.lock().await;
        let mut child = map
            .remove(installation_id)
            .ok_or_else(|| AppError::Process("no managed child process is running".to_string()))?;
        child.kill().await.map_err(|e| AppError::Process(e.to_string()))?;
        Ok(())
    }

    pub async fn restart(&self, installation_id: &str, profile: &LaunchProfile) -> AppResult<()> {
        let _ = self.stop(installation_id).await;
        self.start(installation_id, profile).await
    }
}
