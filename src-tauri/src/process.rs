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
            return Err(AppError::Process(
                "process already running for installation".to_string(),
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
        let child = command.spawn()?;
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
