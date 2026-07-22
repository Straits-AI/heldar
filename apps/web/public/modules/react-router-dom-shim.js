/**
 * react-router-dom shim for dynamically-loaded module bundles.
 * Re-exports from the shell's window.__HELDAR_REACT_ROUTER_DOM__ global.
 * See react-shim.js for full context.
 */

const RRD = window.__HELDAR_REACT_ROUTER_DOM__;
export const {
  BrowserRouter,
  HashRouter,
  MemoryRouter,
  Link,
  NavLink,
  Navigate,
  Outlet,
  Route,
  Router,
  Routes,
  useHref,
  useInRouterContext,
  useLocation,
  useNavigate,
  useNavigationType,
  useOutlet,
  useOutletContext,
  useParams,
  useResolvedPath,
  useRoutes,
  useSearchParams,
  createBrowserRouter,
  createHashRouter,
  createMemoryRouter,
  createPath,
  createRoutesFromChildren,
  createRoutesFromElements,
  generatePath,
  matchPath,
  matchRoutes,
  parsePath,
  redirect,
  renderMatches,
  resolvePath,
} = RRD;

export default RRD;
