// Dev-only demo of the markdown renderer. Served by `vite dev` at
// /src/features/markdown/dev/index.html. Vite builds only the root index.html, so this page
// never reaches app/dist. Query: ?scheme=dark|light&stream=1
import { StrictMode, useEffect, useState } from "react";
import { createRoot } from "react-dom/client";
import "@/styles/index.css";
import { Markdown } from "../Markdown";
import { SAMPLE } from "./sample";

const params = new URLSearchParams(location.search);
document.documentElement.dataset.scheme = params.get("scheme") === "dark" ? "dark" : "light";

function Demo() {
  const stream = params.get("stream") === "1";
  const [length, setLength] = useState(stream ? 0 : SAMPLE.length);
  useEffect(() => {
    if (!stream) return;
    const timer = setInterval(() => setLength((n) => Math.min(SAMPLE.length, n + 7)), 30);
    return () => clearInterval(timer);
  }, [stream]);
  return (
    <main style={{ maxWidth: 720, margin: "0 auto", padding: 16 }}>
      <Markdown
        text={SAMPLE.slice(0, length)}
        streaming={stream && length < SAMPLE.length}
        onOpenFile={(path, line) => console.info("open", path, line)}
      />
    </main>
  );
}

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <Demo />
  </StrictMode>,
);
