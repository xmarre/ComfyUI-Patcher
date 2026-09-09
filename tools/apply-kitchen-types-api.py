from pathlib import Path


def replace_once(path: Path, old: str, new: str, label: str) -> None:
    text = path.read_text()
    count = text.count(old)
    if count != 1:
        raise SystemExit(f"{label}: expected exactly one match, found {count}")
    path.write_text(text.replace(old, new, 1))


types = Path("src/types.ts")
replace_once(
    types,
    'export type RepoKind = "core" | "frontend" | "custom_node";\n',
    'export type RepoKind = "core" | "frontend" | "kitchen" | "custom_node";\n',
    "RepoKind Kitchen",
)
replace_once(
    types,
    '''  | "install_frontend"\n  | "patch_frontend"\n  | "install_custom_node"\n''',
    '''  | "install_frontend"\n  | "patch_frontend"\n  | "install_kitchen"\n  | "patch_kitchen"\n  | "restore_comfy_managed_kitchen"\n  | "install_custom_node"\n''',
    "Kitchen operation kinds",
)
replace_once(
    types,
    '''export type RepoDependencyState = {\n  plan: DependencyPlan | null;\n  error: string | null;\n  manifestFiles: string[];\n  relevantChangedFiles: string[];\n};\n\n''',
    '''export type RepoDependencyState = {\n  plan: DependencyPlan | null;\n  error: string | null;\n  manifestFiles: string[];\n  relevantChangedFiles: string[];\n};\n\nexport type MaterializationStatus =\n  | "current"\n  | "stale"\n  | "missing"\n  | "import_failed"\n  | "replaced"\n  | "failed";\n\nexport type KitchenRuntimeProbe = {\n  distributionPresent: boolean;\n  installedVersion: string | null;\n  distributionLocation: string | null;\n  directUrl: string | null;\n  directUrlSha256: string | null;\n  recordSha256: string | null;\n  importOk: boolean;\n  importError: string | null;\n  moduleLocation: string | null;\n  probedAt: string | null;\n};\n\nexport type RepoMaterializationState = {\n  materializedHeadSha: string | null;\n  installedVersion: string | null;\n  installedOrigin: string | null;\n  artifactSha256: string | null;\n  installedRecordSha256: string | null;\n  status: MaterializationStatus;\n  lastMaterializedAt: string | null;\n  lastError: string | null;\n};\n\n''',
    "Kitchen runtime/materialization DTOs",
)
replace_once(
    types,
    '''  dependencyState: RepoDependencyState | null;\n  lastScannedAt: string | null;\n''',
    '''  dependencyState: RepoDependencyState | null;\n  materializationState: RepoMaterializationState | null;\n  lastScannedAt: string | null;\n''',
    "ManagedRepo materialization state",
)
replace_once(
    types,
    '''  coreRepo: ManagedRepo | null;\n  frontendRepo: ManagedRepo | null;\n  customNodeRepos: ManagedRepo[];\n''',
    '''  coreRepo: ManagedRepo | null;\n  frontendRepo: ManagedRepo | null;\n  kitchenRepo: ManagedRepo | null;\n  kitchenRuntime: KitchenRuntimeProbe | null;\n  customNodeRepos: ManagedRepo[];\n''',
    "InstallationDetail Kitchen",
)
replace_once(
    types,
    '''  dependencyState: RepoDependencyState | null;\n  createdAt: string;\n};\n\nexport type RepoActionPreviewCommit''',
    '''  dependencyState: RepoDependencyState | null;\n  materializationState: RepoMaterializationState | null;\n  createdAt: string;\n};\n\nexport type RepoActionPreviewCommit''',
    "Checkpoint materialization state",
)
replace_once(
    types,
    '''    | "dependency_sync"\n    | "state_refresh"\n''',
    '''    | "dependency_sync"\n    | "materialization"\n    | "state_refresh"\n''',
    "materialization event phase",
)
replace_once(
    types,
    '''  coreRepo: ManagedRepo | null;\n  frontendRepo: ManagedRepo | null;\n  discoveredCustomNodes: ManagedRepo[];\n''',
    '''  coreRepo: ManagedRepo | null;\n  frontendRepo: ManagedRepo | null;\n  kitchenRepo: ManagedRepo | null;\n  kitchenRuntime: KitchenRuntimeProbe | null;\n  discoveredCustomNodes: ManagedRepo[];\n''',
    "RegisterInstallationResult Kitchen",
)
replace_once(
    types,
    '''export type PatchCustomNodeInput = {\n''',
    '''export type PatchKitchenInput = {\n  installationId: string;\n  input: string;\n  existingRepoConflictStrategy: ExistingRepoConflictStrategy;\n  dirtyRepoStrategy: DirtyRepoStrategy;\n  setTrackedTarget: boolean;\n  restartAfterSuccess: boolean;\n};\n\nexport type RestoreComfyManagedKitchenInput = {\n  repoId: string;\n  restartAfterSuccess: boolean;\n};\n\nexport type PatchCustomNodeInput = {\n''',
    "Kitchen command input DTOs",
)

api = Path("src/api.ts")
replace_once(
    api,
    '''  PatchCoreInput,\n  PatchFrontendInput,\n  PatchCustomNodeInput,\n''',
    '''  PatchCoreInput,\n  PatchFrontendInput,\n  PatchKitchenInput,\n  RestoreComfyManagedKitchenInput,\n  PatchCustomNodeInput,\n''',
    "API Kitchen imports",
)
replace_once(
    api,
    '''  installOrPatchFrontend: (input: PatchFrontendInput) =>\n    invoke<OperationStart>("install_or_patch_frontend", { input }),\n  installOrPatchCustomNode: (input: PatchCustomNodeInput) =>\n''',
    '''  installOrPatchFrontend: (input: PatchFrontendInput) =>\n    invoke<OperationStart>("install_or_patch_frontend", { input }),\n  installOrPatchKitchen: (input: PatchKitchenInput) =>\n    invoke<OperationStart>("install_or_patch_kitchen", { input }),\n  restoreComfyManagedKitchen: (input: RestoreComfyManagedKitchenInput) =>\n    invoke<OperationStart>("restore_comfy_managed_kitchen", { input }),\n  installOrPatchCustomNode: (input: PatchCustomNodeInput) =>\n''',
    "API Kitchen commands",
)
