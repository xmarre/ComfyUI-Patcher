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

fn pyproject_dependency_args(pyproject: &Path) -> AppResult<Option<Vec<String>>> {
    let content = std::fs::read_to_string(pyproject).map_err(|error| {
        AppError::Dependency(format!(
            "failed to read {}: {error}",
            pyproject.display()
        ))
    })?;
    let document: PyprojectDocument = toml::from_str(&content).map_err(|error| {
        AppError::Dependency(format!(
            "failed to parse {}: {error}",
            pyproject.display()
        ))
    })?;
    Ok(document
        .project
        .and_then(|project| project.dependencies)
        .filter(|deps| !deps.is_empty()))
}

pub fn plan_dependency_sync(
    installation: &Installation,
    repo_path: &Path,
) -> AppResult<DependencyPlan> {
    let requirements = repo_path.join("requirements.txt");
    let pyproject = repo_path.join("pyproject.toml");
    if requirements.exists() {
        Ok(DependencyPlan {
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
        })
    } else if pyproject.exists() {
        match pyproject_dependency_args(&pyproject)? {
            Some(dependencies) => {
                let mut args = vec!["-m".to_string(), "pip".to_string(), "install".to_string()];
                args.extend(dependencies);
                Ok(DependencyPlan {
                    strategy: "pyproject_dependencies".to_string(),
                    command: installation.python_exe.clone(),
                    args,
                    cwd: repo_path.to_string_lossy().to_string(),
                    reason: "pyproject.toml dependency metadata detected".to_string(),
                })
            }
            None => Ok(DependencyPlan {
                strategy: "none".to_string(),
                command: String::new(),
                args: Vec::new(),
                cwd: repo_path.to_string_lossy().to_string(),
                reason: "pyproject.toml detected, but no standalone dependency list was found"
                    .to_string(),
            }),
        }
    } else {
        Ok(DependencyPlan {
            strategy: "none".to_string(),
            command: String::new(),
            args: Vec::new(),
            cwd: repo_path.to_string_lossy().to_string(),
            reason: "no supported dependency manifest found".to_string(),
        })
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
