use crate::errors::{AppError, AppResult};
use crate::execution::output_command;
use crate::models::{
    DependencyPlan, DependencyStep, FrontendPackageManager, Installation, ManagedRepo, RepoKind,
};
use serde::Deserialize;
use serde_json::Value;
use std::path::Path;

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

fn single_step_plan(step: DependencyStep) -> DependencyPlan {
    DependencyPlan {
        strategy: step.strategy.clone(),
        reason: step.reason.clone(),
        steps: vec![step],
    }
}

fn python_dependency_plan(installation: &Installation, repo_path: &Path) -> AppResult<DependencyPlan> {
    let requirements = repo_path.join("requirements.txt");
    let pyproject = repo_path.join("pyproject.toml");
    if requirements.exists() {
        Ok(single_step_plan(DependencyStep {
            phase: "install".to_string(),
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
        }))
    } else if pyproject.exists() {
        match pyproject_dependency_args(&pyproject)? {
            Some(dependencies) => {
                let mut args = vec!["-m".to_string(), "pip".to_string(), "install".to_string()];
                args.extend(dependencies);
                Ok(single_step_plan(DependencyStep {
                    phase: "install".to_string(),
                    strategy: "pyproject_dependencies".to_string(),
                    command: installation.python_exe.clone(),
                    args,
                    cwd: repo_path.to_string_lossy().to_string(),
                    reason: "pyproject.toml dependency metadata detected".to_string(),
                }))
            }
            None => Ok(DependencyPlan {
                strategy: "none".to_string(),
                reason: "pyproject.toml detected, but no standalone dependency list was found"
                    .to_string(),
                steps: Vec::new(),
            }),
        }
    } else {
        Ok(DependencyPlan {
            strategy: "none".to_string(),
            reason: "no supported dependency manifest found".to_string(),
            steps: Vec::new(),
        })
    }
}

fn read_package_json(repo_path: &Path) -> AppResult<Value> {
    let package_json = repo_path.join("package.json");
    let content = std::fs::read_to_string(&package_json).map_err(|error| {
        AppError::Dependency(format!(
            "failed to read {}: {error}",
            package_json.display()
        ))
    })?;
    serde_json::from_str(&content).map_err(|error| {
        AppError::Dependency(format!(
            "failed to parse {}: {error}",
            package_json.display()
        ))
    })
}

fn package_has_build_script(package_json: &Value) -> bool {
    package_json
        .get("scripts")
        .and_then(|value| value.as_object())
        .and_then(|scripts| scripts.get("build"))
        .is_some_and(|value| value.is_string())
}

fn package_manager_from_package_json(package_json: &Value) -> Option<FrontendPackageManager> {
    let raw = package_json.get("packageManager")?.as_str()?.trim();
    let name = raw.split('@').next()?.trim();
    match name {
        "npm" => Some(FrontendPackageManager::Npm),
        "pnpm" => Some(FrontendPackageManager::Pnpm),
        "yarn" => Some(FrontendPackageManager::Yarn),
        _ => None,
    }
}

fn detect_frontend_package_manager(
    repo_path: &Path,
    preferred: &FrontendPackageManager,
    package_json: &Value,
) -> FrontendPackageManager {
    if !matches!(preferred, FrontendPackageManager::Auto) {
        return preferred.clone();
    }
    if let Some(value) = package_manager_from_package_json(package_json) {
        return value;
    }
    if repo_path.join("pnpm-lock.yaml").exists() {
        FrontendPackageManager::Pnpm
    } else if repo_path.join("yarn.lock").exists() {
        FrontendPackageManager::Yarn
    } else {
        FrontendPackageManager::Npm
    }
}

fn frontend_command_name(package_manager: &FrontendPackageManager) -> &'static str {
    match package_manager {
        FrontendPackageManager::Auto => "npm",
        FrontendPackageManager::Npm => "npm",
        FrontendPackageManager::Pnpm => "pnpm",
        FrontendPackageManager::Yarn => "yarn",
    }
}

fn frontend_install_args(package_manager: &FrontendPackageManager) -> Vec<String> {
    match package_manager {
        FrontendPackageManager::Npm | FrontendPackageManager::Auto => vec!["install".to_string()],
        FrontendPackageManager::Pnpm => {
            vec!["install".to_string(), "--frozen-lockfile".to_string()]
        }
        FrontendPackageManager::Yarn => vec!["install".to_string()],
    }
}

fn frontend_build_args(package_manager: &FrontendPackageManager) -> Vec<String> {
    match package_manager {
        FrontendPackageManager::Npm | FrontendPackageManager::Pnpm | FrontendPackageManager::Auto => {
            vec!["run".to_string(), "build".to_string()]
        }
        FrontendPackageManager::Yarn => vec!["build".to_string()],
    }
}

fn frontend_dependency_plan(installation: &Installation, repo_path: &Path) -> AppResult<DependencyPlan> {
    let frontend_settings = installation.frontend_settings.as_ref().ok_or_else(|| {
        AppError::Dependency("installation has no managed frontend settings".to_string())
    })?;
    let package_json = read_package_json(repo_path)?;
    if !package_has_build_script(&package_json) {
        return Err(AppError::Dependency(
            "managed frontend repo has no build script in package.json".to_string(),
        ));
    }
    let package_manager =
        detect_frontend_package_manager(repo_path, &frontend_settings.package_manager, &package_json);
    let command = frontend_command_name(&package_manager).to_string();
    let cwd = repo_path.to_string_lossy().to_string();
    Ok(DependencyPlan {
        strategy: match package_manager {
            FrontendPackageManager::Auto => "node_auto".to_string(),
            FrontendPackageManager::Npm => "npm".to_string(),
            FrontendPackageManager::Pnpm => "pnpm".to_string(),
            FrontendPackageManager::Yarn => "yarn".to_string(),
        },
        reason: format!(
            "package.json with build script detected; using {}",
            frontend_command_name(&package_manager)
        ),
        steps: vec![
            DependencyStep {
                phase: "install".to_string(),
                strategy: format!("{}_install", frontend_command_name(&package_manager)),
                command: command.clone(),
                args: frontend_install_args(&package_manager),
                cwd: cwd.clone(),
                reason: "frontend dependency install".to_string(),
            },
            DependencyStep {
                phase: "build".to_string(),
                strategy: format!("{}_build", frontend_command_name(&package_manager)),
                command,
                args: frontend_build_args(&package_manager),
                cwd,
                reason: format!(
                    "frontend build; expected output {}",
                    frontend_settings.dist_path
                ),
            },
        ],
    })
}

pub fn plan_dependency_sync(
    installation: &Installation,
    repo: &ManagedRepo,
    repo_path: &Path,
) -> AppResult<DependencyPlan> {
    match repo.kind {
        RepoKind::Core | RepoKind::CustomNode => python_dependency_plan(installation, repo_path),
        RepoKind::Frontend => frontend_dependency_plan(installation, repo_path),
    }
}

pub async fn execute_dependency_sync(plan: &DependencyPlan) -> AppResult<()> {
    for step in &plan.steps {
        let output = output_command(&step.command, &step.args, Some(Path::new(&step.cwd))).await?;
        if !output.status.success() {
            return Err(AppError::Dependency(format!(
                "{} step failed ({}): {}\n{}",
                step.phase,
                step.strategy,
                output.status,
                String::from_utf8_lossy(&output.stderr)
            )));
        }
    }
    Ok(())
}
