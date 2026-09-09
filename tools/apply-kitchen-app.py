from pathlib import Path

path = Path("src/App.tsx")
text = path.read_text()


def replace_once(old: str, new: str, label: str) -> None:
    global text
    count = text.count(old)
    if count != 1:
        raise SystemExit(f"{label}: expected exactly one match, found {count}")
    text = text.replace(old, new, 1)


replace_once(
    '''const defaultFrontendPackageManager: FrontendPackageManager = "auto";\nconst defaultDirtyRepoStrategy: DirtyRepoStrategy = "stash";\n''',
    '''const defaultFrontendPackageManager: FrontendPackageManager = "auto";\nconst defaultDirtyRepoStrategy: DirtyRepoStrategy = "stash";\nconst DEFAULT_KITCHEN_SOURCE = "https://github.com/Comfy-Org/comfy-kitchen";\n''',
    "Kitchen source constant",
)
replace_once(
    '''function repoNeedsTrackedRecovery(repo: ManagedRepo): boolean {\n  return (\n    repoHasTrackedState(repo) &&\n    (repo.liveStatus === "dirty" || repo.liveStatus === "drifted")\n  );\n}\n''',
    '''function repoNeedsTrackedRecovery(repo: ManagedRepo): boolean {\n  const materializationNeedsRecovery =\n    repo.kind === "kitchen" &&\n    repo.materializationState !== null &&\n    repo.materializationState.status !== "current";\n  return (\n    materializationNeedsRecovery ||\n    (repoHasTrackedState(repo) &&\n      (repo.liveStatus === "dirty" || repo.liveStatus === "drifted"))\n  );\n}\n''',
    "Kitchen materialization repair predicate",
)
replace_once(
    '''  const [frontendInput, setFrontendInput] = useState("");\n  const [nodeInput, setNodeInput] = useState("");\n''',
    '''  const [frontendInput, setFrontendInput] = useState("");\n  const [kitchenInput, setKitchenInput] = useState(DEFAULT_KITCHEN_SOURCE);\n  const [nodeInput, setNodeInput] = useState("");\n''',
    "Kitchen input state",
)
replace_once(
    '''  const [frontendActionPreview, setFrontendActionPreview] = useState<RepoActionPreview | null>(null);\n  const [nodeActionPreview, setNodeActionPreview] = useState<RepoActionPreview | null>(null);\n  const [corePreviewError, setCorePreviewError] = useState<string | null>(null);\n  const [frontendPreviewError, setFrontendPreviewError] = useState<string | null>(null);\n  const [nodePreviewError, setNodePreviewError] = useState<string | null>(null);\n''',
    '''  const [frontendActionPreview, setFrontendActionPreview] = useState<RepoActionPreview | null>(null);\n  const [kitchenActionPreview, setKitchenActionPreview] = useState<RepoActionPreview | null>(null);\n  const [nodeActionPreview, setNodeActionPreview] = useState<RepoActionPreview | null>(null);\n  const [corePreviewError, setCorePreviewError] = useState<string | null>(null);\n  const [frontendPreviewError, setFrontendPreviewError] = useState<string | null>(null);\n  const [kitchenPreviewError, setKitchenPreviewError] = useState<string | null>(null);\n  const [nodePreviewError, setNodePreviewError] = useState<string | null>(null);\n''',
    "Kitchen preview state",
)
replace_once(
    '''  const frontendPreviewRequestSeq = useRef(0);\n  const nodePreviewRequestSeq = useRef(0);\n''',
    '''  const frontendPreviewRequestSeq = useRef(0);\n  const kitchenPreviewRequestSeq = useRef(0);\n  const nodePreviewRequestSeq = useRef(0);\n''',
    "Kitchen preview sequence",
)
replace_once(
    '''  const frontendInputRef = useRef("");\n  const nodeInputRef = useRef("");\n''',
    '''  const frontendInputRef = useRef("");\n  const kitchenInputRef = useRef(DEFAULT_KITCHEN_SOURCE);\n  const nodeInputRef = useRef("");\n''',
    "Kitchen input ref",
)
replace_once(
    '''  useEffect(() => {\n    frontendInputRef.current = frontendInput;\n  }, [frontendInput]);\n\n  useEffect(() => {\n    nodeInputRef.current = nodeInput;\n''',
    '''  useEffect(() => {\n    frontendInputRef.current = frontendInput;\n  }, [frontendInput]);\n\n  useEffect(() => {\n    kitchenInputRef.current = kitchenInput;\n  }, [kitchenInput]);\n\n  useEffect(() => {\n    nodeInputRef.current = nodeInput;\n''',
    "Kitchen input ref effect",
)
replace_once(
    '''    setCoreActionPreview(null);\n    setFrontendActionPreview(null);\n    setNodeActionPreview(null);\n    setCorePreviewError(null);\n    setFrontendPreviewError(null);\n    setNodePreviewError(null);\n''',
    '''    setCoreActionPreview(null);\n    setFrontendActionPreview(null);\n    setKitchenActionPreview(null);\n    setNodeActionPreview(null);\n    setCorePreviewError(null);\n    setFrontendPreviewError(null);\n    setKitchenPreviewError(null);\n    setNodePreviewError(null);\n''',
    "selection Kitchen preview reset",
)
replace_once(
    '''  useEffect(() => {\n    setNodePreview(null);\n    setNodeActionPreview(null);\n    setNodePreviewError(null);\n    nodePreviewRequestSeq.current += 1;\n  }, [nodeInput]);\n\n  async function preview(input: ResolveTargetInput, target: "core" | "frontend" | "node") {\n''',
    '''  useEffect(() => {\n    setKitchenActionPreview(null);\n    setKitchenPreviewError(null);\n    kitchenPreviewRequestSeq.current += 1;\n  }, [kitchenInput]);\n\n  useEffect(() => {\n    setNodePreview(null);\n    setNodeActionPreview(null);\n    setNodePreviewError(null);\n    nodePreviewRequestSeq.current += 1;\n  }, [nodeInput]);\n\n  async function preview(input: ResolveTargetInput, target: "core" | "frontend" | "node") {\n''',
    "Kitchen input preview invalidation",
)

