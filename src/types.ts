export type DirtyRepoStrategy = "abort" | "stash" | "hard_reset";
export type ExistingRepoConflictStrategy = "abort" | "replace" | "install_with_suffix";
export type RepoKind = "core" | "frontend" | "custom_node";
export type OperationStatus = "queued" | "running" | "succeeded" | "failed";
export type OperationKind =
  | "patch_core"
  | "install_frontend"
  | "patch_frontend"
  | "install_custom_node"
  | "patch_custom_node"
  | "manage_repo_stack"
  | "uninstall_repo"
  | "disable_repo"
  | "untrack_repo"
  | "update_repo"
  | "update_all"
  | "rollback_repo"
  | "restore_checkpoint"
  | "start_installation"
  | "stop_installation"
  | "restart_installation";

export type OverlayApplyStatus = "pending" | "applied" | "disabled" | "conflict" | "error";
export type OverlayMoveDirection = "up" | "down";

export type FrontendPackageManager = "auto" | "npm" | "pnpm" | "yarn";

export type FrontendSettings = {
  repoRoot: string;
  distPath: string;
  packageManager: FrontendPackageManager;
};

export type DependencyStep = {
  phase: string;
  strategy: string;
  command: string;
  args: string[];
  cwd: string;
  reason: string;
};

export type DependencyPlan = {
  strategy: string;
  reason: string;
  steps: DependencyStep[];
};

export type RepoLiveStatus = "clean" | "dirty" | "drifted" | "missing" | "not_git";

export type RepoDependencyState = {
  plan: DependencyPlan | null;
  error: string | null;
  manifestFiles: string[];
  relevantChangedFiles: string[];
};

export type LaunchProfile = {
  mode: "managed_child" | "custom_command";
  command: string;
  args: string[];
  extraArgs?: string[] | null;
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
  frontendSettings: FrontendSettings | null;
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
  trackedState: TrackedRepoState | null;
  liveStatus: RepoLiveStatus;
  liveWarnings: string[];
  changedFiles: string[];
  dependencyState: RepoDependencyState | null;
  lastScannedAt: string | null;
  createdAt: string;
  updatedAt: string;
};

export type InstallationDetail = {
  installation: Installation;
  coreRepo: ManagedRepo | null;
  frontendRepo: ManagedRepo | null;
  customNodeRepos: ManagedRepo[];
  warnings: string[];
  lastReconciledAt: string | null;
  isRunning: boolean;
};


export type ManagerRegistryCustomNode = {
  registryId: string;
  title: string;
  author: string | null;
  description: string | null;
  installType: string;
  sourceInput: string | null;
  canonicalRepoUrl: string | null;
  isInstallable: boolean;
  isInstalled: boolean;
  isTrackingManaged: boolean;
  trackingLocalPath: string | null;
  isPresentNonGit: boolean;
  presentLocalPath: string | null;
  hasAmbiguousInstallation: boolean;
  installedRepoId: string | null;
  installedDisplayName: string | null;
  installedLocalPath: string | null;
};

export type ListManagerCustomNodesInput = {
  installationId: string;
  query?: string | null;
  limit?: number | null;
};

export type AdoptTrackedCustomNodesInput = {
  installationId: string;
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
  prBaseRef: string | null;
  prHeadRepoUrl: string | null;
  prHeadRef: string | null;
  summaryLabel: string;
  suggestedLocalDirName: string;
};

export type TrackedBaseTarget = {
  sourceInput: string;
  targetKind: "branch" | "tag" | "commit" | "pr" | "default_branch" | "named_ref";
  canonicalRepoUrl: string;
  checkoutRef: string;
  resolvedSha: string | null;
  summaryLabel: string;
};

export type TrackedPrOverlay = {
  id: string;
  sourceInput: string;
  canonicalRepoUrl: string;
  prNumber: number;
  prBaseRepoUrl: string;
  prBaseRef: string;
  prHeadRepoUrl: string | null;
  prHeadRef: string | null;
  resolvedSha: string | null;
  summaryLabel: string;
  position: number;
  enabled: boolean;
  lastApplyStatus: OverlayApplyStatus | null;
  lastError: string | null;
};

export type TrackedRepoState = {
  version: number;
  base: TrackedBaseTarget;
  overlays: TrackedPrOverlay[];
  materializedBranch: string | null;
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
  hasTrackedTargetSnapshot: boolean;
  stashCreated: boolean;
  stashRef: string | null;
  label: string | null;
  reason: string | null;
  dependencyState: RepoDependencyState | null;
  createdAt: string;
};

