import { afterEach, describe, expect, it } from "vitest";
import { fireEvent, render, screen } from "@testing-library/react";
import { clearMocks, mockIPC } from "@tauri-apps/api/mocks";
import App from "./App";

describe("App", () => {
  afterEach(() => {
    clearMocks();
  });

  it("muestra el título de NEXØRA, su lema y el estado IPC del motor Rust", async () => {
    mockIPC((cmd) => {
      if (cmd === "get_app_status") {
        return {
          appName: "NEXØRA",
          version: "0.1.0",
          engineReady: true,
          modules: ["scanner", "detectors", "cleaner"],
        };
      }
      return undefined;
    });

    render(<App />);

    expect(
      screen.getByRole("heading", { level: 1, name: "NEXØRA" }),
    ).toBeInTheDocument();
    expect(
      screen.getByText("Nueva generación de optimización"),
    ).toBeInTheDocument();

    expect(await screen.findByText("MOTOR ACTIVO")).toBeInTheDocument();
    expect(screen.getByText("v0.1.0")).toBeInTheDocument();
    expect(screen.getByText("scanner")).toBeInTheDocument();
    expect(screen.getByText("detectors")).toBeInTheDocument();
    expect(screen.getByText("cleaner")).toBeInTheDocument();
  });

  it("muestra un estado de respaldo limpio si el comando IPC falla", async () => {
    mockIPC((cmd) => {
      if (cmd === "get_app_status") {
        throw new Error("Tauri runtime no disponible");
      }
      return undefined;
    });

    render(<App />);

    expect(await screen.findByText("MODO AISLADO")).toBeInTheDocument();
    expect(
      screen.getByText(/IPC en espera: Tauri runtime no disponible/i),
    ).toBeInTheDocument();
  });

  it("vuelve a consultar el motor Rust al pulsar Verificar motor", async () => {
    let calls = 0;
    mockIPC((cmd) => {
      if (cmd === "get_app_status") {
        calls += 1;
        if (calls === 1) {
          throw new Error("Inicializando");
        }
        return {
          appName: "NEXØRA",
          version: "0.1.0",
          engineReady: true,
          modules: ["scanner", "detectors", "cleaner"],
        };
      }
      return undefined;
    });

    render(<App />);

    expect(await screen.findByText("MODO AISLADO")).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "Verificar motor" }));

    expect(await screen.findByText("MOTOR ACTIVO")).toBeInTheDocument();
    expect(calls).toBe(2);
  });
});
