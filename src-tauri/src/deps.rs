use crate::errors::{AppError, AppResult};
use crate::models::Installation;
use serde::{Deserialize, Serialize};
use std::path::Path;
use tokio::process::Command;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DependencyPlan {
    pub strategy: String,
    pub command: String,
    pub args: Vec<String>,
    pub cwd: String,
    pub reason: String,
}

pub fn plan_dependency_sync(installation: &Installation, repo_path: &Path) -> DependencyPlan {
    let requirements = repo_path.join("requirements.txt");
    let pyproject = repo_path.join("pyproject.toml");
    if requirements.exists() {
        DependencyPlan {
            strategy: "requirements".to_string(),
            command: installation.python_exe.clone(),
            args: vec![
                "-m".to_string(),
                "pip".to_string(),
                "install".to_string(),
                "-r".to_string(),
                requirements.to_string_lossy().to_string(),
            ],
            cwd: repo_path.to_string_lossy().to_string(),
            reason: "requirements.txt detected".to_string(),
        }
    } else if pyproject.exists() {
        DependencyPlan {
            strategy: "editable_pyproject".to_string(),
            command: installation.python_exe.clone(),
            args: vec![
                "-m".to_string(),
                "pip".to_string(),
                "install".to_string(),
                "-e".to_string(),
                ".".to_string(),
            ],
            cwd: repo_path.to_string_lossy().to_string(),
            reason: "pyproject.toml detected".to_string(),
        }
    } else {
        DependencyPlan {
            strategy: "none".to_string(),
            command: String::new(),
            args: Vec::new(),
            cwd: repo_path.to_string_lossy().to_string(),
            reason: "no supported dependency manifest found".to_string(),
        }
    }
}

pub async fn execute_dependency_sync(plan: &DependencyPlan) -> AppResult<()> {
    if plan.strategy == "none" {
        return Ok(());
    }
    let output = Command::new(&plan.command)
        .args(&plan.args)
        .current_dir(&plan.cwd)
        .output()
        .await?;
    if !output.status.success() {
        return Err(AppError::Dependency(format!(
            "{}\n{}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        )));
    }
    Ok(())
}
