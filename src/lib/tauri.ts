import { invoke } from "@tauri-apps/api/core";

export interface AppStatus {
  appName: string;
  version: string;
  engineReady: boolean;
  modules: string[];
}

export async function fetchAppStatus(): Promise<AppStatus> {
  return invoke<AppStatus>("get_app_status");
}
