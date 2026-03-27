use crate::errors::{AppError, AppResult};
use crate::execution::output_command;
use crate::models::Installation;
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DependencyPlan {
    pub strategy: String,
    pub command: String,
    pub args: Vec<String>,
    pub cwd: String,
    pub reason: String,
}

#[derive(Debug, Deserialize)]
struct PyprojectProject {
    dependencies: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
struct PyprojectDocument {
    project: Option<PyprojectProject>,
}

fn pyproject_dependency_args(pyproject: &Path) -> Option<Vec<String>> {
    let content = std::fs::read_to_string(pyproject).ok()?;
    let document: PyprojectDocument = toml::from_str(&content).ok()?;
    document.project?.dependencies.filter(|deps| !deps.is_empty())
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
                "requirements.txt".to_string(),
            ],
            cwd: repo_path.to_string_lossy().to_string(),
            reason: "requirements.txt detected".to_string(),
        }
    } else if pyproject.exists() {
        if let Some(dependencies) = pyproject_dependency_args(&pyproject).filter(|deps| !deps.is_empty()) {
            let mut args = vec!["-m".to_string(), "pip".to_string(), "install".to_string()];
            args.extend(dependencies);
            DependencyPlan {
                strategy: "pyproject_dependencies".to_string(),
                command: installation.python_exe.clone(),
                args,
                cwd: repo_path.to_string_lossy().to_string(),
                reason: "pyproject.toml dependency metadata detected".to_string(),
            }
        } else {
            DependencyPlan {
                strategy: "none".to_string(),
                command: String::new(),
                args: Vec::new(),
                cwd: repo_path.to_string_lossy().to_string(),
                reason: "pyproject.toml detected, but no standalone dependency list was found"
                    .to_string(),
            }
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
    let output = output_command(&plan.command, &plan.args, Some(Path::new(&plan.cwd))).await?;
    if !output.status.success() {
        return Err(AppError::Dependency(format!(
            "{}\n{}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        )));
    }
    Ok(())
}
