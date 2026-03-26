import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type {
  InstallationDetail,
  ListManagerCustomNodesInput,
  ManagerRegistryCustomNode,
  OperationEvent,
  OperationRecord,
  OperationStart,
  PatchCoreInput,
  PatchCustomNodeInput,
  RegisterInstallationInput,
  RegisterInstallationResult,
  RepoCheckpoint,
  ResolveTargetInput,
  ResolvedTarget,
  RestartInstallationInput,
  RollbackRepoInput,
  UpdateAllInput,
  UpdateRepoInput,
  Installation
} from "./types";

export const api = {
  listInstallations: () => invoke<Installation[]>("list_installations"),
  registerInstallation: (input: RegisterInstallationInput) =>
    invoke<RegisterInstallationResult>("register_installation", { input }),
  getInstallationDetail: (installationId: string) =>
    invoke<InstallationDetail>("get_installation_detail", { installationId }),
  listManagerCustomNodes: (input: ListManagerCustomNodesInput) =>
    invoke<ManagerRegistryCustomNode[]>("list_manager_custom_nodes", { input }),
  resolveTarget: (input: ResolveTargetInput) =>
    invoke<ResolvedTarget>("resolve_target", { input }),
  patchCore: (input: PatchCoreInput) =>
    invoke<OperationStart>("patch_core", { input }),
  installOrPatchCustomNode: (input: PatchCustomNodeInput) =>
    invoke<OperationStart>("install_or_patch_custom_node", { input }),
  updateRepo: (input: UpdateRepoInput) =>
    invoke<OperationStart>("update_repo", { input }),
  updateAll: (input: UpdateAllInput) =>
    invoke<OperationStart>("update_all", { input }),
  rollbackRepo: (input: RollbackRepoInput) =>
    invoke<OperationStart>("rollback_repo", { input }),
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