export type RepoActionPreviewCommit = {
  sha: string;
  subject: string;
};

export type RepoActionPreviewFile = {
  path: string;
  status: string;
};

export type RepoStackPreviewItem = {
  kind: string;
  label: string;
  enabled: boolean;
  status: string | null;
};

export type RepoActionPreview = {
  action: string;
  repoId: string | null;
  repoDisplayName: string;
  currentHeadSha: string | null;
  targetHeadSha: string | null;
  targetSummary: string;
  targetRef: string | null;
  commits: RepoActionPreviewCommit[];
  fileChanges: RepoActionPreviewFile[];
  warnings: string[];
  conflictFiles: string[];
  stackPreview: RepoStackPreviewItem[];
  dependencyState: RepoDependencyState | null;
};

export type RepoCheckpointComparison = {
  checkpoint: RepoCheckpoint;
  currentHeadSha: string | null;
  commits: RepoActionPreviewCommit[];
  fileChanges: RepoActionPreviewFile[];
  warnings: string[];
  currentDependencyState: RepoDependencyState | null;
};

export type OperationEvent = {
  operationId: string;
  phase:
    | "queued"
    | "preflight"
    | "checkpoint"
    | "stash"
    | "fetch"
    | "clone"
    | "checkout"
    | "reset"
    | "submodules"
    | "dependency_plan"
    | "dependency_sync"
    | "state_refresh"
    | "start"
    | "stop"
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
  frontendSettings?: FrontendSettings | null;
};

export type RegisterInstallationResult = {
  installation: Installation;
  coreRepo: ManagedRepo | null;
  frontendRepo: ManagedRepo | null;
  discoveredCustomNodes: ManagedRepo[];
  warnings: string[];
};

export type SaveInstallationInput = {
  installationId: string;
  name: string;
  pythonExe?: string | null;
  launchProfile?: LaunchProfile | null;
  frontendSettings?: FrontendSettings | null;
};

export type DeleteInstallationInput = {
  installationId: string;
};

export type PatchCoreInput = {
  installationId: string;
  input: string;
  dirtyRepoStrategy: DirtyRepoStrategy;
  setTrackedTarget: boolean;
  syncDependencies: boolean;
  restartAfterSuccess: boolean;
};

export type PatchFrontendInput = {
  installationId: string;
  input: string;
  existingRepoConflictStrategy: ExistingRepoConflictStrategy;
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
  adoptTrackingInstall?: boolean;
};

export type UpdateRepoInput = {
  repoId: string;
  dirtyRepoStrategy: DirtyRepoStrategy;
  syncDependencies: boolean;
};

export type RepoLifecycleInput = {
  repoId: string;
};

export type SetRepoBaseTargetInput = {
  repoId: string;
  input: string;
  clearOverlays: boolean;
  dirtyRepoStrategy: DirtyRepoStrategy;
  syncDependencies: boolean;
};

export type AddRepoOverlayInput = {
  repoId: string;
  input: string;
  dirtyRepoStrategy: DirtyRepoStrategy;
  syncDependencies: boolean;
};

export type SetRepoOverlayEnabledInput = {
  repoId: string;
  overlayId: string;
  enabled: boolean;
  dirtyRepoStrategy: DirtyRepoStrategy;
  syncDependencies: boolean;
};

export type RemoveRepoOverlayInput = {
  repoId: string;
  overlayId: string;
  dirtyRepoStrategy: DirtyRepoStrategy;
  syncDependencies: boolean;
};

export type MoveRepoOverlayInput = {
  repoId: string;
  overlayId: string;
  direction: OverlayMoveDirection;
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

export type RestoreCheckpointInput = {
  repoId: string;
  checkpointId: string;
  restoreStash: boolean;
  syncDependencies: boolean;
  restartAfterSuccess: boolean;
};

export type StartInstallationInput = {
  installationId: string;
};

export type StopInstallationInput = {
  installationId: string;
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

export type PreviewRepoTargetInput = {
  installationId: string;
  kind: RepoKind;
  input: string;
  repoId?: string | null;
  clearOverlays?: boolean | null;
};

export type OperationStart = {
  operationId: string;
};

export type AppUpdateMetadata = {
  version: string;
  currentVersion: string;
};

export type AppUpdateCheckResult = {
  enabled: boolean;
  disabledReason: string | null;
  update: AppUpdateMetadata | null;
};

export type AppUpdateEvent =
  | {
      kind: "started";
      contentLength: number | null;
    }
  | {
      kind: "progress";
      chunkLength: number;
    }
  | {
      kind: "finished";
    };
