use crate::errors::{AppError, AppResult};
use crate::execution::output_command;
use crate::git::rev_parse;
use crate::models::{
    Installation, KitchenRuntimeProbe, ManagedRepo, MaterializationStatus, RepoKind,
    RepoMaterializationState,
};
use crate::util::now_rfc3339;
use std::path::{Path, PathBuf};

pub const DEFAULT_KITCHEN_REPO_URL: &str = "https://github.com/Comfy-Org/comfy-kitchen";
const KITCHEN_DISTRIBUTION_NAME: &str = "comfy-kitchen";

const KITCHEN_PROBE_SCRIPT: &str = r#"
import hashlib
import importlib
import importlib.metadata
import json

result = {
    "distributionPresent": False,
    "installedVersion": None,
    "distributionLocation": None,
    "directUrl": None,
    "directUrlSha256": None,
    "recordSha256": None,
    "importOk": False,
    "importError": None,
    "moduleLocation": None,
}
try:
    dist = importlib.metadata.distribution("comfy-kitchen")
    result["distributionPresent"] = True
    result["installedVersion"] = dist.version
    result["distributionLocation"] = str(dist.locate_file(""))
    direct_url_raw = dist.read_text("direct_url.json")
    if direct_url_raw:
        direct_url = json.loads(direct_url_raw)
        result["directUrl"] = direct_url.get("url")
        archive_info = direct_url.get("archive_info") or {}
        hashes = archive_info.get("hashes") or {}
        result["directUrlSha256"] = hashes.get("sha256")
        if not result["directUrlSha256"]:
            raw_hash = archive_info.get("hash")
            if isinstance(raw_hash, str) and raw_hash.startswith("sha256="):
                result["directUrlSha256"] = raw_hash.split("=", 1)[1]
    record = dist.read_text("RECORD")
    if record is not None:
        result["recordSha256"] = hashlib.sha256(record.encode("utf-8")).hexdigest()
except importlib.metadata.PackageNotFoundError:
    pass
except Exception as exc:
    result["importError"] = "distribution inspection failed: " + repr(exc)

try:
    module = importlib.import_module("comfy_kitchen")
    result["importOk"] = True
    result["moduleLocation"] = getattr(module, "__file__", None)
except Exception as exc:
    result["importError"] = repr(exc)

print(json.dumps(result, separators=(",", ":")))
"#;

const FILE_SHA256_SCRIPT: &str = r#"
import hashlib
import pathlib
import sys

path = pathlib.Path(sys.argv[1])
h = hashlib.sha256()
with path.open("rb") as handle:
    for chunk in iter(lambda: handle.read(1024 * 1024), b""):
        h.update(chunk)
print(h.hexdigest())
"#;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KitchenBuildPlan {
    pub python_exe: String,
    pub cwd: String,
    pub wheel_dir_arg: String,
    pub build_args: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KitchenInstallPlan {
    pub python_exe: String,
    pub cwd: String,
    pub wheel_arg: String,
    pub install_args: Vec<String>,
}

pub fn default_kitchen_path(installation: &Installation) -> PathBuf {
    let comfy_root = Path::new(&installation.comfy_root);
    comfy_root
        .parent()
        .unwrap_or(comfy_root)
        .join("comfy-kitchen")
}

pub fn plan_kitchen_wheel_build(
    installation: &Installation,
    repo_path: &Path,
    wheel_dir_arg: &str,
) -> KitchenBuildPlan {
    KitchenBuildPlan {
        python_exe: installation.python_exe.clone(),
        cwd: repo_path.to_string_lossy().to_string(),
        wheel_dir_arg: wheel_dir_arg.to_string(),
        build_args: vec![
            "-m".to_string(),
            "pip".to_string(),
            "wheel".to_string(),
            "--no-deps".to_string(),
            "--wheel-dir".to_string(),
            wheel_dir_arg.to_string(),
            ".".to_string(),
        ],
    }
}

pub fn plan_kitchen_wheel_install(
    installation: &Installation,
    repo_path: &Path,
    wheel_arg: &str,
) -> KitchenInstallPlan {
    KitchenInstallPlan {
        python_exe: installation.python_exe.clone(),
        cwd: repo_path.to_string_lossy().to_string(),
        wheel_arg: wheel_arg.to_string(),
        install_args: vec![
            "-m".to_string(),
            "pip".to_string(),
            "install".to_string(),
            "--no-deps".to_string(),
            "--force-reinstall".to_string(),
            wheel_arg.to_string(),
        ],
    }
}

