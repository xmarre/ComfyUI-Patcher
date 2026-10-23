use crate::errors::{AppError, AppResult};
use crate::execution::{output_command, parse_wsl_unc_path};
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

/// Include both streams: pnpm and TypeScript diagnostics may be printed to stdout.
fn dependency_failure_output(stdout: &[u8], stderr: &[u8]) -> String {
    const MAX_CHARS: usize = 24_000;
    fn tail(bytes: &[u8]) -> Option<String> {
        let text = String::from_utf8_lossy(bytes);
        let text = text.trim();
        if text.is_empty() {
            return None;
        }
        let chars: Vec<char> = text.chars().rev().take(MAX_CHARS + 1).collect();
        if chars.len() > MAX_CHARS {
            Some(format!(
                "[earlier output truncated]\n{}",
                chars[..MAX_CHARS].iter().rev().collect::<String>()
            ))
        } else {
            Some(text.to_owned())
        }
    }

    let mut parts = Vec::new();
    if let Some(stderr) = tail(stderr) {
        parts.push(format!("stderr:\n{stderr}"));
    }
    if let Some(stdout) = tail(stdout) {
        parts.push(format!("stdout:\n{stdout}"));
    }
    if parts.is_empty() {
        "command produced no stdout or stderr".to_owned()
    } else {
        parts.join("\n")
    }
}

const MANAGED_KITCHEN_PIP_BOOTSTRAP: &str = include_str!("managed_kitchen_pip.py");

/// Protect a source-managed Kitchen build without changing tracked requirements.
///
/// Python runs in the selected installation interpreter (including WSL), so its
/// temporary requirements files and constraint are interpreted by the same pip.
/// The installed source version is a resolver constraint: incompatible third-party
/// requirements fail instead of silently replacing the managed runtime.
pub fn protect_managed_kitchen_dependency_plan(
    plan: &DependencyPlan,
    installed_version: &str,
) -> AppResult<DependencyPlan> {
    let mut guarded = plan.clone();
    for step in &mut guarded.steps {
        if !matches!(
            step.strategy.as_str(),
            "requirements" | "pyproject_dependencies"
        ) {
            continue;
        }
        if step.args.len() < 3 || step.args[0] != "-m" || step.args[1] != "pip"
            || step.args[2] != "install"
        {
            return Err(AppError::Dependency(format!(
                "cannot protect managed Comfy Kitchen during unsupported pip step: {}",
                step.strategy
            )));
        }
        let mut args = vec![
            "-c".to_string(),
            MANAGED_KITCHEN_PIP_BOOTSTRAP.to_string(),
            installed_version.to_string(),
        ];
        args.extend_from_slice(&step.args[2..]);
        step.args = args;
        step.reason = format!(
            "{}; preserving the source-managed Comfy Kitchen distribution",
            step.reason
        );
    }
    Ok(guarded)
}

