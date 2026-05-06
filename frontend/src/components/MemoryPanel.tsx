// Stacked-bar memory breakdown for the RSS tile drill-down. Operators
// looking at a climbing RSS need to see *where* it climbs — internal
// caches, jemalloc fragmentation, kernel/lib resident, stacks. The bar
// adds up to RSS; cgroup limits are displayed alongside as a "headroom"
// indicator.

import { useEffect, useState } from "react";
import { apiGet } from "../api";
import { useAdminAuth } from "../hooks/useAdminAuth";
import type { MemoryBreakdownDto } from "../types";

const POLL_MS = 5000;

export function MemoryPanel({ open, onClose }: { open: boolean; onClose: () => void }) {
  const { authHeader } = useAdminAuth();
  const [data, setData] = useState<MemoryBreakdownDto | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!open) return;
    let cancelled = false;
    const ctrl = new AbortController();
    const tick = () => {
      apiGet<MemoryBreakdownDto>("/api/process/memory", authHeader, ctrl.signal)
        .then((d) => {
          if (cancelled) return;
          setData(d);
          setError(null);
        })
        .catch((e: unknown) => {
          if (cancelled) return;
          if (e instanceof DOMException && e.name === "AbortError") return;
          setError(e instanceof Error ? e.message : String(e));
        });
    };
    tick();
    const id = window.setInterval(tick, POLL_MS);
    return () => {
      cancelled = true;
      ctrl.abort();
      window.clearInterval(id);
    };
  }, [open, authHeader]);

  useEffect(() => {
    if (!open) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [open, onClose]);

  if (!open) return null;

  return (
    <div
      className="fixed inset-0 z-40 flex items-center justify-center bg-bg/85 backdrop-blur-sm"
      onClick={onClose}
    >
      <div
        className="m-6 flex h-[90vh] w-[min(96vw,1200px)] flex-col border border-border bg-surface"
        onClick={(e) => e.stopPropagation()}
      >
        <header className="flex items-center justify-between border-b border-border px-4 py-3">
          <div>
            <div className="text-[10px] uppercase tracking-[0.2em] text-text-dim">panel</div>
            <h2 className="font-mono text-base font-semibold text-text">Process memory breakdown</h2>
          </div>
          <button
            type="button"
            onClick={onClose}
            className="border border-border-strong px-2 py-0.5 text-xs font-mono uppercase tracking-wider text-text-muted hover:text-accent"
            title="Close (Esc)"
          >
            ✕
          </button>
        </header>
        <div className="flex-1 overflow-auto p-6 text-sm">
          {error && <p className="text-danger">memory fetch failed: {error}</p>}
          {!data && !error && <p className="text-text-dim">loading…</p>}
          {data && <Body data={data} />}
        </div>
      </div>
    </div>
  );
}

