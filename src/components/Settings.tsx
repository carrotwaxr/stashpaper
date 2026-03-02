import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { Settings, RotationMode, Interval, FitMode } from "../lib/types";
import {
  INTERVAL_LABELS,
  ROTATION_MODE_LABELS,
  FIT_MODE_LABELS,
} from "../lib/types";

const DEFAULT_SETTINGS: Settings = {
  stash_url: "",
  api_key: "",
  image_filter: "{}",
  rotation_mode: "random",
  interval: "thirty_minutes",
  fit_mode: "crop",
  per_monitor: false,
  wifi_only: false,
};

type ConnectionStatus = "idle" | "testing" | "connected" | "failed";

export default function SettingsPanel() {
  const [settings, setSettings] = useState<Settings>(DEFAULT_SETTINGS);
  const [connectionStatus, setConnectionStatus] =
    useState<ConnectionStatus>("idle");
  const [saved, setSaved] = useState(false);

  useEffect(() => {
    invoke<Settings>("get_settings")
      .then((loaded) => {
        setSettings(loaded);
      })
      .catch(() => {
        // Use defaults if no settings loaded yet
      });
  }, []);

  function update<K extends keyof Settings>(key: K, value: Settings[K]) {
    setSettings((prev) => ({ ...prev, [key]: value }));
  }

  async function testConnection() {
    setConnectionStatus("testing");
    try {
      const ok = await invoke<boolean>("test_connection", {
        url: settings.stash_url,
        apiKey: settings.api_key,
      });
      setConnectionStatus(ok ? "connected" : "failed");
    } catch {
      setConnectionStatus("failed");
    }
  }

  async function saveSettings() {
    try {
      await invoke("save_settings", { newSettings: settings });
      setSaved(true);
      setTimeout(() => setSaved(false), 2000);
    } catch (err) {
      console.error("Failed to save settings:", err);
    }
  }

  const inputClass =
    "w-full rounded bg-zinc-800 border border-zinc-700 px-3 py-2 text-zinc-100 placeholder-zinc-500 focus:outline-none focus:ring-2 focus:ring-blue-500";
  const selectClass =
    "w-full rounded bg-zinc-800 border border-zinc-700 px-3 py-2 text-zinc-100 focus:outline-none focus:ring-2 focus:ring-blue-500";
  const labelClass = "block text-sm font-medium text-zinc-400 mb-1";
  const sectionClass = "space-y-3";
  const headingClass = "text-lg font-semibold text-zinc-200";

  return (
    <div className="min-h-screen bg-zinc-900 text-zinc-100 p-6">
      <div className="mx-auto max-w-lg space-y-6">
        <h1 className="text-2xl font-bold">StashPaper Settings</h1>

        {/* Stash Connection */}
        <section className={sectionClass}>
          <h2 className={headingClass}>Stash Connection</h2>
          <div>
            <label className={labelClass}>Server URL</label>
            <input
              type="text"
              className={inputClass}
              value={settings.stash_url}
              onChange={(e) => update("stash_url", e.target.value)}
              placeholder="http://localhost:9999"
            />
          </div>
          <div>
            <label className={labelClass}>API Key</label>
            <input
              type="password"
              className={inputClass}
              value={settings.api_key}
              onChange={(e) => update("api_key", e.target.value)}
              placeholder="Enter API key"
            />
          </div>
          <div className="flex items-center gap-3">
            <button
              type="button"
              onClick={testConnection}
              disabled={connectionStatus === "testing"}
              className="rounded bg-zinc-700 px-4 py-2 text-sm font-medium text-zinc-100 hover:bg-zinc-600 disabled:opacity-50"
            >
              {connectionStatus === "testing"
                ? "Testing..."
                : "Test Connection"}
            </button>
            {connectionStatus === "connected" && (
              <span className="text-sm text-green-400">Connected</span>
            )}
            {connectionStatus === "failed" && (
              <span className="text-sm text-red-400">Connection failed</span>
            )}
          </div>
        </section>

        {/* Image Filter */}
        <section className={sectionClass}>
          <h2 className={headingClass}>Image Filter</h2>
          <p className="text-sm text-zinc-400">
            Paste an ImageFilterType JSON from Stash's GraphQL Playground. Use{" "}
            <code className="rounded bg-zinc-800 px-1">{"{}"}</code> for all
            images.
          </p>
          <textarea
            className={`${inputClass} font-mono`}
            rows={6}
            value={settings.image_filter}
            onChange={(e) => update("image_filter", e.target.value)}
          />
        </section>

        {/* Rotation */}
        <section className={sectionClass}>
          <h2 className={headingClass}>Rotation</h2>
          <div className="grid grid-cols-2 gap-4">
            <div>
              <label className={labelClass}>Mode</label>
              <select
                className={selectClass}
                value={settings.rotation_mode}
                onChange={(e) =>
                  update("rotation_mode", e.target.value as RotationMode)
                }
              >
                {(
                  Object.entries(ROTATION_MODE_LABELS) as [
                    RotationMode,
                    string,
                  ][]
                ).map(([value, label]) => (
                  <option key={value} value={value}>
                    {label}
                  </option>
                ))}
              </select>
            </div>
            <div>
              <label className={labelClass}>Interval</label>
              <select
                className={selectClass}
                value={settings.interval}
                onChange={(e) =>
                  update("interval", e.target.value as Interval)
                }
              >
                {(
                  Object.entries(INTERVAL_LABELS) as [Interval, string][]
                ).map(([value, label]) => (
                  <option key={value} value={value}>
                    {label}
                  </option>
                ))}
              </select>
            </div>
          </div>
        </section>

        {/* Display */}
        <section className={sectionClass}>
          <h2 className={headingClass}>Display</h2>
          <div>
            <label className={labelClass}>Fit Mode</label>
            <select
              className={selectClass}
              value={settings.fit_mode}
              onChange={(e) =>
                update("fit_mode", e.target.value as FitMode)
              }
            >
              {(
                Object.entries(FIT_MODE_LABELS) as [FitMode, string][]
              ).map(([value, label]) => (
                <option key={value} value={value}>
                  {label}
                </option>
              ))}
            </select>
          </div>
          <label className="flex items-center gap-2">
            <input
              type="checkbox"
              checked={settings.per_monitor}
              onChange={(e) => update("per_monitor", e.target.checked)}
              className="h-4 w-4 rounded border-zinc-600 bg-zinc-800 text-blue-500 focus:ring-blue-500"
            />
            <span className="text-sm text-zinc-300">
              Different wallpaper per monitor
            </span>
            <span className="text-xs text-zinc-500">(coming soon)</span>
          </label>
          <p className="text-xs text-zinc-500 ml-6">
            Per-monitor support varies by platform and desktop environment.
          </p>
        </section>

        {/* Network */}
        <section className={sectionClass}>
          <h2 className={headingClass}>Network</h2>
          <label className="flex items-center gap-2">
            <input
              type="checkbox"
              checked={settings.wifi_only}
              onChange={(e) => update("wifi_only", e.target.checked)}
              className="h-4 w-4 rounded border-zinc-600 bg-zinc-800 text-blue-500 focus:ring-blue-500"
            />
            <span className="text-sm text-zinc-300">
              Only rotate when connected to Wi-Fi
            </span>
            <span className="text-xs text-zinc-500">(coming soon)</span>
          </label>
        </section>

        {/* Save */}
        <button
          type="button"
          onClick={saveSettings}
          className="w-full rounded bg-blue-600 px-4 py-2.5 font-medium text-white hover:bg-blue-500 focus:outline-none focus:ring-2 focus:ring-blue-500 focus:ring-offset-2 focus:ring-offset-zinc-900"
        >
          {saved ? "Saved!" : "Save Settings"}
        </button>
      </div>
    </div>
  );
}