pub async fn execute_dependency_sync(plan: &DependencyPlan) -> AppResult<()> {
    for step in &plan.steps {
        let output = execute_dependency_step(step).await?;
        if output.status.success() {
            continue;
        }

        if should_retry_pnpm_build_after_forced_install(step, &output) {
            eprintln!(
                "dependency sync: pnpm build could not resolve an installed dependency; forcing one frozen-lockfile reinstall before retrying the build"
            );
            let repair_output = force_reinstall_pnpm_dependencies(step).await?;
            if !repair_output.status.success() {
                return Err(AppError::Dependency(format!(
                    "{} step failed ({}): {}\n{}\npnpm dependency self-heal also failed during forced frozen-lockfile install: {}\n{}",
                    step.phase,
                    step.strategy,
                    output.status,
                    dependency_failure_output(&output.stdout, &output.stderr),
                    repair_output.status,
                    dependency_failure_output(&repair_output.stdout, &repair_output.stderr)
                )));
            }

            let retry_output =
                output_command(&step.command, &step.args, Some(Path::new(&step.cwd))).await?;
            if retry_output.status.success() {
                continue;
            }

            if looks_like_broken_pnpm_dependency_graph(
                &retry_output.stdout,
                &retry_output.stderr,
            ) {
                eprintln!(
                    "dependency sync: forced pnpm reinstall left the dependency graph unresolved; removing generated node_modules and performing one clean frozen-lockfile reinstall"
                );
                let clean_install_output = clean_reinstall_pnpm_dependencies(step).await?;
                if !clean_install_output.status.success() {
                    return Err(AppError::Dependency(format!(
                        "{} step failed ({}) after forced pnpm reinstall: {}\n{}\nclean pnpm dependency reinstall also failed: {}\n{}\n{}",
                        step.phase,
                        step.strategy,
                        retry_output.status,
                        dependency_failure_output(&retry_output.stdout, &retry_output.stderr),
                        clean_install_output.status,
                        dependency_failure_output(&clean_install_output.stdout, &clean_install_output.stderr),
                        pnpm_runtime_summary(step).await
                    )));
                }

                let clean_retry_output =
                    output_command(&step.command, &step.args, Some(Path::new(&step.cwd))).await?;
                if clean_retry_output.status.success() {
                    continue;
                }
                return Err(AppError::Dependency(format!(
                    "{} step failed ({}) after clean pnpm dependency reinstall: {}\n{}\n{}",
                    step.phase,
                    step.strategy,
                    clean_retry_output.status,
                    dependency_failure_output(&clean_retry_output.stdout, &clean_retry_output.stderr),
                    pnpm_runtime_summary(step).await
                )));
            }

            return Err(AppError::Dependency(format!(
                "{} step failed ({}) after pnpm dependency self-heal: {}\n{}",
                step.phase,
                step.strategy,
                retry_output.status,
                String::from_utf8_lossy(&retry_output.stderr)
            )));
        }

        return Err(AppError::Dependency(format!(
            "{} step failed ({}): {}\n{}",
            step.phase,
            step.strategy,
            output.status,
            String::from_utf8_lossy(&output.stderr)
        )));
    }
    Ok(())
}

async fn force_reinstall_pnpm_dependencies(step: &DependencyStep) -> AppResult<Output> {
    let args = vec![
        "install".to_string(),
        "--frozen-lockfile".to_string(),
        "--force".to_string(),
    ];
    output_command(&step.command, &args, Some(Path::new(&step.cwd)))
        .await
        .map_err(AppError::from)
}

async fn clean_reinstall_pnpm_dependencies(step: &DependencyStep) -> AppResult<Output> {
    let cwd = Path::new(&step.cwd);
    remove_frontend_node_modules(cwd).await?;
    let args = vec!["install".to_string(), "--frozen-lockfile".to_string()];
    output_command(&step.command, &args, Some(cwd))
        .await
        .map_err(AppError::from)
}

async fn remove_frontend_node_modules(cwd: &Path) -> AppResult<()> {
    if parse_wsl_unc_path(cwd).is_some() {
        let args = vec![
            "-rf".to_string(),
            "--".to_string(),
            "node_modules".to_string(),
        ];
        let output = output_command("rm", &args, Some(cwd)).await?;
        if !output.status.success() {
            return Err(AppError::Dependency(format!(
                "failed to remove generated frontend node_modules in WSL: {}\n{}",
                output.status,
                String::from_utf8_lossy(&output.stderr)
            )));
        }
        return Ok(());
    }

    let node_modules = cwd.join("node_modules");
    if !node_modules.exists() {
        return Ok(());
    }

    let mut last_error = None;
    for attempt in 0..3 {
        match std::fs::remove_dir_all(&node_modules) {
            Ok(()) => return Ok(()),
            Err(error) => {
                last_error = Some(error);
                if attempt < 2 {
                    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                }
            }
        }
    }

    Err(AppError::Dependency(format!(
        "failed to remove generated frontend node_modules before clean pnpm reinstall: {}",
        last_error
            .map(|error| error.to_string())
            .unwrap_or_else(|| "unknown removal error".to_string())
    )))
}

async fn command_version(command: &str, cwd: &Path) -> String {
    let args = vec!["--version".to_string()];
    match output_command(command, &args, Some(cwd)).await {
        Ok(output) if output.status.success() => {
            let version = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if version.is_empty() {
                "<empty>".to_string()
            } else {
                version
            }
        }
        Ok(output) => format!("<failed: {}>", output.status),
        Err(error) => format!("<unavailable: {error}>"),
    }
}

