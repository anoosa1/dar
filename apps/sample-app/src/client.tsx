/** @jsxImportSource @dar/app-sdk */
import {createRoot,useState,useEffect,AppWorkspace,SaveStatus,type AppHost,type AppRecord,type SaveState} from "@dar/app-sdk";
import "./style.css";

interface SampleNote {
  status: string;
}

const errorMessage = (e: unknown) =>
  e instanceof Error ? e.message : "The request failed. Please try again.";

function SampleApp({ host }: { host: AppHost }) {
  const [record, setRecord] = useState<AppRecord<SampleNote> | null>();
  const [draft, setDraft] = useState("");
  const [query, setQuery] = useState("");
  const [state, setState] = useState<SaveState>();
  const [error, setError] = useState("");

  async function load() {
    setError("");
    try {
      const value = await host.data.get<SampleNote>("status");
      if (host.signal.aborted) return;
      setRecord(value);
      setDraft(typeof value?.value?.status === "string" ? value.value.status : "");
    } catch (e) {
      if (!host.signal.aborted) setError(errorMessage(e));
    }
  }

  useEffect(() => {
    void load();
  }, []);

  async function save(nextText: string) {
    setState("saving");
    setError("");
    try {
      const saved = await host.data.put<SampleNote>(
        "status",
        { status: nextText },
        record?.revision ?? 0,
      );
      if (host.signal.aborted) return;
      setRecord(saved);
      setState("saved");
    } catch (e) {
      if (!host.signal.aborted) {
        setState("error");
        setError(errorMessage(e));
      }
    }
  }

  return (
    <AppWorkspace
      host={host}
      query={query}
      onQuery={setQuery}
      searchLabel="Search sample app"
      sidebar={
        <div className="sample-app-sidebar">
          <p className="muted small">
            Minimal installed app using Dar&apos;s SDK host data storage.
          </p>
        </div>
      }
    >
      <section className="sample-app-main" aria-labelledby="sample-app-title">
        <div className="sample-app-header">
          <h1 id="sample-app-title">Sample App</h1>
          <SaveStatus state={state} />
        </div>
        {error && (
          <div className="error" role="alert">
            {error}{" "}
            <button type="button" className="text-button" onClick={() => void load()}>
              Retry
            </button>
          </div>
        )}
        {record === undefined ? (
          <p role="status">Loading saved status…</p>
        ) : (
          <form
            className="sample-app-form"
            onSubmit={(event) => {
              event.preventDefault();
              void save(draft);
            }}
          >
            <label className="sample-app-field">
              <span>Status note</span>
              <input
                type="text"
                maxLength={200}
                value={draft}
                onChange={(event) => setDraft(event.target.value)}
                placeholder="Write a status note"
              />
            </label>
            <div className="sample-app-actions">
              <button type="submit" className="primary-button" disabled={state === "saving"}>
                Save
              </button>
            </div>
          </form>
        )}
      </section>
    </AppWorkspace>
  );
}

export function mount(element: HTMLElement, host: AppHost) {
  const root = createRoot(element, { onUncaughtError: host.onError });
  root.render(<SampleApp host={host} />);
  return () => root.unmount();
}
