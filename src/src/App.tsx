import { useEffect } from "react";
import { useBuildInfo } from "./store";

/**
 * GitGotten app shell.
 *
 * Stage 0: a top bar (product name + build identity) over a content area. The build
 * identity comes from the Rust core over IPC, which proves the Rust -> web bridge.
 * Stage 1 replaces the content area with the repo workspace.
 */
export default function App() {
  const { info, loading, error, load } = useBuildInfo();

  useEffect(() => {
    void load();
  }, [load]);

  return (
    <main className="shell">
      <header className="topbar">
        <h1 className="brand">GitGotten</h1>
        {info && (
          <span className="version">
            {info.name} {info.version} · {info.arch}
          </span>
        )}
      </header>

      <section className="content">
        {loading ? (
          <p>Connecting to the Rust core…</p>
        ) : error ? (
          <p className="error">{error}</p>
        ) : info ? (
          <p>
            Connected to the Rust core. Stage 1 opens a repository and reads its live
            status.
          </p>
        ) : (
          <p>
            Running outside Tauri (plain web preview). Start with <code>pnpm tauri dev</code>{" "}
            to connect to the Rust core.
          </p>
        )}
      </section>
    </main>
  );
}
