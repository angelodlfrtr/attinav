import { useEffect, useRef, useState } from "react";
import { api, toError } from "../../api";
import { describeMessage, type HostMessage } from "../../protocol";
import { useAppDispatch, useAppSelector } from "../../store";
import { consoleActions } from "../../store/consoleSlice";
import { toast } from "../../store/uiSlice";
import { clockTime } from "../../util/format";
import { Badge, Card, Toggle } from "../ui";

const EXAMPLES: { label: string; msg: HostMessage }[] = [
  { label: "Ping", msg: { kind: "Ping" } },
  { label: "GetInfo", msg: { kind: "GetInfo" } },
  { label: "GetConfig", msg: { kind: "GetConfig" } },
  { label: "CalInfo", msg: { kind: "CalInfo" } },
  { label: "Shot (raw)", msg: { kind: "Shot", data: { flags: 1, mag_samples: 0, cycle_count: 0 } } },
  { label: "Abort", msg: { kind: "Abort" } },
];

export function Console() {
  const dispatch = useAppDispatch();
  const connected = useAppSelector((s) => s.link.connected);
  const con = useAppSelector((s) => s.console);
  const [draft, setDraft] = useState(JSON.stringify(EXAMPLES[1]?.msg ?? { kind: "GetInfo" }));
  const [timeout, setTimeoutMs] = useState(5000);
  const [follow, setFollow] = useState(true);
  const endRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (follow) endRef.current?.scrollIntoView({ block: "end" });
  }, [con.entries.length, follow]);

  const send = async () => {
    let msg: HostMessage;
    try {
      msg = JSON.parse(draft) as HostMessage;
    } catch (e) {
      dispatch(toast({ level: "error", text: `not JSON: ${toError(e).message}` }));
      return;
    }
    dispatch(consoleActions.logged({ direction: "tx", seq: 0, kind: msg.kind, summary: `→ ${msg.kind}`, json: JSON.stringify("data" in msg ? msg.data : null) }));
    try {
      const replies = await api.request(msg, timeout);
      dispatch(consoleActions.logged({ direction: "sys", seq: 0, kind: "reply", summary: `${replies.length} frame(s): ${replies.map(describeMessage).join(" | ")}`, json: "" }));
    } catch (e) {
      dispatch(toast({ level: "error", text: toError(e).message }));
    }
  };

  const filter = con.filter.trim().toLowerCase();
  const entries = filter ? con.entries.filter((e) => e.kind.toLowerCase().includes(filter) || e.summary.toLowerCase().includes(filter)) : con.entries;

  return (
    <div className="flex h-full flex-col gap-4">
      <Card title="Send a request">
        <div className="flex flex-wrap items-center gap-2">
          {EXAMPLES.map((ex) => (
            <button key={ex.label} className="btn btn-sm" onClick={() => setDraft(JSON.stringify(ex.msg))}>
              {ex.label}
            </button>
          ))}
        </div>
        <div className="mt-2 flex gap-2">
          <input className="input num flex-1" value={draft} onChange={(e) => setDraft(e.target.value)} spellCheck={false} />
          <input className="input num w-24" type="number" value={timeout} min={100} onChange={(e) => setTimeoutMs(Number(e.target.value))} title="timeout, ms" />
          <button className="btn btn-primary" disabled={!connected} onClick={() => void send()}>
            Send
          </button>
        </div>
        <p className="mt-1 text-xs text-ink-3">
          A <code>HostMessage</code> as JSON: <code className="num">{'{"kind":"SetTime","data":1700000000}'}</code>. Replies are logged below.
        </p>
      </Card>

      <Card
        title={`Frames (${entries.length})`}
        className="flex min-h-0 flex-1 flex-col"
        actions={
          <>
            <input className="input w-48 py-1 text-xs" placeholder="filter kind / text" value={con.filter} onChange={(e) => dispatch(consoleActions.filterChanged(e.target.value))} />
            <Toggle label="hide samples" checked={con.hideSamples} onChange={(v) => dispatch(consoleActions.hideSamples(v))} />
            <Toggle label="follow" checked={follow} onChange={setFollow} />
            <Toggle label="pause" checked={con.paused} onChange={(v) => dispatch(consoleActions.paused(v))} />
            <button className="btn btn-sm" onClick={() => dispatch(consoleActions.cleared())}>
              Clear
            </button>
          </>
        }
      >
        <div className="mb-2 flex flex-wrap gap-1">
          {Object.entries(con.counts).map(([k, v]) => (
            <Badge key={k} tone="neutral">
              {k} <span className="num ml-1 text-ink">{v}</span>
            </Badge>
          ))}
        </div>
        <div className="min-h-0 flex-1 overflow-auto rounded border border-line bg-ground">
          <table className="table">
            <tbody>
              {entries.map((e) => (
                <tr key={e.id} title={e.json}>
                  <td className="num w-24 text-ink-3">{clockTime(e.at)}</td>
                  <td className="w-10">
                    <Badge tone={e.direction === "rx" ? "accent" : e.direction === "tx" ? "ok" : "neutral"}>{e.direction}</Badge>
                  </td>
                  <td className="num w-10 text-ink-3">{e.seq || ""}</td>
                  <td className="w-28 font-medium">{e.kind}</td>
                  <td className="num text-ink-2">{e.summary}</td>
                </tr>
              ))}
            </tbody>
          </table>
          <div ref={endRef} />
        </div>
      </Card>
    </div>
  );
}
