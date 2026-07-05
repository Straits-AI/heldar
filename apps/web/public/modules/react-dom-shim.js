/**
 * react-dom shim for dynamically-loaded module bundles.
 * Re-exports from the shell's window.__HELDAR_REACT_DOM__ global.
 * See react-shim.js for full context.
 */

const RD = window.__HELDAR_REACT_DOM__;
export const {
  createPortal,
  flushSync,
  preconnect,
  prefetchDNS,
  preinit,
  preinitModule,
  preload,
  preloadModule,
  unstable_batchedUpdates,
} = RD;

export default RD;
