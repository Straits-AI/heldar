/**
 * react/jsx-runtime shim for dynamically-loaded module bundles.
 * Re-exports from the shell's window.__HELDAR_REACT_JSX_RUNTIME__ global.
 * See react-shim.js for full context.
 */

const JR = window.__HELDAR_REACT_JSX_RUNTIME__;
export const { jsx, jsxs, jsxDEV, Fragment } = JR;
export default JR;
