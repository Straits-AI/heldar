import * as React from "react";
import * as ReactDOM from "react-dom";
import * as ReactDOMClient from "react-dom/client";
import * as ReactJsxRuntime from "react/jsx-runtime";
import * as ReactRouterDom from "react-router-dom";
import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { BrowserRouter } from "react-router-dom";
import App from "./App";
import "./index.css";

// Expose the shell's React instances on the global so that dynamically-loaded module bundles can
// import from a tiny shim (public/modules/react-shim.js) and share one React instance without
// bundling React themselves. This is the fallback for the import-map approach, which is blocked by
// Vite 8/Rolldown's chunk format (chunks use an internal module wrapper, not named ES exports).
// Only populated in production builds where module loading is used; harmless in dev.
declare global {
  interface Window {
    __HELDAR_REACT__: typeof React;
    __HELDAR_REACT_DOM__: typeof ReactDOM;
    __HELDAR_REACT_DOM_CLIENT__: typeof ReactDOMClient;
    __HELDAR_REACT_JSX_RUNTIME__: typeof ReactJsxRuntime;
    __HELDAR_REACT_ROUTER_DOM__: typeof ReactRouterDom;
  }
}
window.__HELDAR_REACT__ = React;
window.__HELDAR_REACT_DOM__ = ReactDOM;
window.__HELDAR_REACT_DOM_CLIENT__ = ReactDOMClient;
window.__HELDAR_REACT_JSX_RUNTIME__ = ReactJsxRuntime;
window.__HELDAR_REACT_ROUTER_DOM__ = ReactRouterDom;

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
