import { BrowserRouter, Navigate, Route, Routes } from "react-router-dom";
import { AuthGate } from "./components/AuthGate";
import { Sidebar } from "./components/Sidebar";
import { AdminAuthProvider } from "./hooks/useAdminAuth";
import Overview from "./pages/Overview";
import Pools from "./pages/Pools";
import Clients from "./pages/Clients";
import Caches from "./pages/Caches";
import Logs from "./pages/Logs";
import ConfigState from "./pages/ConfigState";

export default function App() {
  return (
    <AdminAuthProvider>
      <BrowserRouter>
        <AuthGate>
          <div className="flex min-h-screen bg-bg text-text">
            <Sidebar />
            <main className="flex-1">
              <Routes>
                <Route path="/" element={<Navigate to="/overview" replace />} />
                <Route path="/overview" element={<Overview />} />
                <Route path="/pools" element={<Pools />} />
                <Route path="/clients" element={<Clients />} />
                <Route path="/caches" element={<Caches />} />
                <Route path="/logs" element={<Logs />} />
                <Route path="/config" element={<ConfigState />} />
                <Route path="*" element={<NotFound />} />
              </Routes>
            </main>
          </div>
        </AuthGate>
      </BrowserRouter>
    </AdminAuthProvider>
  );
}

function NotFound() {
  return (
    <section className="p-6">
      <h1 className="text-lg font-semibold text-text">Not found</h1>
    </section>
  );
}
