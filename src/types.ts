export type DirtyRepoStrategy = "abort" | "stash" | "hard_reset";
export type ExistingRepoConflictStrategy = "abort" | "replace" | "install_with_suffix";
export type RepoKind = "core" | "custom_node";
export type OperationStatus = "queued" | "running" | "succeeded" | "failed";
export type OperationKind =
  | "patch_core"
  | "install_custom_node"
  | "patch_custom_node"
  | "update_repo"
  | "update_all"
  | "rollback_repo"
  | "restart_installation";

export type LaunchProfile = {
  mode: "managed_child" | "custom_command";
  command: string;
  args: string[];
  cwd?: string | null;
  env?: Record<string, string>;
  stopCommand?: string | null;
  stopArgs?: string[] | null;
  restartCommand?: string | null;
  restartArgs?: string[] | null;
};

export type Installation = {
  id: string;
  name: string;
  comfyRoot: string;
  pythonExe: string;
  customNodesDir: string;
  launchProfile: LaunchProfile | null;
  detectedEnvKind: "venv" | "conda" | "system" | "unknown";
  isGitRepo: boolean;
  createdAt: string;
  updatedAt: string;
};

export type ManagedRepo = {
  id: string;
  installationId: string;
  kind: RepoKind;
  displayName: string;
  localPath: string;
  canonicalRemote: string | null;
  currentHeadSha: string | null;
  currentBranch: string | null;
  isDetached: boolean;
  isDirty: boolean;
  trackedTargetKind: "branch" | "tag" | "commit" | "pr" | "default_branch" | "named_ref" | null;
  trackedTargetInput: string | null;
  trackedTargetResolvedSha: string | null;
  createdAt: string;
  updatedAt: string;
};

export type InstallationDetail = {
  installation: Installation;
  coreRepo: ManagedRepo | null;
  customNodeRepos: ManagedRepo[];
};

export type ResolvedTarget = {
  sourceInput: string;
  targetKind: "branch" | "tag" | "commit" | "pr" | "default_branch" | "named_ref";
  canonicalRepoUrl: string;
  fetchUrl: string;
  checkoutRef: string;
  resolvedSha: string | null;
  prNumber: number | null;
  prBaseRepoUrl: string | null;
  prHeadRepoUrl: string | null;
  prHeadRef: string | null;
  summaryLabel: string;
  suggestedLocalDirName: string;
};

export type OperationRecord = {
  id: string;
  installationId: string;
  repoId: string | null;
  kind: OperationKind;
  status: OperationStatus;
  requestedInput: string | null;
  logFile: string;
  errorMessage: string | null;
  checkpointId: string | null;
  createdAt: string;
  startedAt: string | null;
  finishedAt: string | null;
};

export type RepoCheckpoint = {
  id: string;
  repoId: string;
  operationId: string;
  oldHeadSha: string;
  oldBranch: string | null;
  oldIsDetached: boolean;
  stashCreated: boolean;
  stashRef: string | null;
  createdAt: string;
};

export type OperationEvent = {
  operationId: string;
  phase:
    | "queued"
    | "preflight"
    | "checkpoint"
    | "fetch"
    | "clone"
    | "checkout"
    | "reset"
    | "submodules"
    | "dependency_plan"
    | "dependency_sync"
    | "state_refresh"
    | "restart"
    | "done"
    | "error";
  level: "info" | "warn" | "error";
  message: string;
  timestamp: string;
};

export type RegisterInstallationInput = {
  name: string;
  comfyRoot: string;
  pythonExe?: string | null;
  launchProfile?: LaunchProfile | null;
};

export type RegisterInstallationResult = {
  installation: Installation;
  coreRepo: ManagedRepo | null;
  discoveredCustomNodes: ManagedRepo[];
  warnings: string[];
};

export type PatchCoreInput = {
  installationId: string;
  input: string;
  dirtyRepoStrategy: DirtyRepoStrategy;
  setTrackedTarget: boolean;
  syncDependencies: boolean;
  restartAfterSuccess: boolean;
};

export type PatchCustomNodeInput = {
  installationId: string;
  input: string;
  targetLocalDirName?: string | null;
  existingRepoConflictStrategy: ExistingRepoConflictStrategy;
  dirtyRepoStrategy: DirtyRepoStrategy;
  setTrackedTarget: boolean;
  syncDependencies: boolean;
  restartAfterSuccess: boolean;
};

export type UpdateRepoInput = {
  repoId: string;
  dirtyRepoStrategy: DirtyRepoStrategy;
  syncDependencies: boolean;
};

export type UpdateAllInput = {
  installationId: string;
  dirtyRepoStrategy: DirtyRepoStrategy;
  syncDependencies: boolean;
  restartAfterSuccess: boolean;
};

export type RollbackRepoInput = {
  repoId: string;
  restoreStash: boolean;
  syncDependencies: boolean;
  restartAfterSuccess: boolean;
};

export type RestartInstallationInput = {
  installationId: string;
};

export type ResolveTargetInput = {
  installationId: string;
  kind: RepoKind;
  input: string;
  repoId?: string | null;
};

export type OperationStart = {
  operationId: string;
};