async fn pnpm_runtime_summary(step: &DependencyStep) -> String {
    let cwd = Path::new(&step.cwd);
    let package_manager = read_package_json(cwd)
        .ok()
        .and_then(|package_json| {
            package_json
                .get("packageManager")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .unwrap_or_else(|| "<not declared>".to_string());
    let node = command_version("node", cwd).await;
    let pnpm = command_version("pnpm", cwd).await;
    format!(
        "frontend toolchain: node={node}; pnpm={pnpm}; packageManager={package_manager}"
    )
}

fn should_retry_pnpm_build_after_forced_install(
    step: &DependencyStep,
    output: &Output,
) -> bool {
    step.command == "pnpm"
        && step.phase == "build"
        && step.strategy == "pnpm_build"
        && !output.status.success()
        && looks_like_broken_pnpm_dependency_graph(&output.stdout, &output.stderr)
}

fn looks_like_broken_pnpm_dependency_graph(stdout: &[u8], stderr: &[u8]) -> bool {
    let stdout = String::from_utf8_lossy(stdout);
    let stderr = String::from_utf8_lossy(stderr);
    format!("{stderr}\n{stdout}").lines().any(|line| {
        let line = line.to_lowercase().replace('\\', "/");
        let reports_missing_module = line.contains("cannot find package")
            || line.contains("cannot find module")
            || line.contains("err_module_not_found");
        reports_missing_module && line.contains("node_modules/.pnpm/")
    })
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
    fn recognizes_missing_package_from_pnpm_dependency_graph() {
        let stderr = br#"[plugin unplugin-icons] Error: Cannot find package '@vue/compiler-sfc' imported from /home/toor/ComfyUI_frontend/node_modules/.pnpm/local-pkg@1.2.1/node_modules/local-pkg/dist/index.mjs
Error [ERR_MODULE_NOT_FOUND]: Cannot find package '@vue/compiler-sfc'"#;
        assert!(looks_like_broken_pnpm_dependency_graph(b"", stderr));
    }

    #[test]
    fn recognizes_windows_style_pnpm_dependency_graph_paths() {
        let stderr = br#"Error: Cannot find module 'dep' imported from C:\repo\node_modules\.pnpm\local-pkg@1.2.1\node_modules\local-pkg\dist\index.mjs"#;
        assert!(looks_like_broken_pnpm_dependency_graph(b"", stderr));
    }

    #[test]
    fn does_not_repair_source_level_missing_package_errors() {
        let stderr = br#"Error [ERR_MODULE_NOT_FOUND]: Cannot find package 'missing' imported from /repo/vite.config.mts
    at resolvePackage (/repo/node_modules/.pnpm/vite@8.2.2/node_modules/vite/dist/node/chunks/node.js:100:1)"#;
        assert!(!looks_like_broken_pnpm_dependency_graph(b"", stderr));
    }

    #[test]
    fn does_not_repair_unrelated_build_failures() {
        let stderr = b"src/App.tsx(10,4): error TS2322: Type 'string' is not assignable to type 'number'.";
        assert!(!looks_like_broken_pnpm_dependency_graph(b"", stderr));
    }

    #[test]
    fn failed_dependency_step_includes_stdout_only_compiler_diagnostics() {
        let message = dependency_failure_output(
            b"src/example.ts(12,4): error TS2322: Type 'string' is not assignable to type 'number'.",
            b"",
        );
        assert!(message.contains("stdout:\n"));
        assert!(message.contains("TS2322"));
    }

    #[test]
    fn failed_dependency_step_retains_both_output_streams() {
        let message = dependency_failure_output(b"error TS2345", b"TNB ACTIVE");
        assert!(message.contains("stderr:\nTNB ACTIVE"));
        assert!(message.contains("stdout:\nerror TS2345"));
    }

    #[test]
    fn failed_dependency_step_limits_large_diagnostic_output() {
        let huge_output = vec![b'x'; 48_000];
        let message = dependency_failure_output(&huge_output, b"");
        assert!(message.contains("[earlier output truncated]"));
        assert!(message.len() < 25_000);
    }

    #[tokio::test]
    async fn clean_pnpm_reinstall_removes_generated_node_modules() {
        let root = std::env::temp_dir().join(format!(
            "comfyui-patcher-pnpm-clean-{}",
            uuid::Uuid::new_v4()
        ));
        let nested = root.join("node_modules").join(".pnpm").join("broken");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(nested.join("marker"), b"broken").unwrap();

        remove_frontend_node_modules(&root).await.unwrap();

        assert!(!root.join("node_modules").exists());
        let _ = std::fs::remove_dir_all(root);
    }

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
