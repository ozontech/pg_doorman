// Stacked-bar memory breakdown for the RSS tile drill-down. Operators
// looking at a climbing RSS need to see *where* it climbs — internal
// caches, jemalloc fragmentation, kernel/lib resident, stacks. The bar
// adds up to RSS; cgroup limits are displayed alongside as a "headroom"
// indicator.

import { useEffect, useState } from "react";
import { apiGet } from "../api";
import { useAdminAuth } from "../hooks/useAdminAuth";
import { tip } from "../lib/tooltips";
import type { MemoryBreakdownDto } from "../types";
import { InfoLabel } from "./InfoLabel";

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

  // Bar widths scale to RSS (the header value), not to the sum of
  // categories. jemalloc.allocated tracks "live allocations" by the
  // allocator's own bookkeeping; the kernel may have reclaimed pages
  // (MADV_DONTNEED, swap) while jemalloc still considers the objects
  // live, so categories can sum to more than the kernel-reported RSS.
  // Without scaling to RSS the bar visually says "100 % of categories"
  // while the header says "RSS = X" — the operator reads two numbers
  // that disagree. Render against `rss_bytes` and clip late segments
  // to the remaining bar width; the table below keeps every category's
  // honest byte count.
  const attributedTotal = data.categories.reduce((s, c) => s + c.bytes, 0);
  const denom = data.rss_bytes > 0 ? data.rss_bytes : attributedTotal;
  let usedPct = 0;
  const segments = data.categories.map((c) => {
    const wanted = denom > 0 ? (c.bytes / denom) * 100 : 0;
    const remaining = Math.max(0, 100 - usedPct);
    const pct = Math.min(wanted, remaining);
    const midPct = usedPct + pct / 2;
    usedPct += pct;
    // Flip the popover's anchor side once the segment sits in the right
    // half of the bar — without this the rightmost categories ("Stacks
    // + page tables", "Other") render their tooltip past the viewport's
    // right edge. Left-anchored popovers on left segments extend right
    // and stay on screen; right-anchored popovers on right segments
    // extend left and stay on screen.
    const anchorRight = midPct > 60;
    return { ...c, pct, anchorRight };
  });
  const overAttributed = attributedTotal > data.rss_bytes && data.rss_bytes > 0;
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
        {/*
          Each segment carries its own styled tooltip. Drop overflow-hidden
          from the bar so the popover can escape upward; the segments sum to
          100% width and the bar's border still frames them cleanly.
        */}
        <div className="flex h-8 w-full border border-border bg-bg">
          {segments.map((c) => (
            <div
              key={c.key}
              className="group/seg relative h-full cursor-help"
              style={{ width: `${c.pct}%` }}
            >
              <div
                className="h-full"
                style={{ background: palette[c.key] ?? "rgb(154 148 133)" }}
              />
              {/*
                Anchor the popover to the segment's left or right edge
                depending on which half of the bar the segment lives in.
                A centered popover (`left-1/2 -translate-x-1/2`) clips
                on the left for the bar's leftmost segments; a left-
                anchored one clips on the right for the rightmost. The
                inline `left` / `right` attributes on the parent flip
                between `0` and `auto`, keeping the popover on-screen
                regardless of where the operator hovers.
              */}
              <div
                role="tooltip"
                style={{
                  left: c.anchorRight ? "auto" : 0,
                  right: c.anchorRight ? 0 : "auto",
                }}
                className="
                  pointer-events-none invisible absolute bottom-full z-30 mb-2
                  w-72 max-w-[20rem] border border-border-strong bg-surface px-3 py-2
                  text-left text-xs leading-snug text-text shadow-xl break-words
                  opacity-0 transition-opacity duration-100
                  group-hover/seg:visible group-hover/seg:opacity-100
                "
              >
                <div className="font-mono font-semibold">{c.label}</div>
                <div className="mt-1 tabular">
                  {fmt(c.bytes)} · {denom > 0 ? ((c.bytes / denom) * 100).toFixed(1) : "0"}% of RSS
                </div>
                <div className="mt-1 text-text-muted">{c.explain}</div>
              </div>
            </div>
          ))}
        </div>
        {overAttributed && (
          <p className="mt-1 text-[10px] leading-snug text-text-dim">
            Attributed total {fmt(attributedTotal)} exceeds kernel-reported RSS {fmt(data.rss_bytes)} —
            jemalloc.allocated counts live objects whose pages the kernel may have reclaimed.
            Bar widths clip to RSS; the table below keeps each category&rsquo;s full byte count.
          </p>
        )}
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
              <Row
                k="allocated"
                v={fmt(data.jemalloc.allocated_bytes)}
                tip={tip.jemallocAllocated}
              />
              <Row k="active" v={fmt(data.jemalloc.active_bytes)} tip={tip.jemallocActive} />
              <Row
                k="resident"
                v={fmt(data.jemalloc.resident_bytes)}
                tip={tip.jemallocResident}
              />
              <Row k="mapped" v={fmt(data.jemalloc.mapped_bytes)} tip={tip.jemallocMapped} />
              <Row
                k="retained"
                v={fmt(data.jemalloc.retained_bytes)}
                tip={tip.jemallocRetained}
              />
              <Row
                k="metadata"
                v={fmt(data.jemalloc.metadata_bytes)}
                tip={tip.jemallocMetadata}
              />
              <Row
                k="fragmentation (resident − allocated)"
                v={fmt(data.jemalloc.fragmentation_bytes)}
                tip={tip.jemallocFragmentation}
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
            <Row
              k="VmPeak (lifetime)"
              v={fmtOpt(data.vm_peak_bytes)}
              tip="High-water mark of the process virtual address space since startup. Includes all mmaps and arenas ever held; can stay high after RSS shrinks."
            />
            <Row
              k="VmHWM (resident peak)"
              v={fmtOpt(data.vm_hwm_bytes)}
              tip="Peak RSS observed since startup. Reset only on process restart — useful for capacity planning even after a load drop."
            />
            <Row
              k="VmData"
              v={fmtOpt(data.vm_data_bytes)}
              tip="Anonymous data segment: heap, BSS, malloc arenas. Most of the action lives here for a long-running pooler."
            />
            <Row
              k="VmStk"
              v={fmtOpt(data.vm_stack_bytes)}
              tip="Stack pages for every thread. Grows linearly with worker_threads × per-thread stack budget."
            />
            <Row
              k="VmExe"
              v={fmtOpt(data.vm_exe_bytes)}
              tip="Resident text (code) of the pg_doorman binary. Static for a given build — changes mean a binary upgrade landed."
            />
            <Row
              k="VmLib"
              v={fmtOpt(data.vm_lib_bytes)}
              tip="Resident text and data of dynamically-linked shared objects (libc, OpenSSL, etc.)."
            />
            <Row
              k="VmPTE (page tables)"
              v={fmtOpt(data.vm_pte_bytes)}
              tip="Kernel page-table pages backing this process's address space. Climbs with mapped memory; large here = lots of small mmaps or fragmented arenas."
            />
            <Row
              k="VmSwap"
              v={fmtOpt(data.vm_swap_bytes)}
              tip="Bytes paged out to swap. Non-zero on a database-class machine = the kernel is reclaiming under pressure; expect latency spikes."
            />
            <Row
              k="RssAnon"
              v={fmtOpt(data.rss_anon_bytes)}
              tip="RSS attributable to anonymous mappings (heap, stacks). Subset of RSS — combine with RssFile + RssShmem to recover the total."
            />
            <Row
              k="RssFile"
              v={fmtOpt(data.rss_file_bytes)}
              tip="RSS pages backing file mappings (binary text + libs). Counted toward the cgroup limit on cgroup v1; on v2 it depends on memory.swap.max."
            />
            <Row
              k="RssShmem"
              v={fmtOpt(data.rss_shmem_bytes)}
              tip="RSS in shared memory segments. Should be near zero — pg_doorman does not use sysvshm; non-zero usually means a noisy neighbour mapped into the cgroup."
            />
          </tbody>
        </table>
      </section>

      <section>
        <div className="mb-2 text-[10px] uppercase tracking-[0.2em] text-text-dim">
          pg_doorman caches
        </div>
        <table className="w-full text-xs tabular">
          <tbody>
            <Row
              k="interner named"
              v={fmt(data.interner_named_bytes)}
              tip="Bytes held by the named-statement query interner (one entry per unique SQL text used as a prepared statement). Bounded by passive GC over Arc::strong_count."
            />
            <Row
              k="interner anonymous"
              v={fmt(data.interner_anonymous_bytes)}
              tip="Bytes held by the anonymous-statement interner (ad-hoc SQL). Bounded by per-entry TTL; growing without bound = an app sends one-off SQL on every call. Either fix the app or shrink client_anonymous_prepared_cache_size."
            />
          </tbody>
        </table>
      </section>
    </div>
  );
}

function Row({ k, v, tip }: { k: string; v: string; tip?: string }) {
  return (
    <tr className="border-b border-border/40">
      <td className="py-1 text-text-muted">
        {tip ? <InfoLabel tip={tip}>{k}</InfoLabel> : k}
      </td>
      <td className="py-1 text-right font-mono">{v}</td>
    </tr>
  );
}
