/**
 * react-dom/client shim for dynamically-loaded module bundles.
 * Re-exports from the shell's window.__HELDAR_REACT_DOM_CLIENT__ global.
 * See react-shim.js for full context.
 */

const RDC = window.__HELDAR_REACT_DOM_CLIENT__;
export const { createRoot, hydrateRoot } = RDC;
export default RDC;