pub async fn probe_kitchen_runtime(installation: &Installation) -> AppResult<KitchenRuntimeProbe> {
    let args = vec!["-c".to_string(), KITCHEN_PROBE_SCRIPT.to_string()];
    let output = output_command(
        &installation.python_exe,
        &args,
        Some(Path::new(&installation.comfy_root)),
    )
    .await
    .map_err(|error| {
        AppError::Dependency(format!(
            "failed to inspect {KITCHEN_DISTRIBUTION_NAME} with managed Python '{}': {error}",
            installation.python_exe
        ))
    })?;
    if !output.status.success() {
        return Err(AppError::Dependency(format!(
            "failed to inspect {KITCHEN_DISTRIBUTION_NAME} with managed Python '{}': {}\n{}",
            installation.python_exe,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let payload = stdout
        .lines()
        .rev()
        .find(|line| !line.trim().is_empty())
        .ok_or_else(|| {
            AppError::Dependency("Kitchen runtime probe returned no JSON".to_string())
        })?;
    let mut probe: KitchenRuntimeProbe = serde_json::from_str(payload).map_err(|error| {
        AppError::Dependency(format!(
            "Kitchen runtime probe returned invalid JSON: {error}; output: {stdout}"
        ))
    })?;
    probe.probed_at = Some(now_rfc3339());
    Ok(probe)
}

fn command_failure(context: &str, output: &std::process::Output) -> AppError {
    AppError::Dependency(format!(
        "{context} failed ({}):\n{}\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    ))
}

fn one_built_wheel(build_dir: &Path) -> AppResult<PathBuf> {
    let mut wheels = std::fs::read_dir(build_dir)?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("whl"))
        })
        .collect::<Vec<_>>();
    wheels.sort();
    match wheels.len() {
        1 => Ok(wheels.remove(0)),
        0 => Err(AppError::Dependency(format!(
            "Kitchen wheel build succeeded but produced no wheel in {}",
            build_dir.display()
        ))),
        count => Err(AppError::Dependency(format!(
            "Kitchen wheel build produced {count} wheels in {}; expected exactly one",
            build_dir.display()
        ))),
    }
}

async fn file_sha256(
    installation: &Installation,
    repo_path: &Path,
    relative_path: &str,
) -> AppResult<String> {
    let args = vec![
        "-c".to_string(),
        FILE_SHA256_SCRIPT.to_string(),
        relative_path.to_string(),
    ];
    let output = output_command(&installation.python_exe, &args, Some(repo_path)).await?;
    if !output.status.success() {
        return Err(command_failure("hashing built Kitchen wheel", &output));
    }
    let digest = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if digest.len() != 64 || !digest.chars().all(|ch| ch.is_ascii_hexdigit()) {
        return Err(AppError::Dependency(format!(
            "managed Python returned an invalid Kitchen wheel SHA-256: {digest}"
        )));
    }
    Ok(digest.to_ascii_lowercase())
}

pub async fn materialize_kitchen_project(
    installation: &Installation,
    repo: &ManagedRepo,
) -> AppResult<RepoMaterializationState> {
    if !matches!(repo.kind, RepoKind::Kitchen) {
        return Err(AppError::InvalidInput(
            "Kitchen project materialization requires a kitchen repository".to_string(),
        ));
    }
    let repo_path = Path::new(&repo.local_path);
    let head_sha = rev_parse(repo_path, "HEAD").await?.ok_or_else(|| {
        AppError::Git(
            "cannot materialize Comfy Kitchen source: repository HEAD could not be resolved"
                .to_string(),
        )
    })?;
    let build_id = uuid::Uuid::new_v4().to_string();
    let build_root = repo_path.join(".patcher-build");
    let build_dir = build_root.join(&build_id);
    std::fs::create_dir_all(&build_dir)?;

    // Keep build artifacts inside the managed checkout and pass relative argv to the
    // managed Python. This remains valid for native Windows paths, WSL UNC paths, and
    // paths containing spaces because execution.rs preserves argv through wsl --exec.
    let wheel_dir_arg = format!(".patcher-build/{build_id}");
    let result = async {
        let build = plan_kitchen_wheel_build(installation, repo_path, &wheel_dir_arg);
        let output = output_command(&build.python_exe, &build.build_args, Some(repo_path)).await?;
        if !output.status.success() {
            return Err(command_failure(
                "building Comfy Kitchen source wheel before installation",
                &output,
            ));
        }

        let wheel = one_built_wheel(&build_dir)?;
        let wheel_name = wheel
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| AppError::Dependency("Kitchen wheel filename is not valid UTF-8".to_string()))?;
        let wheel_arg = format!("{wheel_dir_arg}/{wheel_name}");
        let artifact_sha256 = file_sha256(installation, repo_path, &wheel_arg).await?;

        let install = plan_kitchen_wheel_install(installation, repo_path, &wheel_arg);
        let output = output_command(&install.python_exe, &install.install_args, Some(repo_path)).await?;
        if !output.status.success() {
            return Err(command_failure("installing the built Comfy Kitchen wheel", &output));
        }

        let probe = probe_kitchen_runtime(installation).await?;
        if !probe.distribution_present {
            return Err(AppError::Dependency(
                "Comfy Kitchen wheel installation completed, but the managed Python environment has no comfy-kitchen distribution"
                    .to_string(),
            ));
        }
        if !probe.import_ok {
            return Err(AppError::Dependency(format!(
                "Comfy Kitchen wheel installation completed, but `import comfy_kitchen` failed: {}",
                probe.import_error.as_deref().unwrap_or("unknown import error")
            )));
        }
        if let Some(installed_archive_sha) = probe.direct_url_sha256.as_deref() {
            if !installed_archive_sha.eq_ignore_ascii_case(&artifact_sha256) {
                return Err(AppError::Dependency(format!(
                    "installed Comfy Kitchen wheel provenance does not match the wheel that Patcher built: built {artifact_sha256}, installed metadata {installed_archive_sha}"
                )));
            }
        }

        Ok(RepoMaterializationState {
            materialized_head_sha: Some(head_sha),
            installed_version: probe.installed_version.clone(),
            installed_origin: probe
                .direct_url
                .clone()
                .or_else(|| probe.distribution_location.clone()),
            artifact_sha256: Some(artifact_sha256),
            installed_record_sha256: probe.record_sha256.clone(),
            status: MaterializationStatus::Current,
            last_materialized_at: Some(now_rfc3339()),
            last_error: None,
        })
    }
    .await;

    let _ = std::fs::remove_dir_all(&build_dir);
    if build_root
        .read_dir()
        .ok()
        .is_some_and(|mut entries| entries.next().is_none())
    {
        let _ = std::fs::remove_dir(&build_root);
    }
    result
}

pub fn runtime_identity_matches(
    expected: &KitchenRuntimeProbe,
    actual: &KitchenRuntimeProbe,
) -> bool {
    expected.distribution_present == actual.distribution_present
        && expected.installed_version == actual.installed_version
        && expected.distribution_location == actual.distribution_location
        && expected.direct_url == actual.direct_url
        && expected.direct_url_sha256 == actual.direct_url_sha256
        && expected.record_sha256 == actual.record_sha256
        && expected.import_ok == actual.import_ok
        && expected.module_location == actual.module_location
}

pub fn evaluate_materialization(
    repo: &ManagedRepo,
    probe: &KitchenRuntimeProbe,
) -> RepoMaterializationState {
    let mut state = repo
        .materialization_state
        .clone()
        .unwrap_or(RepoMaterializationState {
            materialized_head_sha: None,
            installed_version: None,
            installed_origin: None,
            artifact_sha256: None,
            installed_record_sha256: None,
            status: MaterializationStatus::Stale,
            last_materialized_at: None,
            last_error: None,
        });

    let installed_artifact_mismatch = state
        .artifact_sha256
        .as_deref()
        .zip(probe.direct_url_sha256.as_deref())
        .is_some_and(|(expected, actual)| !expected.eq_ignore_ascii_case(actual));
    let installed_source_origin_mismatch = state
        .installed_origin
        .as_deref()
        .filter(|origin| origin.starts_with("file:"))
        .is_some_and(|expected| probe.direct_url.as_deref() != Some(expected));

    let status = if !probe.distribution_present {
        MaterializationStatus::Missing
    } else if !probe.import_ok {
        MaterializationStatus::ImportFailed
    } else if state.materialized_head_sha.is_none()
        || repo.current_head_sha.as_deref() != state.materialized_head_sha.as_deref()
    {
        MaterializationStatus::Stale
    } else if state
        .installed_record_sha256
        .as_deref()
        .zip(probe.record_sha256.as_deref())
        .is_some_and(|(expected, actual)| !expected.eq_ignore_ascii_case(actual))
        || state
            .installed_version
            .as_deref()
            .zip(probe.installed_version.as_deref())
            .is_some_and(|(expected, actual)| expected != actual)
        || installed_artifact_mismatch
        || installed_source_origin_mismatch
    {
        MaterializationStatus::Replaced
    } else if state.installed_record_sha256.is_some() && probe.record_sha256.is_none() {
        MaterializationStatus::Replaced
    } else {
        MaterializationStatus::Current
    };
    state.status = status;
    state
}

pub fn materialization_warning(state: &RepoMaterializationState) -> Option<String> {
    match state.status {
        MaterializationStatus::Current => None,
        MaterializationStatus::Stale => Some(format!(
            "Kitchen source checkout HEAD does not match the materialized source revision ({})",
            state.materialized_head_sha.as_deref().unwrap_or("none")
        )),
        MaterializationStatus::Missing => Some(
            "Patcher-managed Kitchen source is active, but comfy-kitchen is missing from the managed Python environment"
                .to_string(),
        ),
        MaterializationStatus::ImportFailed => Some(
            "Patcher-managed Kitchen is installed, but `import comfy_kitchen` fails in the managed Python environment"
                .to_string(),
        ),
        MaterializationStatus::Replaced => Some(
            "The installed comfy-kitchen distribution no longer matches Patcher's last materialized source build"
                .to_string(),
        ),
        MaterializationStatus::Failed => Some(
            state
                .last_error
                .clone()
                .unwrap_or_else(|| "The last Kitchen source materialization failed".to_string()),
        ),
    }
}

pub fn failed_materialization_state(
    previous: Option<&RepoMaterializationState>,
    error: &str,
) -> RepoMaterializationState {
    let mut state = previous.cloned().unwrap_or(RepoMaterializationState {
        materialized_head_sha: None,
        installed_version: None,
        installed_origin: None,
        artifact_sha256: None,
        installed_record_sha256: None,
        status: MaterializationStatus::Failed,
        last_materialized_at: None,
        last_error: None,
    });
    state.status = MaterializationStatus::Failed;
    state.last_error = Some(error.to_string());
    state
}

pub fn comfy_kitchen_requirement(installation: &Installation) -> AppResult<String> {
    let requirements_path = Path::new(&installation.comfy_root).join("requirements.txt");
    let content = std::fs::read_to_string(&requirements_path).map_err(|error| {
        AppError::Dependency(format!(
            "failed to read current ComfyUI requirements at {}: {error}",
            requirements_path.display()
        ))
    })?;
    parse_comfy_kitchen_requirement(&content).ok_or_else(|| {
        AppError::Dependency(format!(
            "current ComfyUI checkout does not declare a comfy-kitchen requirement in {}",
            requirements_path.display()
        ))
    })
}

fn parse_comfy_kitchen_requirement(content: &str) -> Option<String> {
    content.lines().find_map(|line| {
        let without_comment = line.split('#').next()?.trim();
        if without_comment.is_empty() || without_comment.starts_with('-') {
            return None;
        }
        let normalized = without_comment.to_ascii_lowercase().replace('_', "-");
        let package_end = normalized
            .find(|ch: char| matches!(ch, '<' | '>' | '=' | '!' | '~' | '[' | '@' | ';' | ' '))
            .unwrap_or(normalized.len());
        if normalized[..package_end].trim() == KITCHEN_DISTRIBUTION_NAME {
            Some(without_comment.to_string())
        } else {
            None
        }
    })
}

pub async fn restore_comfy_managed_kitchen(
    installation: &Installation,
) -> AppResult<KitchenRuntimeProbe> {
    let requirement = comfy_kitchen_requirement(installation)?;
    let args = vec![
        "-m".to_string(),
        "pip".to_string(),
        "install".to_string(),
        "--no-deps".to_string(),
        "--force-reinstall".to_string(),
        requirement.clone(),
    ];
    let output = output_command(
        &installation.python_exe,
        &args,
        Some(Path::new(&installation.comfy_root)),
    )
    .await?;
    if !output.status.success() {
        return Err(command_failure(
            &format!("restoring ComfyUI-managed Kitchen requirement `{requirement}`"),
            &output,
        ));
    }
    let probe = probe_kitchen_runtime(installation).await?;
    if !probe.distribution_present || !probe.import_ok {
        return Err(AppError::Dependency(format!(
            "restored ComfyUI Kitchen requirement `{requirement}`, but runtime verification failed: {}",
            probe
                .import_error
                .as_deref()
                .unwrap_or("distribution is missing")
        )));
    }
    Ok(probe)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{RepoLiveStatus, TargetKind};

    fn installation() -> Installation {
        Installation {
            id: "i".to_string(),
            name: "test".to_string(),
            comfy_root: "/opt/Comfy UI".to_string(),
            python_exe: "/opt/venv with spaces/bin/python".to_string(),
            custom_nodes_dir: "/opt/Comfy UI/custom_nodes".to_string(),
            launch_profile: None,
            frontend_settings: None,
            detected_env_kind: "venv".to_string(),
            is_git_repo: true,
            last_reconciled_at: None,
            created_at: "now".to_string(),
            updated_at: "now".to_string(),
        }
    }

    fn repo() -> ManagedRepo {
        ManagedRepo {
            id: "r".to_string(),
            installation_id: "i".to_string(),
            kind: RepoKind::Kitchen,
            display_name: "Comfy Kitchen".to_string(),
            local_path: "/opt/comfy-kitchen".to_string(),
            canonical_remote: Some(DEFAULT_KITCHEN_REPO_URL.to_string()),
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
            materialization_state: Some(RepoMaterializationState {
                materialized_head_sha: Some("abc".to_string()),
                installed_version: Some("0.2.33".to_string()),
                installed_origin: Some("file:///wheel.whl".to_string()),
                artifact_sha256: Some("a".repeat(64)),
                installed_record_sha256: Some("record-a".to_string()),
                status: MaterializationStatus::Current,
                last_materialized_at: Some("now".to_string()),
                last_error: None,
            }),
            last_scanned_at: None,
            created_at: "now".to_string(),
            updated_at: "now".to_string(),
        }
    }

    #[test]
    fn build_plan_uses_exact_installation_python_and_builds_before_install() {
        let installation = installation();
        let build = plan_kitchen_wheel_build(
            &installation,
            Path::new("/opt/comfy-kitchen"),
            ".patcher-build/test",
        );
        assert_eq!(build.python_exe, installation.python_exe);
        assert_eq!(
            build.build_args,
            vec![
                "-m",
                "pip",
                "wheel",
                "--no-deps",
                "--wheel-dir",
                ".patcher-build/test",
                "."
            ]
        );
        let install = plan_kitchen_wheel_install(
            &installation,
            Path::new("/opt/comfy-kitchen"),
            ".patcher-build/test/comfy_kitchen.whl",
        );
        assert_eq!(install.python_exe, installation.python_exe);
        assert_eq!(
            install.install_args,
            vec![
                "-m",
                "pip",
                "install",
                "--no-deps",
                "--force-reinstall",
                ".patcher-build/test/comfy_kitchen.whl"
            ]
        );
    }

    #[test]
    fn requirement_parser_reads_current_core_pin_without_hard_coding_version() {
        assert_eq!(
            parse_comfy_kitchen_requirement(
                "torch\ncomfy-kitchen==9.8.7  # core-owned pin\nother-package\n"
            )
            .as_deref(),
            Some("comfy-kitchen==9.8.7")
        );
        assert_eq!(
            parse_comfy_kitchen_requirement("comfy_kitchen @ https://example.test/kitchen.whl\n")
                .as_deref(),
            Some("comfy_kitchen @ https://example.test/kitchen.whl")
        );
    }

    #[test]
    fn runtime_identity_ignores_probe_time_but_detects_distribution_changes() {
        let baseline = KitchenRuntimeProbe {
            distribution_present: true,
            installed_version: Some("0.2.33".to_string()),
            distribution_location: Some("/venv/site-packages".to_string()),
            direct_url: Some("file:///old.whl".to_string()),
            direct_url_sha256: Some("a".repeat(64)),
            record_sha256: Some("record-a".to_string()),
            import_ok: true,
            module_location: Some("/venv/site-packages/comfy_kitchen/__init__.py".to_string()),
            probed_at: Some("before".to_string()),
            ..Default::default()
        };
        let reprobe = KitchenRuntimeProbe {
            probed_at: Some("after".to_string()),
            ..baseline.clone()
        };
        assert!(runtime_identity_matches(&baseline, &reprobe));

        let replaced = KitchenRuntimeProbe {
            direct_url: Some("file:///new.whl".to_string()),
            direct_url_sha256: Some("b".repeat(64)),
            record_sha256: Some("record-b".to_string()),
            ..reprobe
        };
        assert!(!runtime_identity_matches(&baseline, &replaced));

        let missing_before = KitchenRuntimeProbe {
            probed_at: Some("before".to_string()),
            ..Default::default()
        };
        let missing_after = KitchenRuntimeProbe {
            probed_at: Some("after".to_string()),
            ..Default::default()
        };
        assert!(runtime_identity_matches(&missing_before, &missing_after));
    }

    #[test]
    fn reconciliation_distinguishes_checkout_staleness_and_external_replacement() {
        let mut repo = repo();
        let probe = KitchenRuntimeProbe {
            distribution_present: true,
            installed_version: Some("0.2.33".to_string()),
            direct_url: Some("file:///wheel.whl".to_string()),
            direct_url_sha256: Some("a".repeat(64)),
            record_sha256: Some("record-a".to_string()),
            import_ok: true,
            ..Default::default()
        };
        assert_eq!(
            evaluate_materialization(&repo, &probe).status,
            MaterializationStatus::Current
        );

        repo.current_head_sha = Some("def".to_string());
        assert_eq!(
            evaluate_materialization(&repo, &probe).status,
            MaterializationStatus::Stale
        );

        repo.current_head_sha = Some("abc".to_string());
        let replaced_record = KitchenRuntimeProbe {
            record_sha256: Some("record-b".to_string()),
            ..probe.clone()
        };
        assert_eq!(
            evaluate_materialization(&repo, &replaced_record).status,
            MaterializationStatus::Replaced
        );

        let replaced_artifact = KitchenRuntimeProbe {
            direct_url_sha256: Some("b".repeat(64)),
            ..probe.clone()
        };
        assert_eq!(
            evaluate_materialization(&repo, &replaced_artifact).status,
            MaterializationStatus::Replaced
        );

        let replaced_origin = KitchenRuntimeProbe {
            direct_url: None,
            direct_url_sha256: None,
            ..probe.clone()
        };
        assert_eq!(
            evaluate_materialization(&repo, &replaced_origin).status,
            MaterializationStatus::Replaced
        );

        let mut fallback_repo = repo.clone();
        fallback_repo
            .materialization_state
            .as_mut()
            .unwrap()
            .installed_origin = Some("/venv/site-packages".to_string());
        let fallback_probe = KitchenRuntimeProbe {
            direct_url: None,
            direct_url_sha256: None,
            ..probe
        };
        assert_eq!(
            evaluate_materialization(&fallback_repo, &fallback_probe).status,
            MaterializationStatus::Current
        );
    }

    #[test]
    fn reconciliation_distinguishes_missing_and_import_failure() {
        let repo = repo();
        let missing = KitchenRuntimeProbe::default();
        assert_eq!(
            evaluate_materialization(&repo, &missing).status,
            MaterializationStatus::Missing
        );
        let broken = KitchenRuntimeProbe {
            distribution_present: true,
            installed_version: Some("0.2.33".to_string()),
            import_ok: false,
            import_error: Some("boom".to_string()),
            ..Default::default()
        };
        assert_eq!(
            evaluate_materialization(&repo, &broken).status,
            MaterializationStatus::ImportFailed
        );
    }
}
