import { api, toError } from "../api";
import { useAppDispatch, useAppSelector } from "../store";
import { baudChanged, connecting, portSelected } from "../store/linkSlice";
import { refreshPorts } from "../listeners";
import { store } from "../store";
import { toast } from "../store/uiSlice";
import { Spinner } from "./ui";

export function ConnectionBar() {
  const dispatch = useAppDispatch();
  const link = useAppSelector((s) => s.link);

  const connectSerial = async () => {
    if (!link.selectedPort) return;
    dispatch(connecting(true));
    try {
      await api.connect({ kind: "serial", path: link.selectedPort, baud: link.baud });
    } catch (e) {
      dispatch(connecting(false));
      dispatch(toast({ level: "error", text: toError(e).message }));
    }
  };
  const connectSim = async () => {
    dispatch(connecting(true));
    try {
      await api.connect({ kind: "simulator" });
    } catch (e) {
      dispatch(connecting(false));
      dispatch(toast({ level: "error", text: toError(e).message }));
    }
  };
  const disconnect = async () => {
    try {
      await api.disconnect();
    } catch (e) {
      dispatch(toast({ level: "error", text: toError(e).message }));
    }
  };

  return (
    <header className="flex h-12 items-center gap-3 border-b border-line bg-panel px-4">
      <div className="flex items-center gap-2">
        <span className={`inline-block h-2.5 w-2.5 rounded-full ${link.connected ? "bg-ok" : "bg-ink-3"}`} />
        <span className="text-sm">
          {link.connected ? (
            <>
              <span className="text-ink-2">connected to </span>
              <span className="num">{link.description}</span>
              {link.simulator && <span className="ml-2 badge bg-warn/15 text-warn">simulator</span>}
            </>
          ) : (
            <span className="text-ink-2">not connected</span>
          )}
        </span>
      </div>
      <div className="flex-1" />
      {link.connected ? (
        <button className="btn btn-sm" onClick={() => void disconnect()}>
          Disconnect
        </button>
      ) : (
        <>
          <select
            className="input w-64 py-1 text-xs"
            value={link.selectedPort ?? ""}
            onChange={(e) => dispatch(portSelected(e.target.value))}
            disabled={link.ports.length === 0}
          >
            {link.ports.length === 0 && <option value="">no serial ports</option>}
            {link.ports.map((p) => (
              <option key={p.path} value={p.path}>
                {p.path}
                {p.is_attinav ? " — Attinav" : p.product ? ` — ${p.product}` : ""}
                {p.serial ? ` (${p.serial})` : ""}
              </option>
            ))}
          </select>
          <select className="input w-28 py-1 text-xs num" value={link.baud} onChange={(e) => dispatch(baudChanged(Number(e.target.value)))} title="UART baud rate (ignored over USB)">
            {[115200, 230400, 460800, 921600, 1000000, 2000000].map((b) => (
              <option key={b} value={b}>
                {b}
              </option>
            ))}
          </select>
          <button className="btn btn-sm" onClick={() => void refreshPorts(store)} title="Rescan ports">
            ↻
          </button>
          <button className="btn btn-sm btn-primary" disabled={!link.selectedPort || link.connecting} onClick={() => void connectSerial()}>
            {link.connecting ? <Spinner /> : null} Connect
          </button>
          <button className="btn btn-sm" disabled={link.connecting} onClick={() => void connectSim()} title="Connect to the built-in simulated device">
            Simulator
          </button>
        </>
      )}
    </header>
  );
}
