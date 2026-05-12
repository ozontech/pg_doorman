import { BrowserRouter, Navigate, Route, Routes, useLocation } from "react-router-dom";
import { Toaster } from "sonner";
import { AuthGate } from "./components/AuthGate";
import { CommandPalette } from "./components/CommandPalette";
import { HelpModal } from "./components/HelpModal";
import { Sidebar } from "./components/Sidebar";
import { SilentCallback } from "./components/SilentCallback";
import { AdminAuthProvider } from "./hooks/useAdminAuth";
import { ThemeProvider, useTheme } from "./hooks/useTheme";
import Overview from "./pages/Overview";
import Pools from "./pages/Pools";
import PoolDetail from "./pages/PoolDetail";
import Clients from "./pages/Clients";
import Apps from "./pages/Apps";
import Caches from "./pages/Caches";
import Wall from "./pages/Wall";
import Logs from "./pages/Logs";
import ConfigState from "./pages/ConfigState";

export default function App() {
  // Silent SSO refresh: when the iframe lands on `${origin}/?sso_silent=1`
  // we render a minimal callback component instead of the regular shell,
  // so polling effects don't fire inside the hidden frame.
  if (
    typeof window !== "undefined" &&
    new URLSearchParams(window.location.search).get("sso_silent") === "1"
  ) {
    return <SilentCallback />;
  }
  return <AppMain />;
}

function RoutedShell() {
  const location = useLocation();
  return (
    <main className="flex-1 min-w-0">
      <AuthGate>
        {/* Re-keying the wrapper on pathname change replays the fade-in
            animation; the page mounts as usual but the operator sees a
            short ease-in instead of a snap. Children are unmounted by
            the route switch above us either way, so no extra remount. */}
        <div key={location.pathname} className="animate-page-in">
          <Routes>
            <Route path="/" element={<Navigate to="/overview" replace />} />
            <Route path="/overview" element={<Overview />} />
            <Route path="/pools" element={<Pools />} />
            <Route path="/pools/:poolId" element={<PoolDetail />} />
            <Route path="/clients" element={<Clients />} />
            <Route path="/apps" element={<Apps />} />
            <Route path="/caches" element={<Caches />} />
            <Route path="/logs" element={<Logs />} />
            <Route path="/config" element={<ConfigState />} />
            <Route path="/wall" element={<Wall />} />
            <Route path="*" element={<NotFound />} />
          </Routes>
        </div>
      </AuthGate>
    </main>
  );
}

function AppMain() {
  return (
    <ThemeProvider>
      <AdminAuthProvider>
        <BrowserRouter>
          <div className="flex min-h-screen bg-bg text-text">
            <Sidebar />
            <RoutedShell />
          </div>
          <CommandPalette />
          <HelpModal />
          <AppToaster />
        </BrowserRouter>
      </AdminAuthProvider>
    </ThemeProvider>
  );
}

function AppToaster() {
  const { resolved } = useTheme();
  return (
    <Toaster
      position="top-right"
      theme={resolved}
      duration={4000}
      toastOptions={{
        classNames: {
          toast: "border border-border-strong bg-surface text-text",
          title: "font-medium",
          description: "text-text-muted",
          success: "border-success/40",
          error: "border-danger/40",
        },
      }}
    />
  );
}

function NotFound() {
  return (
    <section className="px-6 py-8">
      <h1 className="font-mono text-lg font-semibold text-text">Not found</h1>
    </section>
  );
}
