use crate::db::Database;
use crate::errors::AppResult;
use crate::github::GithubClient;
use crate::process::ProcessRegistry;
use crate::registry::ManagerRegistryClient;
use std::collections::HashMap;
use std::sync::Arc;
use tauri::{AppHandle, Manager};
use tokio::sync::Mutex;

#[derive(Clone)]
pub struct AppState {
    pub db: Database,
    pub github: GithubClient,
    pub processes: ProcessRegistry,
    pub manager_registry: ManagerRegistryClient,
    lifecycle_lock: Arc<Mutex<()>>,
    repo_locks: Arc<Mutex<HashMap<String, Arc<Mutex<()>>>>>,
    installation_locks: Arc<Mutex<HashMap<String, Arc<Mutex<()>>>>>,
}

impl AppState {
    pub fn new(app: &AppHandle) -> AppResult<Self> {
        let resolver = app.path();
        let data_dir = resolver
            .app_data_dir()
            .map_err(|e| crate::errors::AppError::Io(e.to_string()))?;
        std::fs::create_dir_all(&data_dir)?;
        let db = Database::new(&data_dir)?;
        let github = GithubClient::new(std::env::var("GITHUB_TOKEN").ok())?;
        let manager_registry = ManagerRegistryClient::new()?;
        Ok(Self {
            db,
            github,
            processes: ProcessRegistry::new(),
            manager_registry,
            lifecycle_lock: Arc::new(Mutex::new(())),
            repo_locks: Arc::new(Mutex::new(HashMap::new())),
            installation_locks: Arc::new(Mutex::new(HashMap::new())),
        })
    }

    pub fn lifecycle_lock(&self) -> Arc<Mutex<()>> {
        self.lifecycle_lock.clone()
    }

    pub async fn repo_lock(&self, repo_id: &str) -> Arc<Mutex<()>> {
        let mut map = self.repo_locks.lock().await;
        map.entry(repo_id.to_string())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
    }

    pub async fn installation_lock(&self, installation_id: &str) -> Arc<Mutex<()>> {
        let mut map = self.installation_locks.lock().await;
        map.entry(installation_id.to_string())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
    }
}
