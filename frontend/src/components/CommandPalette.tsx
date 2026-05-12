import { Command } from "cmdk";
import {
  AppWindow,
  Boxes,
  Database,
  LayoutDashboard,
  ScrollText,
  Settings,
  Tv,
  Users,
  type LucideIcon,
} from "lucide-react";
import { useEffect, useState } from "react";
import { useNavigate } from "react-router-dom";
import { apiGet } from "../api";
import { useAdminAuth } from "../hooks/useAdminAuth";
import type { PoolDto, PoolsDto } from "../types";

// Global Cmd+K palette. Opens on ⌘/Ctrl-K from anywhere, narrows
// across pages and pools by substring match, and navigates on Enter.
// Pools are fetched once when the dialog opens and refreshed on each
// open — the operator searching for a pool wants the latest list, not
// what the SPA loaded when the page mounted.
interface PageEntry {
  label: string;
  to: string;
  icon: LucideIcon;
  hint?: string;
}

const PAGES: PageEntry[] = [
  { label: "Overview", to: "/overview", icon: LayoutDashboard, hint: "global pulse" },
  { label: "Pools", to: "/pools", icon: Database, hint: "saturation / errors / waiting" },
  { label: "Clients", to: "/clients", icon: Users, hint: "active sessions" },
  { label: "Apps", to: "/apps", icon: AppWindow, hint: "by application_name" },
  { label: "Caches", to: "/caches", icon: Boxes, hint: "prepared cache (sso+)" },
  { label: "Logs", to: "/logs", icon: ScrollText, hint: "live stream (sso+)" },
  { label: "Config", to: "/config", icon: Settings, hint: "config & state" },
  { label: "War room", to: "/wall", icon: Tv, hint: "kiosk view" },
];

export function CommandPalette() {
  const [open, setOpen] = useState(false);
  const [pools, setPools] = useState<PoolDto[]>([]);
  const navigate = useNavigate();
  const { authHeader } = useAdminAuth();

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "k") {
        e.preventDefault();
        setOpen((v) => !v);
        return;
      }
      // Esc to close when the palette is open. The dialog otherwise
      // trapped focus inside cmdk's input and the operator had to mouse
      // out — every other admin tool (Linear, Vercel) closes on Esc.
      if (e.key === "Escape" && open) {
        e.preventDefault();
        setOpen(false);
      }
    };
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, [open]);

  useEffect(() => {
    if (!open) return;
    const ctrl = new AbortController();
    apiGet<PoolsDto>("/api/pools", authHeader, ctrl.signal)
      .then((d) => setPools(d.pools))
      .catch(() => {});
    return () => ctrl.abort();
  }, [open, authHeader]);

  const go = (to: string) => {
    setOpen(false);
    navigate(to);
  };

  if (!open) return null;

  return (
    <div
      role="dialog"
      aria-modal="true"
      aria-label="Global command palette"
      className="fixed inset-0 z-50 flex items-start justify-center bg-bg/70 pt-[12vh] backdrop-blur-sm"
      onClick={() => setOpen(false)}
    >
      <Command
        label="Global command palette"
        className="w-[min(640px,calc(100vw-2rem))] overflow-hidden rounded-lg border border-border-strong bg-surface text-text shadow-2xl"
        onClick={(e) => e.stopPropagation()}
      >
        <Command.Input
          autoFocus
          placeholder="Jump to page, find a pool…"
          className="w-full border-b border-border bg-surface-2 px-4 py-3 text-sm text-text outline-none placeholder:text-text-dim"
        />
        <Command.List className="max-h-[60vh] overflow-y-auto p-1">
          <Command.Empty className="px-4 py-6 text-center text-sm text-text-dim">
            No matches.
          </Command.Empty>

          <Command.Group
            heading="Pages"
            className="px-1 py-1 text-[10px] uppercase tracking-wider text-text-dim [&_[cmdk-group-heading]]:px-3 [&_[cmdk-group-heading]]:py-1"
          >
            {PAGES.map((p) => {
              const Icon = p.icon;
              return (
                <Command.Item
                  key={p.to}
                  onSelect={() => go(p.to)}
                  className="flex cursor-pointer items-center gap-2.5 px-3 py-2 text-sm text-text aria-selected:bg-accent/15 aria-selected:text-accent"
                >
                  <Icon size={16} strokeWidth={1.75} aria-hidden="true" />
                  <span className="font-medium">{p.label}</span>
                  {p.hint && (
                    <span className="ml-auto text-xs text-text-dim">{p.hint}</span>
                  )}
                </Command.Item>
              );
            })}
          </Command.Group>

          {pools.length > 0 && (
            <Command.Group
              heading="Pools"
              className="px-1 py-1 text-[10px] uppercase tracking-wider text-text-dim [&_[cmdk-group-heading]]:px-3 [&_[cmdk-group-heading]]:py-1"
            >
              {pools.map((p) => (
                <Command.Item
                  key={p.id}
                  value={`pool ${p.id} ${p.database} ${p.user}`}
                  onSelect={() => go(`/pools/${encodeURIComponent(p.id)}`)}
                  className="flex cursor-pointer items-center gap-2.5 px-3 py-2 text-sm aria-selected:bg-accent/15 aria-selected:text-accent"
                >
                  <Database size={14} strokeWidth={1.75} aria-hidden="true" />
                  <span className="font-mono text-text">{p.id}</span>
                  <span className="ml-auto text-xs text-text-dim tabular">
                    {p.active}/{p.max_connections}
                  </span>
                </Command.Item>
              ))}
            </Command.Group>
          )}
        </Command.List>
        <div className="border-t border-border bg-surface-2 px-4 py-2 text-[10px] text-text-dim">
          ↑↓ navigate · enter to open · esc to close
        </div>
      </Command>
    </div>
  );
}
