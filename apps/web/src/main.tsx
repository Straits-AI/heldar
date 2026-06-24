import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { BrowserRouter } from "react-router-dom";
import App from "./App";
import "./index.css";

const rootEl = document.getElementById("root");
if (!rootEl) throw new Error("Root element #root not found");

// When hosted under a subpath (the remote dashboard at `/app/`, ADR 0003 P3), Vite sets BASE_URL to it;
// the router must use the same basename so client routes resolve. On the appliance BASE_URL is "/".
const basename = import.meta.env.BASE_URL.replace(/\/$/, "");

createRoot(rootEl).render(
  <StrictMode>
    <BrowserRouter basename={basename}>
      <App />
    </BrowserRouter>
  </StrictMode>,
);
