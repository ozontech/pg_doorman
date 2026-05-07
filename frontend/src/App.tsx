import { BrowserRouter, Navigate, Route, Routes } from "react-router-dom";
import { AuthGate } from "./components/AuthGate";
import { Sidebar } from "./components/Sidebar";
import { AdminAuthProvider } from "./hooks/useAdminAuth";
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
  return (
    <AdminAuthProvider>
      <BrowserRouter>
        <div className="flex min-h-screen bg-bg text-text">
          <Sidebar />
          <main className="flex-1 min-w-0">
            <AuthGate>
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
            </AuthGate>
          </main>
        </div>
      </BrowserRouter>
    </AdminAuthProvider>
  );
}

function NotFound() {
  return (
    <section className="px-6 py-8">
      <h1 className="font-mono text-lg font-semibold text-text">Not found</h1>
    </section>
  );
}
