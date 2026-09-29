import { useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import type {
  Settings,
  RotationMode,
  Interval,
  FitMode,
  MinResolution,
  MonitorInfo,
} from "../lib/types";
import {
  INTERVAL_LABELS,
  ROTATION_MODE_LABELS,
  FIT_MODE_LABELS,
  MIN_RESOLUTION_LABELS,
} from "../lib/types";

const DEFAULT_SETTINGS: Settings = {
  stash_url: "",
  api_key: "",
  query_filter: JSON.stringify({ image_filter: {}, filter: {} }, null, 2),
  rotation_mode: "random",
  interval: "thirty_minutes",
  fit_mode: "crop",
  min_resolution: "none",
  per_monitor: false,
};

type ConnectionStatus =
  | { state: "idle" }
  | { state: "testing" }
  | { state: "connected" }
  | { state: "failed"; error: string };
type TestQueryResult =
  | { status: "idle" }
  | { status: "testing" }
  | { status: "success"; count: number }
  | { status: "zero" }
  | { status: "error"; error: string };

function SelectWrapper({ children }: { children: React.ReactNode }) {
  return (
    <div className="relative">
      {children}
      <div className="pointer-events-none absolute inset-y-0 right-0 flex items-center pr-2">
        <svg
          className="h-4 w-4 text-zinc-400"
          fill="none"
          stroke="currentColor"
          viewBox="0 0 24 24"
        >
          <path
            strokeLinecap="round"
            strokeLinejoin="round"
            strokeWidth={2}
            d="M19 9l-7 7-7-7"
          />
        </svg>
      </div>
    </div>
  );
}

export default function SettingsPanel() {
  const [settings, setSettings] = useState<Settings>(DEFAULT_SETTINGS);
  const [connection, setConnection] = useState<ConnectionStatus>({
    state: "idle",
  });
  const [loadWarning, setLoadWarning] = useState<string | null>(null);
  const [saved, setSaved] = useState(false);
  const [saving, setSaving] = useState(false);
  // Bumped whenever the URL or key changes, so a slow Test Connection result
  // for the old values is dropped
  const connectionAttempt = useRef(0);
  const [saveError, setSaveError] = useState<string | null>(null);
  const [monitors, setMonitors] = useState<MonitorInfo[]>([]);
  // How this desktop handles a different image per monitor
  const [perMonitorSupport, setPerMonitorSupport] = useState<
    "native" | "spanned" | "unsupported" | null
  >(null);
  // null until we know; stays null if the platform can't report it
  const [autostart, setAutostart] = useState<boolean | null>(null);
  const [autostartError, setAutostartError] = useState<string | null>(null);
  const [autostartBusy, setAutostartBusy] = useState(false);
  const [testResult, setTestResult] = useState<TestQueryResult>({
    status: "idle",
  });

  useEffect(() => {
    invoke<Settings>("get_settings")
      .then((loaded) => {
        setSettings(loaded);
      })
      .catch(() => {
        // Use defaults if no settings loaded yet
      });
    invoke<MonitorInfo[]>("detect_monitors")
      .then(setMonitors)
      .catch(() => {});
    invoke<"native" | "spanned" | "unsupported">("desktop_per_monitor")
      .then(setPerMonitorSupport)
      .catch(() => {});
    invoke<boolean>("get_autostart")
      .then(setAutostart)
      .catch((err) =>
        setAutostartError(`Couldn't check start at login: ${String(err)}`),
      );
    invoke<string | null>("settings_load_warning")
      .then(setLoadWarning)
      .catch(() => {});
    // Tells the log (and the CI smoke test) the window rendered
    invoke("window_ready").catch(() => {});
  }, []);

  // Applies immediately: it's an OS setting, not part of settings.json
  async function toggleAutostart(enabled: boolean) {
    setAutostartError(null);
    setAutostartBusy(true);
    try {
      await invoke("set_autostart", { enabled });
      setAutostart(enabled);
    } catch (err) {
      setAutostartError(`Couldn't change start at login: ${String(err)}`);
    } finally {
      setAutostartBusy(false);
    }
  }

  function update<K extends keyof Settings>(key: K, value: Settings[K]) {
    setSettings((prev) => ({ ...prev, [key]: value }));
    setSaveError(null);
    if (key === "stash_url" || key === "api_key") {
      connectionAttempt.current += 1;
      setConnection({ state: "idle" });
    }
    // Reset test result when query-affecting fields change
    if (key === "query_filter" || key === "min_resolution") {
      setTestResult({ status: "idle" });
    }
  }

  async function testConnection() {
    const attempt = ++connectionAttempt.current;
    setConnection({ state: "testing" });
    try {
      await invoke("test_connection", {
        url: settings.stash_url,
        apiKey: settings.api_key,
      });
      if (attempt === connectionAttempt.current) {
        setConnection({ state: "connected" });
      }
    } catch (err) {
      if (attempt === connectionAttempt.current) {
        setConnection({ state: "failed", error: String(err) });
      }
    }
  }

  async function saveSettings() {
    setSaveError(null);
    setSaving(true);
    try {
      // The backend normalizes the Server URL; show what was saved
      const stored = await invoke<Settings>("save_settings", {
        newSettings: settings,
      });
      setSettings(stored);
      setLoadWarning(null);
      setSaved(true);
      setTimeout(() => setSaved(false), 2000);
    } catch (err) {
      setSaveError(String(err));
    } finally {
      setSaving(false);
    }
  }

  async function testQuery() {
    setTestResult({ status: "testing" });
    try {
      const count = await invoke<number>("test_query", {
        newSettings: settings,
      });
      if (count > 0) {
        setTestResult({ status: "success", count });
      } else {
        setTestResult({ status: "zero" });
      }
    } catch (err) {
      setTestResult({
        status: "error",
        error: String(err),
      });
    }
  }

  // Mirrors parse_query_filter in stash.rs, which has the final say on save
  const queryFilterError = useMemo(() => {
    const raw = settings.query_filter.trim();
    if (!raw) return null;
    let parsed: unknown;
    try {
      parsed = JSON.parse(raw);
    } catch (e) {
      return `Not valid JSON (${(e as SyntaxError).message})`;
    }
    const isObject = (v: unknown) =>
      typeof v === "object" && v !== null && !Array.isArray(v);
    if (!isObject(parsed)) {
      return "Must be a JSON object with filter and/or image_filter";
    }
    for (const [key, value] of Object.entries(parsed as object)) {
      if (key !== "filter" && key !== "image_filter") {
        return `Unknown key "${key}", use filter and/or image_filter`;
      }
      if (!isObject(value)) return `"${key}" must be a JSON object`;
    }
    return null;
  }, [settings.query_filter]);

  // The filter's own sort, if it sets one, for the notes under Mode
  const filterSort = useMemo(() => {
    try {
      const sort = JSON.parse(settings.query_filter)?.filter?.sort;
      return typeof sort === "string" ? sort : null;
    } catch {
      return null;
    }
  }, [settings.query_filter]);
  const randomSort = filterSort !== null && filterSort.startsWith("random");
  const modeNote =
    settings.rotation_mode === "random" && filterSort !== null && !randomSort
      ? `Your filter sorts by "${filterSort}", so images follow that order. Remove the sort from the filter for a random order.`
      : settings.rotation_mode !== "random" && randomSort
        ? "Your filter sorts randomly. StashPaper fixes that order once, so this mode doesn't repeat images."
        : null;
  // Per-monitor images are cropped to fill their monitor, spanned or native
  const fitIgnored =
    settings.per_monitor &&
    monitors.length > 1 &&
    (perMonitorSupport === "spanned" || perMonitorSupport === "native");

  const inputClass =
    "w-full rounded bg-zinc-800 border border-zinc-700 px-3 py-2 text-zinc-100 placeholder-zinc-500 focus:outline-none focus:ring-2 focus:ring-blue-500";
  const selectClass =
    "w-full appearance-none rounded bg-zinc-800 border border-zinc-700 px-3 py-2 pr-8 text-zinc-100 focus:outline-none focus:ring-2 focus:ring-blue-500";
  const labelClass = "block text-sm font-medium text-zinc-400 mb-1";
  const sectionClass = "space-y-3";
  const headingClass = "text-lg font-semibold text-zinc-200";

  return (
    <div className="min-h-screen bg-zinc-900 text-zinc-100 p-6">
      <div className="mx-auto max-w-lg space-y-6">
        <h1 className="text-2xl font-bold">StashPaper Settings</h1>

        {loadWarning && (
          <p className="rounded border border-amber-600 bg-amber-950 p-3 text-sm text-amber-200">
            {loadWarning}
          </p>
        )}

        {/* Stash Connection */}
        <section className={sectionClass}>
          <h2 className={headingClass}>Stash Connection</h2>
          <div>
            <label htmlFor="stash-url" className={labelClass}>
              Server URL
            </label>
            <input
              id="stash-url"
              type="text"
              className={inputClass}
              value={settings.stash_url}
              onChange={(e) => update("stash_url", e.target.value)}
              placeholder="http://localhost:9999"
            />
          </div>
          <div>
            <label htmlFor="api-key" className={labelClass}>
              API Key (optional)
            </label>
            <input
              id="api-key"
              type="password"
              className={inputClass}
              value={settings.api_key}
              onChange={(e) => update("api_key", e.target.value)}
              placeholder="Only needed if your Stash has a login"
            />
          </div>
          <div className="flex items-center gap-3">
            <button
              type="button"
              onClick={testConnection}
              disabled={connection.state === "testing" || !settings.stash_url.trim()}
              title={settings.stash_url.trim() ? undefined : "Enter the Server URL first"}
              className="rounded bg-zinc-700 px-4 py-2 text-sm font-medium text-zinc-100 hover:bg-zinc-600 disabled:opacity-50 disabled:cursor-not-allowed"
            >
              {connection.state === "testing"
                ? "Testing..."
                : "Test Connection"}
            </button>
            <span role="status" className="min-w-0 text-sm">
              {connection.state === "connected" && (
                <span className="text-green-400">Connected</span>
              )}
              {connection.state === "failed" && (
                <span className="text-red-400">{connection.error}</span>
              )}
            </span>
          </div>
        </section>

        {/* Query Filter */}
        <section className={sectionClass}>
          <h2 className={headingClass}>
            <label htmlFor="query-filter">Query Filter</label>
          </h2>
          <p className="text-sm text-zinc-400">
            JSON with <code className="rounded bg-zinc-800 px-1">filter</code>{" "}
            (sort, direction) and{" "}
            <code className="rounded bg-zinc-800 px-1">image_filter</code>{" "}
            (tags, resolution, rating, etc.) from Stash's GraphQL Playground.
          </p>
          <textarea
            id="query-filter"
            className={`${inputClass} font-mono text-xs ${queryFilterError ? "border-red-500 focus:ring-red-500" : ""}`}
            rows={8}
            spellCheck={false}
            value={settings.query_filter}
            onChange={(e) => update("query_filter", e.target.value)}
          />
          {queryFilterError ? (
            <p className="text-xs text-red-400 mt-1">{queryFilterError}</p>
          ) : settings.query_filter.trim() ? (
            <p className="text-xs text-green-400 mt-1">Filter format OK</p>
          ) : null}
          <div className="flex items-center gap-3">
            <button
              type="button"
              onClick={testQuery}
              disabled={
                !!queryFilterError ||
                testResult.status === "testing" ||
                !settings.stash_url.trim()
              }
              title={settings.stash_url.trim() ? undefined : "Enter the Server URL first"}
              className="rounded bg-zinc-700 px-4 py-2 text-sm font-medium text-zinc-100 hover:bg-zinc-600 disabled:opacity-50 disabled:cursor-not-allowed"
            >
              {testResult.status === "testing"
                ? "Testing..."
                : "Test Query"}
            </button>
            {testResult.status === "success" && (
              <span className="text-sm text-green-400">
                Found {testResult.count} image{testResult.count !== 1 ? "s" : ""}
              </span>
            )}
            {testResult.status === "zero" && (
              <span className="text-sm text-red-400">
                No images match. Check the filter and minimum resolution
              </span>
            )}
            {testResult.status === "error" && (
              <span className="text-sm text-red-400">
                {testResult.error}
              </span>
            )}
          </div>
        </section>

        {/* Rotation */}
        <section className={sectionClass}>
          <h2 className={headingClass}>Rotation</h2>
          <div className="grid grid-cols-2 gap-4">
            <div>
              <label htmlFor="rotation-mode" className={labelClass}>
                Mode
              </label>
              <SelectWrapper>
                <select
                  id="rotation-mode"
                  className={selectClass}
                  style={{ color: "#f4f4f5", backgroundColor: "#27272a" }}
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
                    <option key={value} value={value} style={{ color: "#f4f4f5", backgroundColor: "#27272a" }}>
                      {label}
                    </option>
                  ))}
                </select>
              </SelectWrapper>
            </div>
            <div>
              <label htmlFor="interval" className={labelClass}>
                Interval
              </label>
              <SelectWrapper>
                <select
                  id="interval"
                  className={selectClass}
                  style={{ color: "#f4f4f5", backgroundColor: "#27272a" }}
                  value={settings.interval}
                  onChange={(e) =>
                    update("interval", e.target.value as Interval)
                  }
                >
                  {(
                    Object.entries(INTERVAL_LABELS) as [Interval, string][]
                  ).map(([value, label]) => (
                    <option key={value} value={value} style={{ color: "#f4f4f5", backgroundColor: "#27272a" }}>
                      {label}
                    </option>
                  ))}
                </select>
              </SelectWrapper>
            </div>
          </div>
          {modeNote && <p className="text-xs text-zinc-400">{modeNote}</p>}
        </section>

        {/* Display */}
        <section className={sectionClass}>
          <h2 className={headingClass}>Display</h2>
          <div>
            <label htmlFor="min-resolution" className={labelClass}>
              Minimum Resolution
            </label>
            <SelectWrapper>
              <select
                id="min-resolution"
                className={selectClass}
                style={{ color: "#f4f4f5", backgroundColor: "#27272a" }}
                value={settings.min_resolution}
                onChange={(e) =>
                  update("min_resolution", e.target.value as MinResolution)
                }
              >
                {(
                  Object.entries(MIN_RESOLUTION_LABELS) as [
                    MinResolution,
                    string,
                  ][]
                ).map(([value, label]) => (
                  <option
                    key={value}
                    value={value}
                    style={{ color: "#f4f4f5", backgroundColor: "#27272a" }}
                  >
                    {label}
                  </option>
                ))}
              </select>
            </SelectWrapper>
            {monitors.length > 0 && (
              <p className="text-xs text-zinc-500 mt-1">
                {monitors.length === 1
                  ? `Your monitor: ${monitors[0].width}x${monitors[0].height}`
                  : `Monitors: ${monitors.map((m) => `${m.width}x${m.height}`).join(", ")}`}
              </p>
            )}
          </div>
          <div>
            <label htmlFor="fit-mode" className={labelClass}>
              Fit Mode
            </label>
            <SelectWrapper>
              <select
                id="fit-mode"
                disabled={fitIgnored}
                className={`${selectClass} disabled:opacity-50`}
                style={{ color: "#f4f4f5", backgroundColor: "#27272a" }}
                value={settings.fit_mode}
                onChange={(e) =>
                  update("fit_mode", e.target.value as FitMode)
                }
              >
                {(
                  Object.entries(FIT_MODE_LABELS) as [FitMode, string][]
                ).map(([value, label]) => (
                  <option key={value} value={value} style={{ color: "#f4f4f5", backgroundColor: "#27272a" }}>
                    {label}
                  </option>
                ))}
              </select>
            </SelectWrapper>
            {fitIgnored && (
              <p className="text-xs text-zinc-500 mt-1">
                Fit Mode doesn't apply to per-monitor wallpapers: each image is
                cropped to fill its own monitor.
              </p>
            )}
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
          </label>
          {settings.per_monitor && monitors.length > 1 && perMonitorSupport === "unsupported" && (
            <p className="text-xs text-zinc-500 ml-6">
              This desktop can't show a different image per monitor, so every
              monitor gets the same one.
            </p>
          )}
          {monitors.length > 1 && settings.per_monitor && perMonitorSupport !== "unsupported" && (
            <p className="text-xs text-zinc-500 ml-6">
              {monitors.length} monitors detected: {monitors.map((m) => `${m.width}x${m.height}`).join(" + ")}
            </p>
          )}
          {monitors.length <= 1 && settings.per_monitor && (
            <p className="text-xs text-zinc-500 ml-6">
              Only 1 monitor detected, so this has no effect.
            </p>
          )}
        </section>

        {/* Startup */}
        {(autostart !== null || autostartError) && (
          <section className={sectionClass}>
            <h2 className={headingClass}>Startup</h2>
            {autostart !== null && (
              <label className="flex items-center gap-2">
                <input
                  type="checkbox"
                  checked={autostart}
                  disabled={autostartBusy}
                  onChange={(e) => toggleAutostart(e.target.checked)}
                  className="h-4 w-4 rounded border-zinc-600 bg-zinc-800 text-blue-500 focus:ring-blue-500"
                />
                <span className="text-sm text-zinc-300">
                  Start StashPaper in the tray when I log in
                </span>
                <span className="text-xs text-zinc-500">(applies right away)</span>
              </label>
            )}
            {autostartError && <p className="text-xs text-red-400">{autostartError}</p>}
          </section>
        )}

        {/* Save */}
        <button
          type="button"
          onClick={saveSettings}
          disabled={saving || !!queryFilterError || testResult.status === "zero"}
          className="w-full rounded bg-blue-600 px-4 py-2.5 font-medium text-white hover:bg-blue-500 focus:outline-none focus:ring-2 focus:ring-blue-500 focus:ring-offset-2 focus:ring-offset-zinc-900 disabled:opacity-50 disabled:cursor-not-allowed"
        >
          {saved
            ? "Saved!"
            : queryFilterError
              ? "Fix Filter to Save"
              : testResult.status === "zero"
                ? "No Images Found"
                : "Save Settings"}
        </button>
        {saveError && <p className="text-sm text-red-400">{saveError}</p>}
      </div>
    </div>
  );
}
