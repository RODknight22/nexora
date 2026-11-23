import { useCallback, useEffect, useState } from "react";
import { fetchAppStatus, type AppStatus } from "./lib/tauri";
import "./App.css";

const DEFAULT_MODULES = ["scanner", "detectors", "cleaner"];

function PixelEmblem() {
  return (
    <svg
      className="nexora-emblem"
      viewBox="0 0 16 16"
      shapeRendering="crispEdges"
      aria-hidden="true"
    >
      <rect x="1" y="1" width="14" height="1" fill="#22d3ee" />
      <rect x="1" y="14" width="14" height="1" fill="#22d3ee" />
      <rect x="1" y="2" width="1" height="12" fill="#1e293b" />
      <rect x="14" y="2" width="1" height="12" fill="#1e293b" />
      <rect x="3" y="3" width="2" height="10" fill="#f1f5f9" />
      <rect x="5" y="5" width="2" height="2" fill="#22d3ee" />
      <rect x="7" y="7" width="2" height="2" fill="#22d3ee" />
      <rect x="9" y="9" width="2" height="2" fill="#22d3ee" />
      <rect x="11" y="3" width="2" height="10" fill="#f1f5f9" />
      <rect x="12" y="3" width="1" height="1" fill="#10b981" />
    </svg>
  );
}

export function App() {
  const [status, setStatus] = useState<AppStatus | null>(null);
  const [loading, setLoading] = useState<boolean>(true);
  const [error, setError] = useState<string | null>(null);

  const loadStatus = useCallback(async () => {
    try {
      const response = await fetchAppStatus();
      setStatus(response);
      setError(null);
    } catch (err) {
      const message =
        err instanceof Error
          ? err.message
          : "Entorno Tauri IPC no disponible fuera de la ventana nativa.";
      setStatus(null);
      setError(message);
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    let active = true;

    async function init() {
      try {
        const response = await fetchAppStatus();
        if (active) {
          setStatus(response);
          setError(null);
        }
      } catch (err) {
        if (active) {
          const message =
            err instanceof Error
              ? err.message
              : "Entorno Tauri IPC no disponible fuera de la ventana nativa.";
          setStatus(null);
          setError(message);
        }
      } finally {
        if (active) {
          setLoading(false);
        }
      }
    }

    void init();

    return () => {
      active = false;
    };
  }, []);

  const handleRefresh = () => {
    setLoading(true);
    void loadStatus();
  };

  const badgeState = loading
    ? "loading"
    : status?.engineReady
      ? "ready"
      : "offline";

  const badgeLabel = loading
    ? "VERIFICANDO..."
    : status?.engineReady
      ? "MOTOR ACTIVO"
      : "MODO AISLADO";

  const modules = status?.modules ?? DEFAULT_MODULES;

  return (
    <main className="nexora-shell">
      <section className="nexora-frame" aria-label="Panel inicial de NEXØRA">
        <header className="nexora-header">
          <div className="nexora-brand">
            <PixelEmblem />
            <div>
              <h1 className="nexora-title">NEXØRA</h1>
              <p className="nexora-tagline">Nueva generación de optimización</p>
            </div>
          </div>

          <div className="nexora-badge" data-state={badgeState}>
            <span className="nexora-dot" aria-hidden="true" />
            <span>{badgeLabel}</span>
          </div>
        </header>

        <div className="nexora-telemetry">
          <div className="telemetry-cell">
            <span className="telemetry-label">Núcleo</span>
            <span className="telemetry-value">
              {status ? status.appName : "NEXØRA"}
            </span>
          </div>
          <div className="telemetry-cell">
            <span className="telemetry-label">Versión</span>
            <span className="telemetry-value">
              {status ? `v${status.version}` : "v0.1.0"}
            </span>
          </div>
          <div className="telemetry-cell">
            <span className="telemetry-label">Enlace IPC</span>
            <span className="telemetry-value">
              {loading ? "Conectando" : status?.engineReady ? "Sincronizado" : "Sin runtime"}
            </span>
          </div>
        </div>

        <div className="modules-section">
          <h2 className="section-title">Módulos base preparados</h2>
          <div className="modules-grid">
            {modules.map((mod) => (
              <article key={mod} className="module-card">
                <div className="module-name">{mod}</div>
                <div className="module-state">
                  {status?.engineReady ? "EN ESPERA // LISTO" : "MODO STANDALONE"}
                </div>
              </article>
            ))}
          </div>
        </div>

        {error && (
          <div className="nexora-fallback" role="status">
            IPC en espera: {error}
          </div>
        )}

        <footer className="nexora-footer">
          <span className="footer-note">
            Seguridad Windows // Papelera de reciclaje obligatoria
          </span>
          <button
            type="button"
            className="nexora-button"
            onClick={handleRefresh}
            disabled={loading}
          >
            {loading ? "Consultando..." : "Verificar motor"}
          </button>
        </footer>
      </section>
    </main>
  );
}

export default App;
