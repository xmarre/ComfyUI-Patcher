import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type {
  AddRepoOverlayInput,
  AdoptTrackedCustomNodesInput,
  InstallationDetail,
  ListManagerCustomNodesInput,
  ManagerRegistryCustomNode,
  MoveRepoOverlayInput,
  OperationEvent,
  OperationRecord,
  OperationStart,
  PatchCoreInput,
  PatchFrontendInput,
  PatchCustomNodeInput,
  PreviewRepoTargetInput,
  RepoActionPreview,
  RepoCheckpointComparison,
  RepoLifecycleInput,
  RemoveRepoOverlayInput,
  DeleteInstallationInput,
  RegisterInstallationInput,
  RegisterInstallationResult,
  SetRepoBaseTargetInput,
  SetRepoOverlayEnabledInput,
  SaveInstallationInput,
  RepoCheckpoint,
  ResolveTargetInput,
  ResolvedTarget,
  RestoreCheckpointInput,
  RestartInstallationInput,
  StartInstallationInput,
  StopInstallationInput,
  RollbackRepoInput,
  UpdateAllInput,
  UpdateRepoInput,
  Installation,
  AppUpdateCheckResult,
  AppUpdateEvent
} from "./types";

export const api = {
  listInstallations: () => invoke<Installation[]>("list_installations"),
  registerInstallation: (input: RegisterInstallationInput) =>
    invoke<RegisterInstallationResult>("register_installation", { input }),
  saveInstallation: (input: SaveInstallationInput) =>
    invoke<Installation>("save_installation", { input }),
  deleteInstallation: (input: DeleteInstallationInput) =>
    invoke<void>("delete_installation", { input }),
  getInstallationDetail: (installationId: string) =>
    invoke<InstallationDetail>("get_installation_detail", { installationId }),
  reconcileInstallation: (installationId: string) =>
    invoke<InstallationDetail>("reconcile_installation", { installationId }),
  listManagerCustomNodes: (input: ListManagerCustomNodesInput) =>
    invoke<ManagerRegistryCustomNode[]>("list_manager_custom_nodes", { input }),
  adoptTrackedCustomNodes: (input: AdoptTrackedCustomNodesInput) =>
    invoke<OperationStart>("adopt_tracked_custom_nodes", { input }),
  resolveTarget: (input: ResolveTargetInput) =>
    invoke<ResolvedTarget>("resolve_target", { input }),
  patchCore: (input: PatchCoreInput) =>
    invoke<OperationStart>("patch_core", { input }),
  installOrPatchFrontend: (input: PatchFrontendInput) =>
    invoke<OperationStart>("install_or_patch_frontend", { input }),
  installOrPatchCustomNode: (input: PatchCustomNodeInput) =>
    invoke<OperationStart>("install_or_patch_custom_node", { input }),
  setRepoBaseTarget: (input: SetRepoBaseTargetInput) =>
    invoke<OperationStart>("set_repo_base_target", { input }),
  addRepoOverlay: (input: AddRepoOverlayInput) =>
    invoke<OperationStart>("add_repo_overlay", { input }),
  setRepoOverlayEnabled: (input: SetRepoOverlayEnabledInput) =>
    invoke<OperationStart>("set_repo_overlay_enabled", { input }),
  removeRepoOverlay: (input: RemoveRepoOverlayInput) =>
    invoke<OperationStart>("remove_repo_overlay", { input }),
  moveRepoOverlay: (input: MoveRepoOverlayInput) =>
    invoke<OperationStart>("move_repo_overlay", { input }),
  updateRepo: (input: UpdateRepoInput) =>
    invoke<OperationStart>("update_repo", { input }),
  previewRepoTarget: (input: PreviewRepoTargetInput) =>
    invoke<RepoActionPreview>("preview_repo_target", { input }),
  previewTrackedRepoUpdate: (repoId: string) =>
    invoke<RepoActionPreview>("preview_tracked_repo_update", { repoId }),
  updateAll: (input: UpdateAllInput) =>
    invoke<OperationStart>("update_all", { input }),
  rollbackRepo: (input: RollbackRepoInput) =>
    invoke<OperationStart>("rollback_repo", { input }),
  restoreCheckpoint: (input: RestoreCheckpointInput) =>
    invoke<OperationStart>("restore_checkpoint", { input }),
  compareCheckpoint: (repoId: string, checkpointId: string) =>
    invoke<RepoCheckpointComparison>("compare_checkpoint", { repoId, checkpointId }),
  uninstallRepo: (input: RepoLifecycleInput) =>
    invoke<OperationStart>("uninstall_repo", { input }),
  disableRepo: (input: RepoLifecycleInput) =>
    invoke<OperationStart>("disable_repo", { input }),
  untrackRepo: (input: RepoLifecycleInput) =>
    invoke<OperationStart>("untrack_repo", { input }),
  startInstallation: (input: StartInstallationInput) =>
    invoke<OperationStart>("start_installation", { input }),
  stopInstallation: (input: StopInstallationInput) =>
    invoke<OperationStart>("stop_installation", { input }),
  restartInstallation: (input: RestartInstallationInput) =>
    invoke<OperationStart>("restart_installation", { input }),
  listOperations: (installationId?: string | null) =>
    invoke<OperationRecord[]>("list_operations", { installationId }),
  getOperationLog: (operationId: string) =>
    invoke<string>("get_operation_log", { operationId }),
  listCheckpoints: (repoId: string) =>
    invoke<RepoCheckpoint[]>("list_checkpoints", { repoId }),
  fetchAppUpdate: () => invoke<AppUpdateCheckResult>("fetch_app_update"),
  installAppUpdate: () => invoke<void>("install_app_update"),
  subscribeAppUpdateEvents: async (handler: (event: AppUpdateEvent) => void): Promise<UnlistenFn> =>
    listen<AppUpdateEvent>("app-update-event", (event) => handler(event.payload)),
  subscribeOperationEvents: async (handler: (event: OperationEvent) => void): Promise<UnlistenFn> =>
    listen<OperationEvent>("operation-event", (event) => handler(event.payload))
};
