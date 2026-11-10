import { useEffect } from "react";
import { useAppDispatch, useAppSelector } from "../store";
import { dismissToast, navigate, type Page } from "../store/uiSlice";
import { ConnectionBar } from "./ConnectionBar";
import { Dashboard } from "./pages/Dashboard";
import { Shots } from "./pages/Shots";
import { Stream } from "./pages/Stream";
import { Calibration } from "./pages/Calibration";
import { Config } from "./pages/Config";
import { Firmware } from "./pages/Firmware";
import { Console } from "./pages/Console";
import { Simulator } from "./pages/Simulator";

const NAV: { page: Page; label: string; hint: string }[] = [
  { page: "dashboard", label: "Dashboard", hint: "identity, sensors, clock" },
  { page: "shots", label: "Shots", hint: "take and inspect measurements" },
  { page: "stream", label: "Stream", hint: "raw samples, live" },
  { page: "calibration", label: "Calibration", hint: "56-shot procedure, solve, store" },
  { page: "config", label: "Configuration", hint: "thresholds, timing, UART" },
  { page: "firmware", label: "Firmware", hint: "over-the-air update" },
  { page: "console", label: "Console", hint: "every frame, raw requests" },
];

export function App() {
  const dispatch = useAppDispatch();
  const page = useAppSelector((s) => s.ui.page);
  const toasts = useAppSelector((s) => s.ui.toasts);
  const link = useAppSelector((s) => s.link);

  useEffect(() => {
    if (toasts.length === 0) return;
    const first = toasts[0];
    if (!first) return;
    const t = setTimeout(() => dispatch(dismissToast(first.id)), first.level === "error" ? 8000 : 3500);
    return () => {
      clearTimeout(t);
    };
  }, [toasts, dispatch]);

  const nav = link.simulator ? [...NAV, { page: "simulator" as Page, label: "Simulator", hint: "pose, noise, faults" }] : NAV;

  return (
    <div className="flex h-full flex-col">
      <ConnectionBar />
      <div className="flex min-h-0 flex-1">
        <nav className="flex w-52 shrink-0 flex-col border-r border-line bg-panel">
          <div className="px-4 py-4">
            <div className="text-lg font-semibold tracking-tight">Attinav</div>
            <div className="text-xs text-ink-3">survey module client{link.appInfo ? ` · v${link.appInfo.version}` : ""}</div>
          </div>
          <ul className="flex-1">
            {nav.map((n) => (
              <li key={n.page}>
                <button
                  className={`flex w-full flex-col items-start px-4 py-2 text-left transition-colors ${
                    page === n.page ? "border-r-2 border-accent bg-panel-2 text-ink" : "text-ink-2 hover:bg-panel-2 hover:text-ink"
                  }`}
                  onClick={() => dispatch(navigate(n.page))}
                >
                  <span className="text-sm font-medium">{n.label}</span>
                  <span className="text-[11px] text-ink-3">{n.hint}</span>
                </button>
              </li>
            ))}
          </ul>
          <div className="px-4 py-3 text-[11px] text-ink-3">
            protocol v{link.appInfo?.protocol_version ?? "?"} · core fw {link.appInfo?.core_firmware_version.join(".") ?? "?"}
          </div>
        </nav>
        <main className="min-w-0 flex-1 overflow-auto p-5">
          {page === "dashboard" && <Dashboard />}
          {page === "shots" && <Shots />}
          {page === "stream" && <Stream />}
          {page === "calibration" && <Calibration />}
          {page === "config" && <Config />}
          {page === "firmware" && <Firmware />}
          {page === "console" && <Console />}
          {page === "simulator" && <Simulator />}
        </main>
      </div>
      <div className="pointer-events-none fixed bottom-4 right-4 flex w-96 flex-col gap-2">
        {toasts.map((t) => (
          <div
            key={t.id}
            className={`pointer-events-auto rounded-md border px-3 py-2 text-sm shadow-lg ${
              t.level === "error" ? "border-bad/50 bg-[#2a1416] text-bad" : t.level === "success" ? "border-ok/50 bg-[#102a22] text-ok" : "border-line bg-panel-2 text-ink"
            }`}
            onClick={() => dispatch(dismissToast(t.id))}
          >
            {t.text}
          </div>
        ))}
      </div>
    </div>
  );
}
