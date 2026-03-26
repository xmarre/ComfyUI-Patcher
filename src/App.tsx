import { useEffect, useMemo, useState } from "react";
import { api } from "./api";
import RepoCard from "./components/RepoCard";
import OperationPanel from "./components/OperationPanel";
import type {
  Installation,
  InstallationDetail,
  LaunchProfile,
  ManagedRepo,
  OperationEvent,
  ResolveTargetInput,
  ResolvedTarget
} from "./types";

const defaultLaunchProfile: LaunchProfile = {
  mode: "managed_child",
  command: "python",
  args: ["main.py"],
  cwd: "",
  env: {}
};

export default function App() {
  const [installations, setInstallations] = useState<Installation[]>([]);
  const [selectedInstallationId, setSelectedInstallationId] = useState<string | null>(null);
  const [detail, setDetail] = useState<InstallationDetail | null>(null);
  const [coreInput, setCoreInput] = useState("");
  const [nodeInput, setNodeInput] = useState("");
  const [corePreview, setCorePreview] = useState<ResolvedTarget | null>(null);
  const [nodePreview, setNodePreview] = useState<ResolvedTarget | null>(null);
  const [events, setEvents] = useState<OperationEvent[]>([]);
  const [registerForm, setRegisterForm] = useState({
    name: "Primary ComfyUI",
    comfyRoot: "",
    pythonExe: "",
    launchCommand: defaultLaunchProfile.command,
    launchArgs: defaultLaunchProfile.args.join(" "),
    launchCwd: ""
  });

  const selectedInstallation = useMemo(
    () => installations.find((item) => item.id === selectedInstallationId) ?? null,
    [installations, selectedInstallationId]
  );

  async function refreshInstallations() {
    const next = await api.listInstallations();
    setInstallations(next);
    if (!selectedInstallationId && next.length) {
      setSelectedInstallationId(next[0].id);
    }
  }

  async function refreshDetail(installationId: string | null) {
    if (!installationId) {
      setDetail(null);
      return;
    }
    setDetail(await api.getInstallationDetail(installationId));
  }

  useEffect(() => {
    void refreshInstallations();
  }, []);

  useEffect(() => {
    void refreshDetail(selectedInstallationId);
  }, [selectedInstallationId]);

  useEffect(() => {
    let cancelled = false;
    let unlisten: (() => void) | null = null;
    api.subscribeOperationEvents((event) => {
      if (cancelled) return;
      setEvents((prev) => [event, ...prev].slice(0, 100));
      if (selectedInstallationId) {
        void refreshDetail(selectedInstallationId);
      }
      void refreshInstallations();
    }).then((fn) => {
      if (cancelled) {
        fn();
        return;
      }
      unlisten = fn;
    });
    return () => {
      cancelled = true;
      if (unlisten) unlisten();
    };
  }, [selectedInstallationId]);

  async function preview(input: ResolveTargetInput, target: "core" | "node") {
    if (!input.installationId || !input.input.trim()) return;
    const resolved = await api.resolveTarget(input);
    if (target === "core") setCorePreview(resolved);
    else setNodePreview(resolved);
  }

  const coreRepo = detail?.coreRepo ?? null;
  const customNodeRepos = detail?.customNodeRepos ?? [];

  return (
    <div className="app-shell">
      <aside className="sidebar">
        <h1>ComfyUI Patcher</h1>
        <div className="card">
          <h3>Installations</h3>
          <div className="list">
            {installations.map((installation) => (
              <button
                key={installation.id}
                className={`list-item ${installation.id === selectedInstallationId ? "active" : ""}`}
                onClick={() => setSelectedInstallationId(installation.id)}
              >
                <strong>{installation.name}</strong>
                <div className="mono small">{installation.comfyRoot}</div>
              </button>
            ))}
          </div>
        </div>

        <div className="card">
          <h3>Register installation</h3>
          <label>
            <span>Name</span>
            <input
              value={registerForm.name}
              onChange={(e) => setRegisterForm((v) => ({ ...v, name: e.target.value }))}
            />
          </label>
          <label>
            <span>ComfyUI root</span>
            <input
              placeholder="/path/to/ComfyUI"
              value={registerForm.comfyRoot}
              onChange={(e) => setRegisterForm((v) => ({ ...v, comfyRoot: e.target.value }))}
            />
          </label>
          <label>
            <span>Python executable</span>
            <input
              placeholder="Optional; auto-detected when blank"
              value={registerForm.pythonExe}
              onChange={(e) => setRegisterForm((v) => ({ ...v, pythonExe: e.target.value }))}
            />
          </label>
          <label>
            <span>Launch command</span>
            <input
              value={registerForm.launchCommand}
              onChange={(e) => setRegisterForm((v) => ({ ...v, launchCommand: e.target.value }))}
            />
          </label>
          <label>
            <span>Launch args</span>
            <input
              value={registerForm.launchArgs}
              onChange={(e) => setRegisterForm((v) => ({ ...v, launchArgs: e.target.value }))}
            />
          </label>
          <label>
            <span>Launch cwd</span>
            <input
              value={registerForm.launchCwd}
              onChange={(e) => setRegisterForm((v) => ({ ...v, launchCwd: e.target.value }))}
            />
          </label>
          <button
            onClick={async () => {
              const result = await api.registerInstallation({
                name: registerForm.name,
                comfyRoot: registerForm.comfyRoot,
                pythonExe: registerForm.pythonExe || null,
                launchProfile: {
                  mode: "managed_child",
                  command: registerForm.launchCommand,
                  args: registerForm.launchArgs.split(" ").filter(Boolean),
                  cwd: registerForm.launchCwd || registerForm.comfyRoot,
                  env: {}
                }
              });
              await refreshInstallations();
              setSelectedInstallationId(result.installation.id);
            }}
          >
            Register
          </button>
        </div>

        <div className="card fill">
          <h3>Live events</h3>
          <div className="event-stream">
            {events.map((event, index) => (
              <div key={`${event.operationId}-${event.timestamp}-${index}`} className={`event ${event.level}`}>
                <div className="row between">
                  <span className="mono small">{event.phase}</span>
                  <span className="small">{new Date(event.timestamp).toLocaleTimeString()}</span>
                </div>
                <div className="small">{event.message}</div>
              </div>
            ))}
          </div>
        </div>
      </aside>

      <main className="main">
        {selectedInstallation && detail ? (
          <>
            <section className="page-header card">
              <div className="row between">
                <div>
                  <h2>{selectedInstallation.name}</h2>
                  <div className="mono">{selectedInstallation.comfyRoot}</div>
                </div>
                <div className="row gap">
                  <button
                    onClick={async () => {
                      await api.updateAll({
                        installationId: selectedInstallation.id,
                        dirtyRepoStrategy: "abort",
                        syncDependencies: true,
                        restartAfterSuccess: false
                      });
                    }}
                  >
                    Update all
                  </button>
                  <button
                    className="secondary"
                    onClick={async () => {
                      await api.restartInstallation({ installationId: selectedInstallation.id });
                    }}
                  >
                    Restart
                  </button>
                </div>
              </div>
            </section>

            <section className="card">
              <h3>Patch core ComfyUI</h3>
              <div className="row gap">
                <input
                  className="grow"
                  placeholder="Branch name, commit SHA, GitHub tree URL, or PR URL"
                  value={coreInput}
                  onChange={(e) => setCoreInput(e.target.value)}
                />
                <button
                  className="secondary"
                  onClick={() =>
                    void preview(
                      {
                        installationId: selectedInstallation.id,
                        kind: "core",
                        input: coreInput,
                        repoId: coreRepo?.id ?? null
                      },
                      "core"
                    )
                  }
                >
                  Resolve
                </button>
                <button
                  onClick={async () => {
                    await api.patchCore({
                      installationId: selectedInstallation.id,
                      input: coreInput,
                      dirtyRepoStrategy: "abort",
                      setTrackedTarget: true,
                      syncDependencies: true,
                      restartAfterSuccess: false
                    });
                    setCoreInput("");
                  }}
                >
                  Apply
                </button>
              </div>
              {corePreview ? (
                <div className="preview">
                  <div><strong>{corePreview.summaryLabel}</strong></div>
                  <div className="mono small">{corePreview.canonicalRepoUrl}</div>
                  <div className="mono small">{corePreview.resolvedSha ?? corePreview.checkoutRef}</div>
                </div>
              ) : null}
              {coreRepo ? (
                <RepoCard
                  repo={coreRepo}
                  onRollback={async () => {
                    await api.rollbackRepo({
                      repoId: coreRepo.id,
                      restoreStash: true,
                      syncDependencies: true,
                      restartAfterSuccess: false
                    });
                  }}
                />
              ) : (
                <div className="muted">No managed core repo is registered for this installation.</div>
              )}
            </section>

            <section className="card">
              <h3>Install or patch custom node</h3>
              <div className="row gap">
                <input
                  className="grow"
                  placeholder="Repository URL, branch URL, PR URL"
                  value={nodeInput}
                  onChange={(e) => setNodeInput(e.target.value)}
                />
                <button
                  className="secondary"
                  onClick={() =>
                    void preview(
                      {
                        installationId: selectedInstallation.id,
                        kind: "custom_node",
                        input: nodeInput
                      },
                      "node"
                    )
                  }
                >
                  Resolve
                </button>
                <button
                  onClick={async () => {
                    await api.installOrPatchCustomNode({
                      installationId: selectedInstallation.id,
                      input: nodeInput,
                      existingRepoConflictStrategy: "install_with_suffix",
                      dirtyRepoStrategy: "abort",
                      setTrackedTarget: true,
                      syncDependencies: true,
                      restartAfterSuccess: false
                    });
                    setNodeInput("");
                  }}
                >
                  Install / Patch
                </button>
              </div>
              {nodePreview ? (
                <div className="preview">
                  <div><strong>{nodePreview.summaryLabel}</strong></div>
                  <div className="mono small">{nodePreview.canonicalRepoUrl}</div>
                  <div className="mono small">{nodePreview.suggestedLocalDirName}</div>
                </div>
              ) : null}
            </section>

            <section className="grid two">
              <div className="card">
                <h3>Managed custom nodes</h3>
                {customNodeRepos.length ? (
                  <div className="stack">
                    {customNodeRepos.map((repo: ManagedRepo) => (
                      <RepoCard
                        key={repo.id}
                        repo={repo}
                        onUpdate={async () => {
                          await api.updateRepo({
                            repoId: repo.id,
                            dirtyRepoStrategy: "abort",
                            syncDependencies: true
                          });
                        }}
                        onRollback={async () => {
                          await api.rollbackRepo({
                            repoId: repo.id,
                            restoreStash: true,
                            syncDependencies: true,
                            restartAfterSuccess: false
                          });
                        }}
                      />
                    ))}
                  </div>
                ) : (
                  <div className="muted">No managed custom node repositories were found.</div>
                )}
              </div>

              <OperationPanel installationId={selectedInstallationId} />
            </section>
          </>
        ) : (
          <section className="card">
            <h2>No installation selected</h2>
            <div className="muted">Register a ComfyUI root on the left to begin.</div>
          </section>
        )}
      </main>
    </div>
  );
}
