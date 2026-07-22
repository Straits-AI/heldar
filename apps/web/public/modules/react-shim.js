/**
 * React shim for dynamically-loaded module bundles.
 *
 * Vite 8 / Rolldown wraps all chunks in an internal module system — the emitted
 * react.js chunk does NOT export { useState, useEffect, … } as standard ES named
 * exports. The import-map approach (point "react" → shell's react chunk) therefore
 * fails with "does not provide an export named 'useState'".
 *
 * Fallback: main.tsx exposes the shell's React instance on window.__HELDAR_REACT__
 * before any module bundle loads. This shim re-exports every hook / symbol from that
 * global so module bundles can `import { useState } from "react"` via the import map
 * (which points "react" → this file) and still share the shell's single React instance.
 *
 * This shim is served as a static asset; it is NOT bundled by Vite.
 */

const R = window.__HELDAR_REACT__;
export const {
  // Hooks
  useState,
  useEffect,
  useLayoutEffect,
  useInsertionEffect,
  useReducer,
  useCallback,
  useMemo,
  useRef,
  useContext,
  useId,
  useImperativeHandle,
  useDebugValue,
  useDeferredValue,
  useTransition,
  useSyncExternalStore,
  useOptimistic,
  useActionState,
  use,
  // Components / API
  Component,
  PureComponent,
  Fragment,
  StrictMode,
  Suspense,
  createElement,
  cloneElement,
  createContext,
  createRef,
  forwardRef,
  memo,
  lazy,
  isValidElement,
  Children,
  startTransition,
  cache,
  version,
} = R;

export default R;