preview_fn = '''\n  async function previewKitchen() {\n    const installationId = selectedInstallationIdRef.current;\n    const input = kitchenInput.trim();\n    const requestSeq = ++kitchenPreviewRequestSeq.current;\n    if (!installationId || !input) {\n      setKitchenActionPreview(null);\n      setKitchenPreviewError(null);\n      return;\n    }\n    try {\n      const next = await api.previewRepoTarget({\n        installationId,\n        kind: "kitchen",\n        input,\n        repoId: kitchenRepo?.id ?? null\n      });\n      if (selectedInstallationIdRef.current !== installationId) return;\n      if (kitchenPreviewRequestSeq.current !== requestSeq) return;\n      if (kitchenInputRef.current.trim() !== input) return;\n      setKitchenActionPreview(next);\n      setKitchenPreviewError(null);\n    } catch (error) {\n      if (selectedInstallationIdRef.current !== installationId) return;\n      if (kitchenPreviewRequestSeq.current !== requestSeq) return;\n      if (kitchenInputRef.current.trim() !== input) return;\n      setKitchenActionPreview(null);\n      setKitchenPreviewError(toErrorMessage(error));\n    }\n  }\n\n'''
replace_once(
    '''  const coreRepo = detail?.coreRepo ?? null;\n  const frontendRepo = detail?.frontendRepo ?? null;\n  const customNodeRepos = detail?.customNodeRepos ?? [];\n  const allManagedRepos = [coreRepo, frontendRepo, ...customNodeRepos].filter(\n''',
    preview_fn + '''  const coreRepo = detail?.coreRepo ?? null;\n  const frontendRepo = detail?.frontendRepo ?? null;\n  const kitchenRepo = detail?.kitchenRepo ?? null;\n  const kitchenRuntime = detail?.kitchenRuntime ?? null;\n  const customNodeRepos = detail?.customNodeRepos ?? [];\n  const allManagedRepos = [coreRepo, frontendRepo, ...customNodeRepos, kitchenRepo].filter(\n''',
    "Kitchen managed repo collection",
)

