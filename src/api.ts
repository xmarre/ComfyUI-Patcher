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
  RestartInstallationInput,
  StartInstallationInput,
  StopInstallationInput,
  RollbackRepoInput,
  UpdateAllInput,
  UpdateRepoInput,
  Installation
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
  updateAll: (input: UpdateAllInput) =>
    invoke<OperationStart>("update_all", { input }),
  rollbackRepo: (input: RollbackRepoInput) =>
    invoke<OperationStart>("rollback_repo", { input }),
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
  subscribeOperationEvents: async (handler: (event: OperationEvent) => void): Promise<UnlistenFn> =>
    listen<OperationEvent>("operation-event", (event) => handler(event.payload))
};
