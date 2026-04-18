use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RepoKind {
    Core,
    Frontend,
    CustomNode,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OperationStatus {
    Queued,
    Running,
    Succeeded,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OperationKind {
    PatchCore,
    InstallFrontend,
    PatchFrontend,
    InstallCustomNode,
    PatchCustomNode,
    ManageRepoStack,
    RematerializeTrackedRepos,
    UninstallRepo,
    DisableRepo,
    UntrackRepo,
    UpdateRepo,
    UpdateAll,
    RollbackRepo,
    RestoreCheckpoint,
    StartInstallation,
    StopInstallation,
    RestartInstallation,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DirtyRepoStrategy {
    Abort,
    Stash,
    HardReset,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ExistingRepoConflictStrategy {
    Abort,
    Replace,
    InstallWithSuffix,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TargetKind {
    Branch,
    Tag,
    Commit,
    Pr,
    DefaultBranch,
    NamedRef,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OverlayApplyStatus {
    Pending,
    Applied,
    Disabled,
    Conflict,
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrackedBaseTarget {
    pub source_input: String,
    pub target_kind: TargetKind,
    pub canonical_repo_url: String,
    pub checkout_ref: String,
    pub resolved_sha: Option<String>,
    pub summary_label: String,
}

fn default_overlay_enabled() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrackedPrOverlay {
    pub id: String,
    pub source_input: String,
    pub canonical_repo_url: String,
    pub pr_number: u64,
    pub pr_base_repo_url: String,
    pub pr_base_ref: String,
    pub pr_head_repo_url: Option<String>,
    pub pr_head_ref: Option<String>,
    pub resolved_sha: Option<String>,
    pub summary_label: String,
    pub position: usize,
    #[serde(default = "default_overlay_enabled")]
    pub enabled: bool,
    #[serde(default)]
    pub last_apply_status: Option<OverlayApplyStatus>,
    #[serde(default)]
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrackedRepoState {
    pub version: u32,
    pub base: TrackedBaseTarget,
    #[serde(default)]
    pub overlays: Vec<TrackedPrOverlay>,
    #[serde(default)]
    pub materialized_branch: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OverlayMoveDirection {
    Up,
    Down,
}


#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FrontendPackageManager {
    Auto,
    Npm,
    Pnpm,
    Yarn,
}

fn default_frontend_package_manager() -> FrontendPackageManager {
    FrontendPackageManager::Auto
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FrontendSettings {
    pub repo_root: String,
    pub dist_path: String,
    #[serde(default = "default_frontend_package_manager")]
    pub package_manager: FrontendPackageManager,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DependencyStep {
    pub phase: String,
    pub strategy: String,
    pub command: String,
    pub args: Vec<String>,
    pub cwd: String,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DependencyPlan {
    pub strategy: String,
    pub reason: String,
    pub steps: Vec<DependencyStep>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RepoLiveStatus {
    Clean,
    Dirty,
    Drifted,
    Missing,
    NotGit,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RepoDependencyState {
    pub plan: Option<DependencyPlan>,
    pub error: Option<String>,
    pub manifest_files: Vec<String>,
    pub relevant_changed_files: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LaunchProfile {
    pub mode: String,
    pub command: String,
    pub args: Vec<String>,
    pub extra_args: Option<Vec<String>>,
    pub cwd: Option<String>,
    pub env: Option<std::collections::HashMap<String, String>>,
    pub stop_command: Option<String>,
    pub stop_args: Option<Vec<String>>,
    pub restart_command: Option<String>,
    pub restart_args: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Installation {
    pub id: String,
    pub name: String,
    pub comfy_root: String,
    pub python_exe: String,
    pub custom_nodes_dir: String,
    pub launch_profile: Option<LaunchProfile>,
    pub frontend_settings: Option<FrontendSettings>,
    pub detected_env_kind: String,
    pub is_git_repo: bool,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManagedRepo {
    pub id: String,
    pub installation_id: String,
    pub kind: RepoKind,
    pub display_name: String,
    pub local_path: String,
    pub canonical_remote: Option<String>,
    pub current_head_sha: Option<String>,
    pub current_branch: Option<String>,
    pub is_detached: bool,
    pub is_dirty: bool,
    pub tracked_target_kind: Option<TargetKind>,
    pub tracked_target_input: Option<String>,
    pub tracked_target_resolved_sha: Option<String>,
    #[serde(default)]
    pub tracked_state: Option<TrackedRepoState>,
    #[serde(default = "default_repo_live_status")]
    pub live_status: RepoLiveStatus,
    #[serde(default)]
    pub live_warnings: Vec<String>,
    #[serde(default)]
    pub changed_files: Vec<String>,
    #[serde(default)]
    pub dependency_state: Option<RepoDependencyState>,
    #[serde(default)]
    pub last_scanned_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

fn default_repo_live_status() -> RepoLiveStatus {
    RepoLiveStatus::Clean
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallationDetail {
    pub installation: Installation,
    pub core_repo: Option<ManagedRepo>,
    pub frontend_repo: Option<ManagedRepo>,
    pub custom_node_repos: Vec<ManagedRepo>,
    #[serde(default)]
    pub warnings: Vec<String>,
    #[serde(default)]
    pub last_reconciled_at: Option<String>,
    pub is_running: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolvedTarget {
    pub source_input: String,
    pub target_kind: TargetKind,
    pub canonical_repo_url: String,
    pub fetch_url: String,
    pub checkout_ref: String,
    pub resolved_sha: Option<String>,
    pub pr_number: Option<u64>,
    pub pr_base_repo_url: Option<String>,
    pub pr_base_ref: Option<String>,
    pub pr_head_repo_url: Option<String>,
    pub pr_head_ref: Option<String>,
    pub summary_label: String,
    pub suggested_local_dir_name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OperationRecord {
    pub id: String,
    pub installation_id: String,
    pub repo_id: Option<String>,
    pub kind: OperationKind,
    pub status: OperationStatus,
    pub requested_input: Option<String>,
    pub log_file: String,
    pub error_message: Option<String>,
    pub checkpoint_id: Option<String>,
    pub created_at: String,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RepoCheckpoint {
    pub id: String,
    pub repo_id: String,
    pub operation_id: String,
    pub old_head_sha: String,
    pub old_branch: Option<String>,
    pub old_is_detached: bool,
    pub has_tracked_target_snapshot: bool,
    pub old_tracked_target_kind: Option<TargetKind>,
    pub old_tracked_target_input: Option<String>,
    pub old_tracked_target_resolved_sha: Option<String>,
    pub stash_created: bool,
    pub stash_ref: Option<String>,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub reason: Option<String>,
    #[serde(default)]
    pub dependency_state: Option<RepoDependencyState>,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RepoActionPreviewCommit {
    pub sha: String,
    pub subject: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RepoActionPreviewFile {
    pub path: String,
    pub status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RepoStackPreviewItem {
    pub kind: String,
    pub label: String,
    pub enabled: bool,
    pub status: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RepoActionPreview {
    pub action: String,
    pub repo_id: Option<String>,
    pub repo_display_name: String,
    pub current_head_sha: Option<String>,
    pub target_head_sha: Option<String>,
    pub target_summary: String,
    pub target_ref: Option<String>,
    pub commits: Vec<RepoActionPreviewCommit>,
    pub file_changes: Vec<RepoActionPreviewFile>,
    pub warnings: Vec<String>,
    pub conflict_files: Vec<String>,
    pub stack_preview: Vec<RepoStackPreviewItem>,
    pub dependency_state: Option<RepoDependencyState>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RepoCheckpointComparison {
    pub checkpoint: RepoCheckpoint,
    pub current_head_sha: Option<String>,
    pub commits: Vec<RepoActionPreviewCommit>,
    pub file_changes: Vec<RepoActionPreviewFile>,
    pub warnings: Vec<String>,
    pub current_dependency_state: Option<RepoDependencyState>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RegisterInstallationInput {
    pub name: String,
    pub comfy_root: String,
    pub python_exe: Option<String>,
    pub launch_profile: Option<LaunchProfile>,
    pub frontend_settings: Option<FrontendSettings>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RegisterInstallationResult {
    pub installation: Installation,
    pub core_repo: Option<ManagedRepo>,
    pub frontend_repo: Option<ManagedRepo>,
    pub discovered_custom_nodes: Vec<ManagedRepo>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveInstallationInput {
    pub installation_id: String,
    pub name: String,
    pub python_exe: Option<String>,
    pub launch_profile: Option<LaunchProfile>,
    pub frontend_settings: Option<FrontendSettings>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteInstallationInput {
    pub installation_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolveTargetInput {
    pub installation_id: String,
    pub kind: RepoKind,
    pub input: String,
    pub repo_id: Option<String>,
}


#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListManagerCustomNodesInput {
    pub installation_id: String,
    pub query: Option<String>,
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManagerRegistryCustomNode {
    pub registry_id: String,
    pub title: String,
    pub author: Option<String>,
    pub description: Option<String>,
    pub install_type: String,
    pub source_input: Option<String>,
    pub canonical_repo_url: Option<String>,
    pub is_installable: bool,
    pub is_installed: bool,
    pub is_tracking_managed: bool,
    pub tracking_local_path: Option<String>,
    pub is_present_non_git: bool,
    pub present_local_path: Option<String>,
    pub has_ambiguous_installation: bool,
    pub installed_repo_id: Option<String>,
    pub installed_display_name: Option<String>,
    pub installed_local_path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PatchCoreInput {
    pub installation_id: String,
    pub input: String,
    pub dirty_repo_strategy: DirtyRepoStrategy,
    pub set_tracked_target: bool,
    pub sync_dependencies: bool,
    pub restart_after_success: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PatchFrontendInput {
    pub installation_id: String,
    pub input: String,
    pub existing_repo_conflict_strategy: ExistingRepoConflictStrategy,
    pub dirty_repo_strategy: DirtyRepoStrategy,
    pub set_tracked_target: bool,
    pub sync_dependencies: bool,
    pub restart_after_success: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PatchCustomNodeInput {
    pub installation_id: String,
    pub input: String,
    pub target_local_dir_name: Option<String>,
    pub existing_repo_conflict_strategy: ExistingRepoConflictStrategy,
    pub dirty_repo_strategy: DirtyRepoStrategy,
    pub set_tracked_target: bool,
    pub sync_dependencies: bool,
    pub restart_after_success: bool,
    #[serde(default)]
    pub adopt_tracking_install: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AdoptTrackedCustomNodesInput {
    pub installation_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateRepoInput {
    pub repo_id: String,
    pub dirty_repo_strategy: DirtyRepoStrategy,
    pub sync_dependencies: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RepoLifecycleInput {
    pub repo_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetRepoBaseTargetInput {
    pub repo_id: String,
    pub input: String,
    pub clear_overlays: bool,
    pub dirty_repo_strategy: DirtyRepoStrategy,
    pub sync_dependencies: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AddRepoOverlayInput {
    pub repo_id: String,
    pub input: String,
    pub dirty_repo_strategy: DirtyRepoStrategy,
    pub sync_dependencies: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetRepoOverlayEnabledInput {
    pub repo_id: String,
    pub overlay_id: String,
    pub enabled: bool,
    pub dirty_repo_strategy: DirtyRepoStrategy,
    pub sync_dependencies: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoveRepoOverlayInput {
    pub repo_id: String,
    pub overlay_id: String,
    pub dirty_repo_strategy: DirtyRepoStrategy,
    pub sync_dependencies: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MoveRepoOverlayInput {
    pub repo_id: String,
    pub overlay_id: String,
    pub direction: OverlayMoveDirection,
    pub dirty_repo_strategy: DirtyRepoStrategy,
    pub sync_dependencies: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateAllInput {
    pub installation_id: String,
    pub dirty_repo_strategy: DirtyRepoStrategy,
    pub sync_dependencies: bool,
    pub restart_after_success: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RematerializeTrackedReposInput {
    pub installation_id: String,
    pub sync_dependencies: bool,
    pub restart_after_success: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RollbackRepoInput {
    pub repo_id: String,
    pub restore_stash: bool,
    pub sync_dependencies: bool,
    pub restart_after_success: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RestoreCheckpointInput {
    pub repo_id: String,
    pub checkpoint_id: String,
    pub restore_stash: bool,
    pub sync_dependencies: bool,
    pub restart_after_success: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StartInstallationInput {
    pub installation_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StopInstallationInput {
    pub installation_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RestartInstallationInput {
    pub installation_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OperationStart {
    pub operation_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OperationEvent {
    pub operation_id: String,
    pub phase: String,
    pub level: String,
    pub message: String,
    pub timestamp: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewRepoTargetInput {
    pub installation_id: String,
    pub kind: RepoKind,
    pub input: String,
    pub repo_id: Option<String>,
    pub clear_overlays: Option<bool>,
}