# Clear Kitchen preview when re-registering an installation.
replace_once(
    '''                setCorePreview(null);\n                setFrontendPreview(null);\n                setNodePreview(null);\n                setCorePreviewError(null);\n                setFrontendPreviewError(null);\n                setNodePreviewError(null);\n                await refreshInstallations();\n''',
    '''                setCorePreview(null);\n                setFrontendPreview(null);\n                setKitchenActionPreview(null);\n                setNodePreview(null);\n                setCorePreviewError(null);\n                setFrontendPreviewError(null);\n                setKitchenPreviewError(null);\n                setNodePreviewError(null);\n                await refreshInstallations();\n''',
    "registration Kitchen preview reset",
)

kitchen_section = '''\n            <section className="card tab-panel" hidden={activeTab !== "patching"}>\n              <h3>Install or patch Comfy Kitchen from source</h3>\n              <div className="muted small">\n                Kitchen source management uses the official <code>Comfy-Org/comfy-kitchen</code> repository in a dedicated sibling checkout. Recursive submodules are mandatory, and Patcher builds a wheel from the selected source revision before installing it with this installation&apos;s exact Python environment. This project-materialization step is separate from generic Python dependency sync.\n              </div>\n              <div className="row gap">\n                <input\n                  className="grow"\n                  placeholder="Official repository URL, branch/tree URL, commit, or PR URL"\n                  value={kitchenInput}\n                  onChange={(e) => setKitchenInput(e.target.value)}\n                />\n                <button\n                  className="secondary"\n                  disabled={!kitchenInput.trim()}\n                  onClick={() => void previewKitchen()}\n                >\n                  Preview\n                </button>\n                <button\n                  disabled={!kitchenInput.trim()}\n                  onClick={() =>\n                    void runAction(async () => {\n                      await api.installOrPatchKitchen({\n                        installationId: selectedInstallation.id,\n                        input: kitchenInput,\n                        existingRepoConflictStrategy: "abort",\n                        dirtyRepoStrategy: defaultDirtyRepoStrategy,\n                        setTrackedTarget: true,\n                        restartAfterSuccess: false\n                      });\n                      setKitchenActionPreview(null);\n                      setKitchenPreviewError(null);\n                    })\n                  }\n                >\n                  Install / Patch source\n                </button>\n              </div>\n              {renderRepoActionPreview(kitchenActionPreview)}\n              {kitchenPreviewError ? <div className="muted">{kitchenPreviewError}</div> : null}\n\n              <div className="preview">\n                <div className="row between repo-preview-header">\n                  <div>\n                    <strong>Installed comfy-kitchen runtime</strong>\n                    <div className="muted small">Probed through the configured installation Python, independently of source checkout discovery.</div>\n                  </div>\n                  <span className={`badge ${kitchenRuntime?.importOk ? "ok" : kitchenRuntime?.distributionPresent ? "danger" : ""}`}>\n                    {kitchenRuntime?.importOk\n                      ? "import ok"\n                      : kitchenRuntime?.distributionPresent\n                        ? "import failed"\n                        : "not installed"}\n                  </span>\n                </div>\n                <div className="grid two compact-grid">\n                  <div>\n                    <div className="label">Installed version</div>\n                    <div className="mono small">{kitchenRuntime?.installedVersion ?? "none"}</div>\n                  </div>\n                  <div>\n                    <div className="label">Distribution location</div>\n                    <div className="mono small">{kitchenRuntime?.distributionLocation ?? "unknown"}</div>\n                  </div>\n                  <div>\n                    <div className="label">Imported module</div>\n                    <div className="mono small">{kitchenRuntime?.moduleLocation ?? "not imported"}</div>\n                  </div>\n                  <div>\n                    <div className="label">Installed RECORD SHA-256</div>\n                    <div className="mono small">{kitchenRuntime?.recordSha256 ?? "unknown"}</div>\n                  </div>\n                </div>\n                {kitchenRuntime?.importError ? (\n                  <div className="muted small">{kitchenRuntime.importError}</div>\n                ) : null}\n              </div>\n\n              {kitchenRepo ? (\n                <RepoCard\n                  key={kitchenRepo.id}\n                  repo={kitchenRepo}\n                  onUpdate={() =>\n                    runAction(async () => {\n                      await api.updateRepo({\n                        repoId: kitchenRepo.id,\n                        dirtyRepoStrategy: defaultDirtyRepoStrategy,\n                        syncDependencies: false\n                      });\n                    })\n                  }\n                  onSetBaseTarget={(input, clearOverlays) =>\n                    runActionOk(async () => {\n                      await api.setRepoBaseTarget({\n                        repoId: kitchenRepo.id,\n                        input,\n                        clearOverlays,\n                        dirtyRepoStrategy: defaultDirtyRepoStrategy,\n                        syncDependencies: false\n                      });\n                    })\n                  }\n                  onAddOverlay={(input) =>\n                    runActionOk(async () => {\n                      await api.addRepoOverlay({\n                        repoId: kitchenRepo.id,\n                        input,\n                        dirtyRepoStrategy: defaultDirtyRepoStrategy,\n                        syncDependencies: false\n                      });\n                    })\n                  }\n                  onSetOverlayEnabled={(overlayId, enabled) =>\n                    runActionOk(async () => {\n                      await api.setRepoOverlayEnabled({\n                        repoId: kitchenRepo.id,\n                        overlayId,\n                        enabled,\n                        dirtyRepoStrategy: defaultDirtyRepoStrategy,\n                        syncDependencies: false\n                      });\n                    })\n                  }\n                  onRemoveOverlay={(overlayId) =>\n                    runActionOk(async () => {\n                      await api.removeRepoOverlay({\n                        repoId: kitchenRepo.id,\n                        overlayId,\n                        dirtyRepoStrategy: defaultDirtyRepoStrategy,\n                        syncDependencies: false\n                      });\n                    })\n                  }\n                  onMoveOverlay={(overlayId, direction) =>\n                    runActionOk(async () => {\n                      await api.moveRepoOverlay({\n                        repoId: kitchenRepo.id,\n                        overlayId,\n                        direction,\n                        dirtyRepoStrategy: defaultDirtyRepoStrategy,\n                        syncDependencies: false\n                      });\n                    })\n                  }\n                  onRollback={() =>\n                    runAction(async () => {\n                      await api.rollbackRepo({\n                        repoId: kitchenRepo.id,\n                        restoreStash: true,\n                        syncDependencies: false,\n                        restartAfterSuccess: false\n                      });\n                    })\n                  }\n                />\n              ) : (\n                <div className="muted">\n                  No Kitchen source checkout is managed. An installed comfy-kitchen package remains ordinary ComfyUI/environment runtime state until source management is explicitly enabled here.\n                </div>\n              )}\n            </section>\n\n'''
replace_once(
    '''            <section className="card tab-panel" hidden={activeTab !== "patching"}>\n              <h3>Install or patch custom node manually</h3>\n''',
    kitchen_section + '''            <section className="card tab-panel" hidden={activeTab !== "patching"}>\n              <h3>Install or patch custom node manually</h3>\n''',
    "Kitchen patching panel",
)

path.write_text(text)
