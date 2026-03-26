use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RepoKind {
    Core,
    CustomNode,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OperationStatus {
    Queued,
    Running,
    Succeeded,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OperationKind {
    PatchCore,
    InstallCustomNode,
    PatchCustomNode,
    UpdateRepo,
    UpdateAll,
    RollbackRepo,
    RestartInstallation,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DirtyRepoStrategy {
    Abort,
    Stash,
    HardReset,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExistingRepoConflictStrategy {
    Abort,
    Replace,
    InstallWithSuffix,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TargetKind {
    Branch,
    Tag,
    Commit,
    Pr,
    DefaultBranch,
    NamedRef,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LaunchProfile {
    pub mode: String,
    pub command: String,
    pub args: Vec<String>,
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
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallationDetail {
    pub installation: Installation,
    pub core_repo: Option<ManagedRepo>,
    pub custom_node_repos: Vec<ManagedRepo>,
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
    pub stash_created: bool,
    pub stash_ref: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RegisterInstallationInput {
    pub name: String,
    pub comfy_root: String,
    pub python_exe: Option<String>,
    pub launch_profile: Option<LaunchProfile>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RegisterInstallationResult {
    pub installation: Installation,
    pub core_repo: Option<ManagedRepo>,
    pub discovered_custom_nodes: Vec<ManagedRepo>,
    pub warnings: Vec<String>,
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
pub struct PatchCustomNodeInput {
    pub installation_id: String,
    pub input: String,
    pub target_local_dir_name: Option<String>,
    pub existing_repo_conflict_strategy: ExistingRepoConflictStrategy,
    pub dirty_repo_strategy: DirtyRepoStrategy,
    pub set_tracked_target: bool,
    pub sync_dependencies: bool,
    pub restart_after_success: bool,
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
pub struct UpdateAllInput {
    pub installation_id: String,
    pub dirty_repo_strategy: DirtyRepoStrategy,
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
