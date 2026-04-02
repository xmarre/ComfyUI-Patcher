import { useEffect, useMemo, useRef, useState } from "react";
import { api } from "./api";
import RepoCard from "./components/RepoCard";
import OperationPanel from "./components/OperationPanel";
import ManagerRegistryBrowser from "./components/ManagerRegistryBrowser";
import type {
  FrontendPackageManager,
  FrontendSettings,
  Installation,
  InstallationDetail,
  LaunchProfile,
  ManagedRepo,
  OperationEvent,
  ResolveTargetInput,
  ResolvedTarget,
  SaveInstallationInput
} from "./types";

const defaultLaunchProfile: LaunchProfile = {
  mode: "managed_child",
  command: "python",
  args: ["main.py"],
  cwd: "",
  env: {}
};

function parseLaunchArgs(value: string): string[] {
  const args: string[] = [];
  let current = "";
  let quote: '"' | "'" | null = null;
  let escape = false;
  let tokenStarted = false;

  for (const char of value) {
    if (escape) {
      current += char;
      escape = false;
      tokenStarted = true;
      continue;
    }

    if (quote === '"' && char === "\\") {
      escape = true;
      tokenStarted = true;
      continue;
    }

    if (quote) {
      if (char === quote) {
        quote = null;
        tokenStarted = true;
      } else {
        current += char;
        tokenStarted = true;
      }
      continue;
    }

    if (char === '"' || char === "'") {
      quote = char;
      tokenStarted = true;
      continue;
    }

    if (/\s/.test(char)) {
      if (tokenStarted) {
        args.push(current);
        current = "";
        tokenStarted = false;
      }
      continue;
    }

    current += char;
    tokenStarted = true;
  }

  if (escape) {
    current += "\\";
    tokenStarted = true;
  }
  if (tokenStarted) args.push(current);
  return args;
}

function parseOptionalArgs(value: string): string[] | null {
  const args = parseLaunchArgs(value);
  return args.length ? args : null;
}

function toErrorMessage(error: unknown): string {
  if (error instanceof Error) return error.message;
  return String(error);
}

const defaultFrontendPackageManager: FrontendPackageManager = "auto";

const frontendPackageManagerOptions: Array<{
  value: FrontendPackageManager;
  label: string;
}> = [
  { value: "auto", label: "Auto-detect" },
  { value: "npm", label: "npm" },
  { value: "pnpm", label: "pnpm" },
  { value: "yarn", label: "yarn" }
];

function buildFrontendSettingsPayload(form: {
  frontendRepoRoot: string;
  frontendDistPath: string;
  frontendPackageManager: FrontendPackageManager;
}): FrontendSettings | null {
  const repoRoot = form.frontendRepoRoot.trim();
  if (!repoRoot) {
    return null;
  }
  return {
    repoRoot,
    distPath: form.frontendDistPath.trim(),
    packageManager: form.frontendPackageManager
  };
}

