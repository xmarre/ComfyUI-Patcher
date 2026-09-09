use crate::errors::{AppError, AppResult};
use crate::execution::output_command;
use crate::models::{
    DependencyPlan, DependencyStep, FrontendPackageManager, Installation, ManagedRepo, RepoKind,
};
use serde::Deserialize;
use serde_json::Value;
use std::path::Path;
use std::process::Output;

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
        FrontendPackageManager::Pnpm => vec!["install".to_string(), "--frozen-lockfile".to_string()],
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

fn kitchen_project_plan() -> DependencyPlan {
    DependencyPlan {
        strategy: "project_materialization".to_string(),
        reason: "Comfy Kitchen is built and installed as a source project; generic dependency sync does not install the project itself"
            .to_string(),
        steps: Vec::new(),
    }
}

pub fn plan_dependency_sync(
    installation: &Installation,
    repo: &ManagedRepo,
    repo_path: &Path,
) -> AppResult<DependencyPlan> {
    match repo.kind {
        RepoKind::Core | RepoKind::CustomNode => python_dependency_plan(installation, repo_path),
        RepoKind::Frontend => frontend_dependency_plan(installation, repo_path),
        RepoKind::Kitchen => Ok(kitchen_project_plan()),
    }
}

pub async fn execute_dependency_sync(plan: &DependencyPlan) -> AppResult<()> {
    for step in &plan.steps {
        let output = execute_dependency_step(step).await?;
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

async fn execute_dependency_step(step: &DependencyStep) -> AppResult<Output> {
    let output = output_command(&step.command, &step.args, Some(Path::new(&step.cwd))).await?;
    if output.status.success() {
        return Ok(output);
    }
    if should_retry_pnpm_without_frozen_lockfile(step, &output) {
        eprintln!(
            "dependency sync: pnpm frozen lockfile install failed due to lockfile mismatch; retrying without frozen lockfile"
        );
        let retry_args = vec!["install".to_string(), "--no-frozen-lockfile".to_string()];
        return output_command(&step.command, &retry_args, Some(Path::new(&step.cwd)))
            .await
            .map_err(AppError::from);
    }
    Ok(output)
}

fn should_retry_pnpm_without_frozen_lockfile(step: &DependencyStep, output: &Output) -> bool {
    if step.command != "pnpm"
        || step.phase != "install"
        || step.args != ["install".to_string(), "--frozen-lockfile".to_string()]
        || output.status.success()
    {
        return false;
    }
    let stderr = String::from_utf8_lossy(&output.stderr).to_lowercase();
    let stdout = String::from_utf8_lossy(&output.stdout).to_lowercase();
    let combined = format!("{stderr}\n{stdout}");
    combined.contains("err_pnpm_outdated_lockfile")
        || combined.contains("cannot install with \"frozen-lockfile\"")
        || combined.contains("frozen-lockfile")
            && (combined.contains("not up to date")
                || combined.contains("pnpm-lock.yaml is absent")
                || combined.contains("headless installation requires a pnpm-lock.yaml file"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{RepoLiveStatus, TargetKind};

    #[test]
    fn kitchen_is_not_routed_through_generic_pyproject_dependency_sync() {
        let installation = Installation {
            id: "i".to_string(),
            name: "test".to_string(),
            comfy_root: "/comfy".to_string(),
            python_exe: "/venv/bin/python".to_string(),
            custom_nodes_dir: "/comfy/custom_nodes".to_string(),
            launch_profile: None,
            frontend_settings: None,
            detected_env_kind: "venv".to_string(),
            is_git_repo: true,
            last_reconciled_at: None,
            created_at: "now".to_string(),
            updated_at: "now".to_string(),
        };
        let repo = ManagedRepo {
            id: "r".to_string(),
            installation_id: "i".to_string(),
            kind: RepoKind::Kitchen,
            display_name: "Comfy Kitchen".to_string(),
            local_path: "/comfy-kitchen".to_string(),
            canonical_remote: None,
            current_head_sha: Some("abc".to_string()),
            current_branch: Some("main".to_string()),
            is_detached: false,
            is_dirty: false,
            tracked_target_kind: Some(TargetKind::DefaultBranch),
            tracked_target_input: None,
            tracked_target_resolved_sha: Some("abc".to_string()),
            tracked_state: None,
            live_status: RepoLiveStatus::Clean,
            live_warnings: vec![],
            changed_files: vec![],
            dependency_state: None,
            materialization_state: None,
            last_scanned_at: None,
            created_at: "now".to_string(),
            updated_at: "now".to_string(),
        };
        let plan = plan_dependency_sync(&installation, &repo, Path::new(&repo.local_path)).unwrap();
        assert_eq!(plan.strategy, "project_materialization");
        assert!(plan.steps.is_empty());
    }
}