function Body({ data }: { data: MemoryBreakdownDto }) {
  const fmt = (n: number) => {
    if (n < 1024) return `${n} B`;
    if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KiB`;
    if (n < 1024 * 1024 * 1024) return `${(n / 1024 / 1024).toFixed(1)} MiB`;
    return `${(n / 1024 / 1024 / 1024).toFixed(2)} GiB`;
  };
  const fmtOpt = (n: number | null) => (n === null ? "—" : fmt(n));

  const total = data.categories.reduce((s, c) => s + c.bytes, 0);
  const palette: Record<string, string> = {
    app_caches: "rgb(255 176 0)",
    jemalloc_live: "rgb(0 212 255)",
    jemalloc_fragmentation: "rgb(154 148 133)",
    code_and_libs: "rgb(57 211 83)",
    stacks_and_pagetables: "rgb(177 140 245)",
    swap: "rgb(255 77 77)",
    other: "rgb(91 140 255)",
  };

  return (
    <div className="space-y-6">
      <div className="flex items-baseline justify-between border-b border-border pb-3">
        <div>
          <div className="text-[10px] uppercase tracking-[0.2em] text-text-dim">RSS</div>
          <div className="font-mono text-3xl font-semibold tabular text-text">
            {fmt(data.rss_bytes)}
          </div>
        </div>
        {data.cgroup && (
          <div className="text-right">
            <div className="text-[10px] uppercase tracking-[0.2em] text-text-dim">
              cgroup v{data.cgroup.version}
            </div>
            <div className="font-mono text-base text-text">
              {fmt(data.cgroup.current_bytes)} /{" "}
              {data.cgroup.max_bytes !== null ? fmt(data.cgroup.max_bytes) : "uncapped"}
            </div>
            {data.cgroup.peak_bytes !== null && (
              <div className="text-xs text-text-dim">peak {fmt(data.cgroup.peak_bytes)}</div>
            )}
          </div>
        )}
      </div>

      <section>
        <div className="mb-2 text-[10px] uppercase tracking-[0.2em] text-text-dim">
          breakdown
        </div>
        <div className="flex h-8 w-full overflow-hidden border border-border bg-bg">
          {data.categories.map((c) => {
            const pct = total > 0 ? (c.bytes / total) * 100 : 0;
            return (
              <div
                key={c.key}
                title={`${c.label}: ${fmt(c.bytes)} · ${pct.toFixed(1)}% of attributed RSS\n${c.explain}`}
                style={{
                  width: `${pct}%`,
                  background: palette[c.key] ?? "rgb(154 148 133)",
                }}
              />
            );
          })}
        </div>
        <table className="mt-3 w-full text-xs tabular">
          <tbody>
            {data.categories.map((c) => (
              <tr key={c.key} className="border-b border-border/40">
                <td className="py-1">
                  <span className="inline-flex items-center gap-2">
                    <span
                      className="inline-block h-2 w-2"
                      style={{ background: palette[c.key] ?? "rgb(154 148 133)" }}
                    />
                    {c.label}
                  </span>
                </td>
                <td className="py-1 text-right font-mono">{fmt(c.bytes)}</td>
                <td className="py-1 pl-4 text-text-dim">{c.explain}</td>
              </tr>
            ))}
          </tbody>
        </table>
      </section>

      {data.jemalloc && (
        <section>
          <div className="mb-2 text-[10px] uppercase tracking-[0.2em] text-text-dim">
            jemalloc
          </div>
          <table className="w-full text-xs tabular">
            <tbody>
              <Row k="allocated" v={fmt(data.jemalloc.allocated_bytes)} />
              <Row k="active" v={fmt(data.jemalloc.active_bytes)} />
              <Row k="resident" v={fmt(data.jemalloc.resident_bytes)} />
              <Row k="mapped" v={fmt(data.jemalloc.mapped_bytes)} />
              <Row k="retained" v={fmt(data.jemalloc.retained_bytes)} />
              <Row k="metadata" v={fmt(data.jemalloc.metadata_bytes)} />
              <Row
                k="fragmentation (resident − allocated)"
                v={fmt(data.jemalloc.fragmentation_bytes)}
              />
            </tbody>
          </table>
        </section>
      )}

      <section>
        <div className="mb-2 text-[10px] uppercase tracking-[0.2em] text-text-dim">
          /proc/self/status
        </div>
        <table className="w-full text-xs tabular">
          <tbody>
            <Row k="VmPeak (lifetime)" v={fmtOpt(data.vm_peak_bytes)} />
            <Row k="VmHWM (resident peak)" v={fmtOpt(data.vm_hwm_bytes)} />
            <Row k="VmData" v={fmtOpt(data.vm_data_bytes)} />
            <Row k="VmStk" v={fmtOpt(data.vm_stack_bytes)} />
            <Row k="VmExe" v={fmtOpt(data.vm_exe_bytes)} />
            <Row k="VmLib" v={fmtOpt(data.vm_lib_bytes)} />
            <Row k="VmPTE (page tables)" v={fmtOpt(data.vm_pte_bytes)} />
            <Row k="VmSwap" v={fmtOpt(data.vm_swap_bytes)} />
            <Row k="RssAnon" v={fmtOpt(data.rss_anon_bytes)} />
            <Row k="RssFile" v={fmtOpt(data.rss_file_bytes)} />
            <Row k="RssShmem" v={fmtOpt(data.rss_shmem_bytes)} />
          </tbody>
        </table>
      </section>

      <section>
        <div className="mb-2 text-[10px] uppercase tracking-[0.2em] text-text-dim">
          pg_doorman caches
        </div>
        <table className="w-full text-xs tabular">
          <tbody>
            <Row k="interner named" v={fmt(data.interner_named_bytes)} />
            <Row k="interner anonymous" v={fmt(data.interner_anonymous_bytes)} />
          </tbody>
        </table>
      </section>
    </div>
  );
}

function Row({ k, v }: { k: string; v: string }) {
  return (
    <tr className="border-b border-border/40">
      <td className="py-1 text-text-muted">{k}</td>
      <td className="py-1 text-right font-mono">{v}</td>
    </tr>
  );
}
