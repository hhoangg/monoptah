import { useEffect, useState } from "react";
import { Drawer } from "vaul";
import { motion } from "motion/react";
import { Toaster, toast } from "sonner";
import { QueryClient, QueryClientProvider, useQuery } from "@tanstack/react-query";
import {
  Link, Outlet, RouterProvider, createRootRoute, createRoute, createRouter,
} from "@tanstack/react-router";
import { Workbox } from "workbox-window";
import { tokenize, type Tok } from "./shiki";

const SAMPLE = "const answer: number = 42;\nexport function add(a: number, b: number) { return a + b; }\n";

function Code() {
  const [lines, setLines] = useState<Tok[][] | null>(null);
  const [err, setErr] = useState<string | null>(null);
  useEffect(() => { tokenize(SAMPLE).then(setLines, (e: unknown) => setErr(String(e))); }, []);
  if (err) return <pre data-testid="shiki-error">{err}</pre>;
  if (!lines) return <pre>loading</pre>;
  return (
    <pre data-testid="shiki">
      {lines.map((line, i) => (
        <div key={i}>
          {line.map((t, j) => <span key={j} style={{ color: t.color }}>{t.content}</span>)}
        </div>
      ))}
    </pre>
  );
}

function Fetcher() {
  const q = useQuery({ queryKey: ["hello"], queryFn: async () => (await fetch("/api/hello")).json() as Promise<{ hello: string }> });
  return <p data-testid="query">{q.data ? `query:${q.data.hello}` : "query:loading"}</p>;
}

function Home() {
  const [open, setOpen] = useState(false);
  const [swState, setSwState] = useState("none");
  return (
    <main>
      <h1>S-08 stub</h1>
      <Code />
      <Fetcher />
      <motion.div
        data-testid="motion"
        initial={{ opacity: 0, x: -20 }}
        animate={{ opacity: 1, x: 40, scale: 1.1 }}
        transition={{ duration: 0.3 }}
        style={{ width: 40, height: 40, background: "rebeccapurple" }}
      />
      <button data-testid="toast-btn" onClick={() => toast("Hello toast", { description: "from sonner" })}>toast</button>
      <Drawer.Root open={open} onOpenChange={setOpen}>
        <Drawer.Trigger data-testid="drawer-open">open drawer</Drawer.Trigger>
        <Drawer.Portal>
          <Drawer.Overlay style={{ position: "fixed", inset: 0, background: "rgba(0,0,0,.4)" }} />
          <Drawer.Content data-testid="drawer" style={{ position: "fixed", bottom: 0, left: 0, right: 0, height: 300, background: "white", color: "black" }}>
            <Drawer.Title>Drawer</Drawer.Title>
            <Drawer.Description>drag me down</Drawer.Description>
            <button data-testid="drawer-close" onClick={() => setOpen(false)}>close</button>
          </Drawer.Content>
        </Drawer.Portal>
      </Drawer.Root>
      <button
        data-testid="sw-btn"
        onClick={() => {
          const wb = new Workbox("/sw.js");
          wb.addEventListener("activated", () => setSwState("activated"));
          wb.register().then((r) => setSwState(r ? "registered" : "no-registration"), (e: unknown) => setSwState(`error:${String(e)}`));
        }}
      >register sw</button>
      <p data-testid="sw-state">{swState}</p>
      <Link to="/about" data-testid="to-about">about</Link>
    </main>
  );
}

const rootRoute = createRootRoute({ component: () => (<><Outlet /><Toaster /></>) });
const index = createRoute({ getParentRoute: () => rootRoute, path: "/", component: Home });
const about = createRoute({ getParentRoute: () => rootRoute, path: "/about", component: () => <h2 data-testid="about">About page</h2> });
const router = createRouter({ routeTree: rootRoute.addChildren([index, about]) });
const queryClient = new QueryClient();

export function App() {
  return (
    <QueryClientProvider client={queryClient}>
      <RouterProvider router={router} />
    </QueryClientProvider>
  );
}
