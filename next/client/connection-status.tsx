import { Icon } from "./icons.tsx";

export type ConnectionState = "connecting" | "ready" | "reconnecting" | "error";

/** One status and recovery action for every app's live connection. */
export function ConnectionStatus({ state }: { state: ConnectionState }) {
  if (state === "ready") return null;
  const failed = state === "error";
  return <div className={failed ? "error-banner" : "connection-state"} role={failed ? "alert" : "status"}>
    <Icon name="refresh" />
    <span>{failed ? "Unable to connect. Refresh to try again." : state === "reconnecting" ? "Connection lost. Reconnecting…" : "Connecting…"}</span>
    {failed && <button className="text-button" onClick={() => location.reload()}>Try again</button>}
  </div>;
}