export default function App() {
  const [installations, setInstallations] = useState<Installation[]>([]);
  const [selectedInstallationId, setSelectedInstallationId] = useState<string | null>(null);
  const [detail, setDetail] = useState<InstallationDetail | null>(null);
  const [coreInput, setCoreInput] = useState("");
  const [frontendInput, setFrontendInput] = useState("");
  const [nodeInput, setNodeInput] = useState("");
  const [corePreview, setCorePreview] = useState<ResolvedTarget | null>(null);
  const [frontendPreview, setFrontendPreview] = useState<ResolvedTarget | null>(null);
  const [nodePreview, setNodePreview] = useState<ResolvedTarget | null>(null);
  const [corePreviewError, setCorePreviewError] = useState<string | null>(null);
  const [frontendPreviewError, setFrontendPreviewError] = useState<string | null>(null);
  const [nodePreviewError, setNodePreviewError] = useState<string | null>(null);
  const [actionError, setActionError] = useState<string | null>(null);
  const [events, setEvents] = useState<OperationEvent[]>([]);
  const [registryRefreshToken, setRegistryRefreshToken] = useState(0);
  const [registerForm, setRegisterForm] = useState({
    name: "Primary ComfyUI",
    comfyRoot: "",
    pythonExe: "",
    launchCommand: defaultLaunchProfile.command,
    launchArgs: defaultLaunchProfile.args.join(" "),
    launchCwd: "",
    frontendRepoRoot: "",
    frontendDistPath: "",
    frontendPackageManager: defaultFrontendPackageManager
  });
  const [installationForm, setInstallationForm] = useState({
    name: "",
    pythonExe: "",
    launchCommand: defaultLaunchProfile.command,
    launchArgs: defaultLaunchProfile.args.join(" "),
    extraArgs: "",
    launchCwd: "",
    stopCommand: "",
    stopArgs: "",
    restartCommand: "",
    restartArgs: "",
    frontendRepoRoot: "",
    frontendDistPath: "",
    frontendPackageManager: defaultFrontendPackageManager
  });

  const detailRequestSeq = useRef(0);
  const corePreviewRequestSeq = useRef(0);
  const frontendPreviewRequestSeq = useRef(0);
  const nodePreviewRequestSeq = useRef(0);
  const selectedInstallationIdRef = useRef<string | null>(null);
  const coreInputRef = useRef("");
  const frontendInputRef = useRef("");
  const nodeInputRef = useRef("");

  useEffect(() => {
    selectedInstallationIdRef.current = selectedInstallationId;
  }, [selectedInstallationId]);

  useEffect(() => {
    coreInputRef.current = coreInput;
  }, [coreInput]);

  useEffect(() => {
    frontendInputRef.current = frontendInput;
  }, [frontendInput]);

  useEffect(() => {
    nodeInputRef.current = nodeInput;
  }, [nodeInput]);

  const selectedInstallation = useMemo(
    () => installations.find((item) => item.id === selectedInstallationId) ?? null,
    [installations, selectedInstallationId]
  );

  useEffect(() => {
    const installation = detail?.installation ?? selectedInstallation;
    if (!installation) {
      setInstallationForm({
        name: "",
        pythonExe: "",
        launchCommand: defaultLaunchProfile.command,
        launchArgs: defaultLaunchProfile.args.join(" "),
        extraArgs: "",
        launchCwd: "",
        stopCommand: "",
        stopArgs: "",
        restartCommand: "",
        restartArgs: "",
        frontendRepoRoot: "",
        frontendDistPath: "",
        frontendPackageManager: defaultFrontendPackageManager
      });
      return;
    }
    setInstallationForm({
      name: installation.name,
      pythonExe: installation.pythonExe,
      launchCommand: installation.launchProfile?.command ?? defaultLaunchProfile.command,
      launchArgs: (installation.launchProfile?.args ?? defaultLaunchProfile.args).join(" "),
      extraArgs: (installation.launchProfile?.extraArgs ?? []).join(" "),
      launchCwd: installation.launchProfile?.cwd ?? "",
      stopCommand: installation.launchProfile?.stopCommand ?? "",
      stopArgs: (installation.launchProfile?.stopArgs ?? []).join(" "),
      restartCommand: installation.launchProfile?.restartCommand ?? "",
      restartArgs: (installation.launchProfile?.restartArgs ?? []).join(" "),
      frontendRepoRoot: installation.frontendSettings?.repoRoot ?? "",
      frontendDistPath: installation.frontendSettings?.distPath ?? "",
      frontendPackageManager:
        installation.frontendSettings?.packageManager ?? defaultFrontendPackageManager
    });
  }, [detail, selectedInstallation]);

  async function refreshInstallations() {
    const next = await api.listInstallations();
    setInstallations(next);
    if (!selectedInstallationIdRef.current && next.length) {
      setSelectedInstallationId(next[0].id);
    }
  }

  async function runAction(action: () => Promise<void>) {
    setActionError(null);
    try {
      await action();
    } catch (error) {
      setActionError(toErrorMessage(error));
    }
  }

  async function runActionOk(action: () => Promise<void>): Promise<boolean> {
    setActionError(null);
    try {
      await action();
      return true;
    } catch (error) {
      setActionError(toErrorMessage(error));
      return false;
    }
  }

  async function refreshDetail(
    installationId: string | null,
    options?: { clear?: boolean }
  ) {
    const requestSeq = ++detailRequestSeq.current;
    if (!installationId) {
      setDetail(null);
      return;
    }
    if (options?.clear) {
      setDetail(null);
    }
    const next = await api.getInstallationDetail(installationId);
    if (detailRequestSeq.current !== requestSeq) return;
    if (selectedInstallationIdRef.current !== installationId) return;
    setDetail(next);
  }

  useEffect(() => {
    void refreshInstallations();
  }, []);

  useEffect(() => {
    setCorePreview(null);
    setFrontendPreview(null);
    setNodePreview(null);
    setCorePreviewError(null);
    setFrontendPreviewError(null);
    setNodePreviewError(null);
    void refreshDetail(selectedInstallationId, { clear: true });
  }, [selectedInstallationId]);

  useEffect(() => {
    let cancelled = false;
    let unlisten: (() => void) | null = null;
    api
      .subscribeOperationEvents((event) => {
        if (cancelled) return;
        setEvents((prev) => [event, ...prev].slice(0, 100));
        if (event.phase === "done" || event.phase === "error") {
          setRegistryRefreshToken((value) => value + 1);
        }
        const installationId = selectedInstallationIdRef.current;
        if (installationId) {
          void refreshDetail(installationId);
        }
        void refreshInstallations();
      })
      .then((fn) => {
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
  }, []);

  useEffect(() => {
    setCorePreview(null);
    setCorePreviewError(null);
    corePreviewRequestSeq.current += 1;
  }, [coreInput]);

  useEffect(() => {
    setFrontendPreview(null);
    setFrontendPreviewError(null);
    frontendPreviewRequestSeq.current += 1;
  }, [frontendInput]);

  useEffect(() => {
    setNodePreview(null);
    setNodePreviewError(null);
    nodePreviewRequestSeq.current += 1;
  }, [nodeInput]);

  async function preview(input: ResolveTargetInput, target: "core" | "frontend" | "node") {
    if (!input.installationId || !input.input.trim()) {
      if (target === "core") {
        setCorePreview(null);
        setCorePreviewError(null);
      } else if (target === "frontend") {
        setFrontendPreview(null);
        setFrontendPreviewError(null);
      } else {
        setNodePreview(null);
        setNodePreviewError(null);
      }
      return;
    }

    const requestSeq =
      target === "core"
        ? ++corePreviewRequestSeq.current
        : target === "frontend"
          ? ++frontendPreviewRequestSeq.current
          : ++nodePreviewRequestSeq.current;

    try {
      const resolved = await api.resolveTarget(input);

      if (selectedInstallationIdRef.current !== input.installationId) return;
      if (target === "core") {
        if (corePreviewRequestSeq.current !== requestSeq) return;
        if (coreInputRef.current !== input.input) return;
        setCorePreview(resolved);
        setCorePreviewError(null);
      } else if (target === "frontend") {
        if (frontendPreviewRequestSeq.current !== requestSeq) return;
        if (frontendInputRef.current !== input.input) return;
        setFrontendPreview(resolved);
        setFrontendPreviewError(null);
      } else {
        if (nodePreviewRequestSeq.current !== requestSeq) return;
        if (nodeInputRef.current !== input.input) return;
        setNodePreview(resolved);
        setNodePreviewError(null);
      }
    } catch (error) {
      if (selectedInstallationIdRef.current !== input.installationId) return;
      const message = error instanceof Error ? error.message : String(error);
      if (target === "core") {
        if (corePreviewRequestSeq.current !== requestSeq) return;
        if (coreInputRef.current !== input.input) return;
        setCorePreview(null);
        setCorePreviewError(message);
      } else if (target === "frontend") {
        if (frontendPreviewRequestSeq.current !== requestSeq) return;
        if (frontendInputRef.current !== input.input) return;
        setFrontendPreview(null);
        setFrontendPreviewError(message);
      } else {
        if (nodePreviewRequestSeq.current !== requestSeq) return;
        if (nodeInputRef.current !== input.input) return;
        setNodePreview(null);
        setNodePreviewError(message);
      }
    }
  }

  const coreRepo = detail?.coreRepo ?? null;
  const frontendRepo = detail?.frontendRepo ?? null;
  const customNodeRepos = detail?.customNodeRepos ?? [];
  const hasMatchingDetail =
    !!selectedInstallation && detail?.installation.id === selectedInstallation.id;
  const existingInstallationProfile =
    detail?.installation.launchProfile ??
    selectedInstallation?.launchProfile ??
    defaultLaunchProfile;
  const savedFrontendSettings =
    detail?.installation.frontendSettings ??
    selectedInstallation?.frontendSettings ??
    null;

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
          <div className="muted small">Re-registering the same ComfyUI root updates the existing entry instead of creating a duplicate.</div>
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
              placeholder="Optional; leave blank to inherit default process cwd"
              value={registerForm.launchCwd}
              onChange={(e) => setRegisterForm((v) => ({ ...v, launchCwd: e.target.value }))}
            />
          </label>
          <label>
            <span>Managed frontend repo root</span>
            <input
              placeholder="Optional; separate checkout for ComfyUI_frontend"
              value={registerForm.frontendRepoRoot}
              onChange={(e) =>
                setRegisterForm((v) => ({ ...v, frontendRepoRoot: e.target.value }))
              }
            />
          </label>
          <label>
            <span>Managed frontend dist path</span>
            <input
              placeholder="Optional; defaults to &lt;frontend repo root&gt;/dist"
              value={registerForm.frontendDistPath}
              onChange={(e) =>
                setRegisterForm((v) => ({ ...v, frontendDistPath: e.target.value }))
              }
            />
          </label>
          <label>
            <span>Managed frontend package manager</span>
            <select
              value={registerForm.frontendPackageManager}
              onChange={(e) =>
                setRegisterForm((v) => ({
                  ...v,
                  frontendPackageManager: e.target.value as FrontendPackageManager
                }))
              }
            >
              {frontendPackageManagerOptions.map((option) => (
                <option key={option.value} value={option.value}>
                  {option.label}
                </option>
              ))}
            </select>
          </label>
          <button
            onClick={() =>
              void runAction(async () => {
                const launchArgs = parseLaunchArgs(registerForm.launchArgs);
                const launchCwd =
                  registerForm.launchCwd.trim() ||
                  (registerForm.launchCommand === defaultLaunchProfile.command &&
                  launchArgs.length === defaultLaunchProfile.args.length &&
                  launchArgs.every((arg, index) => arg === defaultLaunchProfile.args[index])
                    ? registerForm.comfyRoot
                    : null);
                const result = await api.registerInstallation({
                  name: registerForm.name,
                  comfyRoot: registerForm.comfyRoot,
                  pythonExe: registerForm.pythonExe || null,
                  launchProfile: {
                    mode: "managed_child",
                    command: registerForm.launchCommand,
                    args: launchArgs,
                    cwd: launchCwd,
                    env: {}
                  },
                  frontendSettings: buildFrontendSettingsPayload(registerForm)
                });
                setCorePreview(null);
                setFrontendPreview(null);
                setNodePreview(null);
                setCorePreviewError(null);
                setFrontendPreviewError(null);
                setNodePreviewError(null);
                await refreshInstallations();
                setSelectedInstallationId(result.installation.id);
              })
            }
          >
            Register / Update by root
          </button>
          {actionError ? <div className="muted">{actionError}</div> : null}
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
        {selectedInstallation && hasMatchingDetail ? (
          <>
            <section className="page-header card">
              <div className="row between">
                <div>
                  <h2>{selectedInstallation.name}</h2>
                  <div className="mono">{selectedInstallation.comfyRoot}</div>
                </div>
                <div className="row gap">
                  <button
                    onClick={() =>
                      void runAction(async () => {
                        await api.updateAll({
                          installationId: selectedInstallation.id,
                          dirtyRepoStrategy: "abort",
                          syncDependencies: true,
                          restartAfterSuccess: false
                        });
                      })
                    }
                  >
                    Update all
                  </button>
                  <button
                    className="secondary"
                    disabled={detail?.isRunning ?? false}
                    onClick={() =>
                      void runAction(async () => {
                        await api.startInstallation({ installationId: selectedInstallation.id });
                      })
                    }
                  >
                    Start
                  </button>
                  <button
                    className="secondary"
                    disabled={!(detail?.isRunning ?? false)}
                    onClick={() =>
                      void runAction(async () => {
                        await api.stopInstallation({ installationId: selectedInstallation.id });
                      })
                    }
                  >
                    Stop
                  </button>
                  <button
                    className="secondary"
                    disabled={!(detail?.isRunning ?? false)}
                    onClick={() =>
                      void runAction(async () => {
                        await api.restartInstallation({ installationId: selectedInstallation.id });
                      })
                    }
                  >
                    Restart
                  </button>
                </div>
              </div>
            </section>

            <section className="card">
              <div className="row between">
                <div>
                  <h3>Installation settings</h3>
                  <div className="muted small">Root path is fixed for an existing installation. Delete and re-register if you need a different root.</div>
                </div>
                <div className="row gap">
                  <button
                    className="secondary"
                    onClick={() =>
                      setInstallationForm({
                        name: detail?.installation.name ?? selectedInstallation.name,
                        pythonExe: detail?.installation.pythonExe ?? selectedInstallation.pythonExe,
                        launchCommand:
                          detail?.installation.launchProfile?.command ??
                          selectedInstallation.launchProfile?.command ??
                          defaultLaunchProfile.command,
                        launchArgs: (
                          detail?.installation.launchProfile?.args ??
                          selectedInstallation.launchProfile?.args ??
                          defaultLaunchProfile.args
                        ).join(" "),
                        extraArgs: (
                          detail?.installation.launchProfile?.extraArgs ??
                          selectedInstallation.launchProfile?.extraArgs ??
                          []
                        ).join(" "),
                        launchCwd:
                          detail?.installation.launchProfile?.cwd ??
                          selectedInstallation.launchProfile?.cwd ??
                          "",
                        stopCommand:
                          detail?.installation.launchProfile?.stopCommand ??
                          selectedInstallation.launchProfile?.stopCommand ??
                          "",
                        stopArgs: (
                          detail?.installation.launchProfile?.stopArgs ??
                          selectedInstallation.launchProfile?.stopArgs ??
                          []
                        ).join(" "),
                        restartCommand:
                          detail?.installation.launchProfile?.restartCommand ??
                          selectedInstallation.launchProfile?.restartCommand ??
                          "",
                        restartArgs: (
                          detail?.installation.launchProfile?.restartArgs ??
                          selectedInstallation.launchProfile?.restartArgs ??
                          []
                        ).join(" "),
                        frontendRepoRoot:
                          detail?.installation.frontendSettings?.repoRoot ??
                          selectedInstallation.frontendSettings?.repoRoot ??
                          "",
                        frontendDistPath:
                          detail?.installation.frontendSettings?.distPath ??
                          selectedInstallation.frontendSettings?.distPath ??
                          "",
                        frontendPackageManager:
                          detail?.installation.frontendSettings?.packageManager ??
                          selectedInstallation.frontendSettings?.packageManager ??
                          defaultFrontendPackageManager
                      })
                    }
                  >
                    Reset
                  </button>
                  <button
                    onClick={() =>
                      void runAction(async () => {
                        const payload: SaveInstallationInput = {
                          installationId: selectedInstallation.id,
                          name: installationForm.name,
                          pythonExe: installationForm.pythonExe || null,
                          launchProfile: {
                            ...existingInstallationProfile,
                            command: installationForm.launchCommand,
                            args: parseLaunchArgs(installationForm.launchArgs),
                            extraArgs: parseOptionalArgs(installationForm.extraArgs),
                            cwd: installationForm.launchCwd.trim() || null,
                            stopCommand: installationForm.stopCommand.trim() || null,
                            stopArgs: installationForm.stopCommand.trim()
                              ? parseOptionalArgs(installationForm.stopArgs)
                              : null,
                            restartCommand: installationForm.restartCommand.trim() || null,
                            restartArgs: installationForm.restartCommand.trim()
                              ? parseOptionalArgs(installationForm.restartArgs)
                              : null
                          },
                          frontendSettings: buildFrontendSettingsPayload(installationForm)
                        };
                        await api.saveInstallation(payload);
                        await refreshInstallations();
                        await refreshDetail(selectedInstallation.id);
                      })
                    }
                  >
                    Save settings
                  </button>
                  <button
                    className="secondary"
                    onClick={() =>
                      void runAction(async () => {
                        if (!window.confirm(`Delete installation entry for ${selectedInstallation.name}? This only removes it from ComfyUI Patcher.`)) {
                          return;
                        }
                        await api.deleteInstallation({ installationId: selectedInstallation.id });
                        setCorePreview(null);
                        setFrontendPreview(null);
                        setNodePreview(null);
                        setCorePreviewError(null);
                        setFrontendPreviewError(null);
                        setNodePreviewError(null);
                        setDetail(null);
                        const next = await api.listInstallations();
                        setInstallations(next);
                        setSelectedInstallationId(next[0]?.id ?? null);
                      })
                    }
                  >
                    Delete entry
                  </button>
                </div>
              </div>
              <div className="grid two">
                <label>
                  <span>Name</span>
                  <input
                    value={installationForm.name}
                    onChange={(e) => setInstallationForm((v) => ({ ...v, name: e.target.value }))}
                  />
                </label>
                <label>
                  <span>ComfyUI root</span>
                  <input value={selectedInstallation.comfyRoot} readOnly />
                </label>
                <label>
                  <span>Python executable</span>
                  <input
                    value={installationForm.pythonExe}
                    onChange={(e) =>
                      setInstallationForm((v) => ({ ...v, pythonExe: e.target.value }))
                    }
                  />
                </label>
                <label>
                  <span>Launch command</span>
                  <input
                    value={installationForm.launchCommand}
                    onChange={(e) =>
                      setInstallationForm((v) => ({ ...v, launchCommand: e.target.value }))
                    }
                  />
                </label>
                <label>
                  <span>Launch args</span>
                  <input
                    value={installationForm.launchArgs}
                    onChange={(e) =>
                      setInstallationForm((v) => ({ ...v, launchArgs: e.target.value }))
                    }
                  />
                </label>
                <label>
                  <span>Appended args</span>
                  <input
                    value={installationForm.extraArgs}
                    onChange={(e) =>
                      setInstallationForm((v) => ({ ...v, extraArgs: e.target.value }))
                    }
                  />
                </label>
                <label>
                  <span>Launch cwd</span>
                  <input
                    placeholder="Optional; leave blank to inherit default process cwd"
                    value={installationForm.launchCwd}
                    onChange={(e) =>
                      setInstallationForm((v) => ({ ...v, launchCwd: e.target.value }))
                    }
                  />
                </label>
                <label>
                  <span>Stop command</span>
                  <input
                    value={installationForm.stopCommand}
                    onChange={(e) =>
                      setInstallationForm((v) => ({ ...v, stopCommand: e.target.value }))
                    }
                  />
                </label>
                <label>
                  <span>Stop args</span>
                  <input
                    value={installationForm.stopArgs}
                    onChange={(e) =>
                      setInstallationForm((v) => ({ ...v, stopArgs: e.target.value }))
                    }
                  />
                </label>
                <label>
                  <span>Restart command</span>
                  <input
                    value={installationForm.restartCommand}
                    onChange={(e) =>
                      setInstallationForm((v) => ({ ...v, restartCommand: e.target.value }))
                    }
                  />
                </label>
                <label>
                  <span>Restart args</span>
                  <input
                    value={installationForm.restartArgs}
                    onChange={(e) =>
                      setInstallationForm((v) => ({ ...v, restartArgs: e.target.value }))
                    }
                  />
                </label>
                <label>
                  <span>Managed frontend repo root</span>
                  <input
                    placeholder="Optional; separate checkout for ComfyUI_frontend"
                    value={installationForm.frontendRepoRoot}
                    onChange={(e) =>
                      setInstallationForm((v) => ({ ...v, frontendRepoRoot: e.target.value }))
                    }
                  />
                </label>
                <label>
                  <span>Managed frontend dist path</span>
                  <input
                    placeholder="Optional; defaults to &lt;frontend repo root&gt;/dist"
                    value={installationForm.frontendDistPath}
                    onChange={(e) =>
                      setInstallationForm((v) => ({ ...v, frontendDistPath: e.target.value }))
                    }
                  />
                </label>
                <label>
                  <span>Managed frontend package manager</span>
                  <select
                    value={installationForm.frontendPackageManager}
                    onChange={(e) =>
                      setInstallationForm((v) => ({
                        ...v,
                        frontendPackageManager: e.target.value as FrontendPackageManager
                      }))
                    }
                  >
                    {frontendPackageManagerOptions.map((option) => (
                      <option key={option.value} value={option.value}>
                        {option.label}
                      </option>
                    ))}
                  </select>
                </label>
              </div>
              <div className="muted small">Appended args are passed after the base launch or restart args. If your launch command calls a shell script, that script should use <code>exec</code> for the final ComfyUI process, and forward <code>&quot;$@&quot;</code> if you want appended args to reach ComfyUI.</div>
              <div className="muted small">When managed frontend settings are configured, Start and Restart strip any existing <code>--front-end-root</code> from the stored launch args, restart args, and appended args, then inject the managed dist path at runtime.</div>
            </section>

            <section className="card">
              <h3>Patch core ComfyUI</h3>
              <div className="muted small">PR URLs append to the tracked overlay stack for this checkout. If the repo already has overlays, change the base from the repo card instead of this Apply button.</div>
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
                  onClick={() =>
                    void runAction(async () => {
                      await api.patchCore({
                        installationId: selectedInstallation.id,
                        input: coreInput,
                        dirtyRepoStrategy: "abort",
                        setTrackedTarget: true,
                        syncDependencies: true,
                        restartAfterSuccess: false
                      });
                      setCoreInput("");
                      setCorePreview(null);
                      setCorePreviewError(null);
                    })
                  }
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
              {corePreviewError ? <div className="muted">{corePreviewError}</div> : null}
              {coreRepo ? (
                <RepoCard
                  key={coreRepo.id}
                  repo={coreRepo}
                  onUpdate={() =>
                    void runAction(async () => {
                      await api.updateRepo({
                        repoId: coreRepo.id,
                        dirtyRepoStrategy: "abort",
                        syncDependencies: true
                      });
                    })
                  }
                  onSetBaseTarget={(input, clearOverlays) =>
                    runActionOk(async () => {
                      await api.setRepoBaseTarget({
                        repoId: coreRepo.id,
                        input,
                        clearOverlays,
                        dirtyRepoStrategy: "abort",
                        syncDependencies: true
                      });
                    })
                  }
                  onAddOverlay={(input) =>
                    runActionOk(async () => {
                      await api.addRepoOverlay({
                        repoId: coreRepo.id,
                        input,
                        dirtyRepoStrategy: "abort",
                        syncDependencies: true
                      });
                    })
                  }
                  onSetOverlayEnabled={(overlayId, enabled) =>
                    runActionOk(async () => {
                      await api.setRepoOverlayEnabled({
                        repoId: coreRepo.id,
                        overlayId,
                        enabled,
                        dirtyRepoStrategy: "abort",
                        syncDependencies: true
                      });
                    })
                  }
                  onRemoveOverlay={(overlayId) =>
                    runActionOk(async () => {
                      await api.removeRepoOverlay({
                        repoId: coreRepo.id,
                        overlayId,
                        dirtyRepoStrategy: "abort",
                        syncDependencies: true
                      });
                    })
                  }
                  onMoveOverlay={(overlayId, direction) =>
                    runActionOk(async () => {
                      await api.moveRepoOverlay({
                        repoId: coreRepo.id,
                        overlayId,
                        direction,
                        dirtyRepoStrategy: "abort",
                        syncDependencies: true
                      });
                    })
                  }
                  onRollback={() =>
                    void runAction(async () => {
                      await api.rollbackRepo({
                        repoId: coreRepo.id,
                        restoreStash: true,
                        syncDependencies: true,
                        restartAfterSuccess: false
                      });
                    })
                  }
                />
              ) : (
                <div className="muted">No managed core repo is registered for this installation.</div>
              )}
            </section>

            <section className="card">
              <h3>Install or patch ComfyUI frontend</h3>
              <div className="muted small">Frontend PRs and branches are managed in a dedicated checkout outside <code>custom_nodes</code>. Dependency sync installs Node dependencies, runs the frontend build, and Start/Restart inject the managed <code>--front-end-root</code> automatically.</div>
              <div className="row gap">
                <input
                  className="grow"
                  placeholder="Repository URL, branch URL, or PR URL"
                  value={frontendInput}
                  onChange={(e) => setFrontendInput(e.target.value)}
                />
                <button
                  className="secondary"
                  onClick={() =>
                    void preview(
                      {
                        installationId: selectedInstallation.id,
                        kind: "frontend",
                        input: frontendInput,
                        repoId: frontendRepo?.id ?? null
                      },
                      "frontend"
                    )
                  }
                >
                  Resolve
                </button>
                <button
                  disabled={!frontendInput.trim()}
                  onClick={() =>
                    void runAction(async () => {
                      await api.installOrPatchFrontend({
                        installationId: selectedInstallation.id,
                        input: frontendInput,
                        existingRepoConflictStrategy: "replace",
                        dirtyRepoStrategy: "abort",
                        setTrackedTarget: true,
                        syncDependencies: true,
                        restartAfterSuccess: false
                      });
                      setFrontendInput("");
                      setFrontendPreview(null);
                      setFrontendPreviewError(null);
                    })
                  }
                >
                  Install / Patch
                </button>
              </div>
              {!savedFrontendSettings ? (
                <div className="muted">No managed frontend repo root is configured yet. A fresh frontend install will automatically use the default sibling checkout path; save Installation settings only if you want to override it.</div>
              ) : null}
              {frontendPreview ? (
                <div className="preview">
                  <div><strong>{frontendPreview.summaryLabel}</strong></div>
                  <div className="mono small">{frontendPreview.canonicalRepoUrl}</div>
                  <div className="mono small">{frontendPreview.resolvedSha ?? frontendPreview.checkoutRef}</div>
                </div>
              ) : null}
              {frontendPreviewError ? <div className="muted">{frontendPreviewError}</div> : null}
              {frontendRepo ? (
                <RepoCard
                  key={frontendRepo.id}
                  repo={frontendRepo}
                  onUpdate={() =>
                    void runAction(async () => {
                      await api.updateRepo({
                        repoId: frontendRepo.id,
                        dirtyRepoStrategy: "abort",
                        syncDependencies: true
                      });
                    })
                  }
                  onSetBaseTarget={(input, clearOverlays) =>
                    runActionOk(async () => {
                      await api.setRepoBaseTarget({
                        repoId: frontendRepo.id,
                        input,
                        clearOverlays,
                        dirtyRepoStrategy: "abort",
                        syncDependencies: true
                      });
                    })
                  }
                  onAddOverlay={(input) =>
                    runActionOk(async () => {
                      await api.addRepoOverlay({
                        repoId: frontendRepo.id,
                        input,
                        dirtyRepoStrategy: "abort",
                        syncDependencies: true
                      });
                    })
                  }
                  onSetOverlayEnabled={(overlayId, enabled) =>
                    runActionOk(async () => {
                      await api.setRepoOverlayEnabled({
                        repoId: frontendRepo.id,
                        overlayId,
                        enabled,
                        dirtyRepoStrategy: "abort",
                        syncDependencies: true
                      });
                    })
                  }
                  onRemoveOverlay={(overlayId) =>
                    runActionOk(async () => {
                      await api.removeRepoOverlay({
                        repoId: frontendRepo.id,
                        overlayId,
                        dirtyRepoStrategy: "abort",
                        syncDependencies: true
                      });
                    })
                  }
                  onMoveOverlay={(overlayId, direction) =>
                    runActionOk(async () => {
                      await api.moveRepoOverlay({
                        repoId: frontendRepo.id,
                        overlayId,
                        direction,
                        dirtyRepoStrategy: "abort",
                        syncDependencies: true
                      });
                    })
                  }
                  onRollback={() =>
                    void runAction(async () => {
                      await api.rollbackRepo({
                        repoId: frontendRepo.id,
                        restoreStash: true,
                        syncDependencies: true,
                        restartAfterSuccess: false
                      });
                    })
                  }
                />
              ) : (
                <div className="muted">No managed frontend repo is registered for this installation.</div>
              )}
            </section>

            <section className="card">
              <h3>Install or patch custom node manually</h3>
              <div className="muted small">On an existing managed repo, PR URLs append to that repo's overlay stack. If the repo already has overlays, change the base from the repo card instead of this Install / Patch button.</div>
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
                  onClick={() =>
                    void runAction(async () => {
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
                      setNodePreview(null);
                      setNodePreviewError(null);
                    })
                  }
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
              {nodePreviewError ? <div className="muted">{nodePreviewError}</div> : null}
            </section>

            <ManagerRegistryBrowser
              installationId={selectedInstallation.id}
              refreshToken={registryRefreshToken}
              onInstall={async (entry) => {
                await runAction(async () => {
                  const targetLocalDirName =
                    entry.isTrackingManaged && !(entry.installedRepoId || entry.installedLocalPath)
                      ? (entry.trackingLocalPath?.split(/[\\/]/).filter(Boolean).pop() ?? undefined)
                      : undefined;
                  await api.installOrPatchCustomNode({
                    installationId: selectedInstallation.id,
                    input: entry.sourceInput ?? "",
                    targetLocalDirName,
                    existingRepoConflictStrategy: "install_with_suffix",
                    dirtyRepoStrategy: "abort",
                    setTrackedTarget: true,
                    syncDependencies: true,
                    restartAfterSuccess: false,
                    adoptTrackingInstall: entry.isTrackingManaged && !(entry.installedRepoId || entry.installedLocalPath)
                  });
                });
              }}
              onUseSourceInput={(sourceInput) => {
                setNodeInput(sourceInput);
                setNodePreview(null);
                setNodePreviewError(null);
              }}
            />

            <section className="grid two">
              <div className="card">
                <h3>Managed custom nodes</h3>
                {customNodeRepos.length ? (
                  <div className="stack">
                    {customNodeRepos.map((repo: ManagedRepo) => (
                      <RepoCard
                        key={repo.id}
                        repo={repo}
                        onSetBaseTarget={(input, clearOverlays) =>
                          runActionOk(async () => {
                            await api.setRepoBaseTarget({
                              repoId: repo.id,
                              input,
                              clearOverlays,
                              dirtyRepoStrategy: "abort",
                              syncDependencies: true
                            });
                          })
                        }
                        onAddOverlay={(input) =>
                          runActionOk(async () => {
                            await api.addRepoOverlay({
                              repoId: repo.id,
                              input,
                              dirtyRepoStrategy: "abort",
                              syncDependencies: true
                            });
                          })
                        }
                        onSetOverlayEnabled={(overlayId, enabled) =>
                          runActionOk(async () => {
                            await api.setRepoOverlayEnabled({
                              repoId: repo.id,
                              overlayId,
                              enabled,
                              dirtyRepoStrategy: "abort",
                              syncDependencies: true
                            });
                          })
                        }
                        onRemoveOverlay={(overlayId) =>
                          runActionOk(async () => {
                            await api.removeRepoOverlay({
                              repoId: repo.id,
                              overlayId,
                              dirtyRepoStrategy: "abort",
                              syncDependencies: true
                            });
                          })
                        }
                        onMoveOverlay={(overlayId, direction) =>
                          runActionOk(async () => {
                            await api.moveRepoOverlay({
                              repoId: repo.id,
                              overlayId,
                              direction,
                              dirtyRepoStrategy: "abort",
                              syncDependencies: true
                            });
                          })
                        }
                        onUpdate={() =>
                          void runAction(async () => {
                            await api.updateRepo({
                              repoId: repo.id,
                              dirtyRepoStrategy: "abort",
                              syncDependencies: true
                            });
                          })
                        }
                        onRollback={() =>
                          void runAction(async () => {
                            await api.rollbackRepo({
                              repoId: repo.id,
                              restoreStash: true,
                              syncDependencies: true,
                              restartAfterSuccess: false
                            });
                          })
                        }
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
            <h2>{selectedInstallation ? "Loading installation" : "No installation selected"}</h2>
            <div className="muted">
              {selectedInstallation
                ? "Refreshing installation details."
                : "Register a ComfyUI root on the left to begin."}
            </div>
          </section>
        )}
      </main>
    </div>
  );
}
