import { useEffect, useState } from "react";
import { api } from "../api";
import type { OperationRecord } from "../types";

type Props = {
  installationId: string | null;
};

export default function OperationPanel({ installationId }: Props) {
  const [operations, setOperations] = useState<OperationRecord[]>([]);
  const [selected, setSelected] = useState<OperationRecord | null>(null);
  const [log, setLog] = useState("");

  async function refresh() {
    const next = await api.listOperations(installationId);
    setOperations(next);
    if (selected) {
      const match = next.find((op) => op.id === selected.id) ?? null;
      setSelected(match);
      if (match) {
        setLog(await api.getOperationLog(match.id));
      }
    }
  }

  useEffect(() => {
    void refresh();
    const id = window.setInterval(() => {
      void refresh();
    }, 2500);
    return () => window.clearInterval(id);
  }, [installationId, selected?.id]);

  return (
    <div className="card fill">
      <div className="row between">
        <h3>Operations</h3>
        <button className="secondary" onClick={() => void refresh()}>Refresh</button>
      </div>
      <div className="split">
        <div className="list">
          {operations.map((op) => (
            <button
              key={op.id}
              className={`list-item ${selected?.id === op.id ? "active" : ""}`}
              onClick={async () => {
                setSelected(op);
                setLog(await api.getOperationLog(op.id));
              }}
            >
              <div className="row between">
                <strong>{op.kind}</strong>
                <span className={`badge ${op.status === "failed" ? "danger" : op.status === "succeeded" ? "ok" : ""}`}>
                  {op.status}
                </span>
              </div>
              <div className="mono small">{op.id}</div>
              <div className="small">{op.requestedInput ?? "no input"}</div>
            </button>
          ))}
        </div>
        <div className="log-viewer">
          {selected ? (
            <>
              <div className="row between">
                <div>
                  <div><strong>{selected.kind}</strong></div>
                  <div className="small mono">{selected.id}</div>
                </div>
                <span className={`badge ${selected.status === "failed" ? "danger" : selected.status === "succeeded" ? "ok" : ""}`}>
                  {selected.status}
                </span>
              </div>
              <pre>{log || "No log output yet."}</pre>
            </>
          ) : (
            <div className="muted">Select an operation to view logs.</div>
          )}
        </div>
      </div>
    </div>
  );
}
