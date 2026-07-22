// Heldar Core — operator sign-in.
// Rendered by the access-control console when the API reports auth is enabled (401 on /auth/me).
// Flow: api.login -> server sets the HttpOnly session cookie -> re-fetch the Principal -> hand it to
// the parent. The token is NOT persisted in JS storage; the cookie carries the session (XSS-safe).

import { useEffect, useRef, useState } from "react";
import type { FormEvent } from "react";
import { api, ApiError, setAuthToken } from "../lib/api";
import { friendlyError } from "../lib/format";
import type { Principal } from "../lib/types";
import { BrandMark, Button, Field, Input, SectionLabel, Spinner } from "./ui";

// Optional Cloudflare Turnstile bot challenge: enabled only when the deployment exposes a site key
// (the Worker enforces the matching TURNSTILE_SECRET on the login endpoint). Empty = no challenge.
const TURNSTILE_SITE_KEY = (import.meta.env.VITE_TURNSTILE_SITE_KEY as string | undefined) || "";
const TURNSTILE_SCRIPT = "https://challenges.cloudflare.com/turnstile/v0/api.js";

type TurnstileApi = {
  render: (
    el: HTMLElement,
    opts: {
      sitekey: string;
      callback: (token: string) => void;
      "expired-callback"?: () => void;
      "error-callback"?: () => void;
    },
  ) => string;
  remove: (id: string) => void;
};
function turnstileApi(): TurnstileApi | undefined {
  return (window as unknown as { turnstile?: TurnstileApi }).turnstile;
}

export function Login({ onSuccess }: { onSuccess: (principal: Principal) => void }) {
  const [username, setUsername] = useState("");
  const [password, setPassword] = useState("");
  const [submitting, setSubmitting] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [turnstileToken, setTurnstileToken] = useState<string | null>(null);
  const widgetRef = useRef<HTMLDivElement>(null);
  // Pre-login box reachability (remote deployments): don't offer a blind login form for a dead box.
  // "checking" renders a brief spinner; "offline" renders the unreachable panel with a 10s auto-retry.
  const [boxStatus, setBoxStatus] = useState<"checking" | "online" | "offline">("checking");

  // One reachability check on mount decides form vs offline panel. 404 = the KERNEL answered (LAN —
  // the endpoint only exists on the remote Worker): the box is by definition reachable. Network errors
  // (status 0) also fall through to the form — a flaky client connection must not masquerade as "box
  // offline"; a real submit then explains itself via friendlyError.
  useEffect(() => {
    let alive = true;
    api
      .siteStatus()
      .then((r) => {
        if (alive) setBoxStatus(r.status === "online" ? "online" : "offline");
      })
      .catch(() => {
        if (alive) setBoxStatus("online");
      });
    return () => {
      alive = false;
    };
  }, []);

  // While offline (from the mount check OR a mid-login 502), re-probe every 10s and flip back to the
  // form the moment the box re-parks its dial-out at the rendezvous.
  useEffect(() => {
    if (boxStatus !== "offline") return;
    const timer = setInterval(() => {
      api
        .siteStatus()
        .then((r) => {
          if (r.status === "online") setBoxStatus("online");
        })
        .catch(() => setBoxStatus("online")); // 404/network: stop claiming offline; let the form speak
    }, 10_000);
    return () => clearInterval(timer);
  }, [boxStatus]);

  // Load the Turnstile script (once) and render the widget when a site key is configured. Keyed on
  // the form actually being visible: during the checking/offline states widgetRef isn't mounted, so
  // rendering must wait for boxStatus === "online" (else the widget silently never appears).
  useEffect(() => {
    if (!TURNSTILE_SITE_KEY || boxStatus !== "online") return;
    let widgetId: string | undefined;
    let cancelled = false;
    const renderWidget = () => {
      const ts = turnstileApi();
      if (!ts || cancelled || !widgetRef.current) return;
      widgetId = ts.render(widgetRef.current, {
        sitekey: TURNSTILE_SITE_KEY,
        callback: (t) => setTurnstileToken(t),
        "expired-callback": () => setTurnstileToken(null),
        "error-callback": () => setTurnstileToken(null),
      });
    };
    if (turnstileApi()) {
      renderWidget();
    } else {
      let script = document.querySelector<HTMLScriptElement>(`script[src="${TURNSTILE_SCRIPT}"]`);
      if (!script) {
        script = document.createElement("script");
        script.src = TURNSTILE_SCRIPT;
        script.async = true;
        script.defer = true;
        document.head.appendChild(script);
      }
      script.addEventListener("load", renderWidget);
    }
    return () => {
      cancelled = true;
      if (widgetId) turnstileApi()?.remove(widgetId);
    };
  }, [boxStatus]);

  async function handleSubmit(e: FormEvent) {
    e.preventDefault();
    if (submitting) return;
    if (!username.trim() || !password) {
      setError("Username and password are required.");
      return;
    }
    if (TURNSTILE_SITE_KEY && !turnstileToken) {
      setError("Please complete the verification challenge.");
      return;
    }
    setSubmitting(true);
    setError(null);
    try {
      const result = await api.login(username.trim(), password, turnstileToken ?? undefined);
      setAuthToken(result.token);
      const principal = await api.me();
      onSuccess(principal);
      // Parent unmounts this form on success; no further state writes here.
    } catch (err) {
      setAuthToken(null);
      // Operator-readable copy (502/504 → "box unreachable", 429 → rate-limited, …); raw detail
      // stays on the error object / console, never as the primary UI message.
      setError(friendlyError(err));
      if (err instanceof ApiError && (err.status === 502 || err.status === 504)) {
        setBoxStatus("offline"); // the box dropped mid-login: flip to the offline panel's retry loop
      }
      setSubmitting(false);
    }
  }

  return (
    <div className="mx-auto flex min-h-[72vh] max-w-sm flex-col justify-center px-4 py-10">
      <div className="animate-rise overflow-hidden rounded-panel border border-line bg-panel shadow-panel">
        {/* Wordmark header — login is a rare brand moment: the lone Bifrost arc + seam surface here. */}
        <div className="relative flex items-center gap-3 border-b border-line px-5 py-4">
          <span className="relative flex h-10 w-10 items-center justify-center rounded-lg border border-accent/35 bg-canvas shadow-[inset_0_1px_0_rgba(255,255,255,0.05),0_0_18px_-6px_rgba(245,158,11,0.5)]">
            <span className="pointer-events-none absolute inset-0 rounded-lg bg-bifrost-soft opacity-50" />
            <BrandMark size={24} className="relative" />
          </span>
          <div className="leading-none">
            <div className="font-display text-[15px] font-extrabold tracking-wider text-fg">
              HELDAR
            </div>
            <div className="mt-1.5 font-mono text-[9px] uppercase tracking-micro text-accent">
              Operator sign-in
            </div>
          </div>
          {/* Lone Bifrost hairline — the brand seam (matches the nav rail). */}
          <span
            aria-hidden="true"
            className="absolute inset-x-0 bottom-0 h-px bg-bifrost-line opacity-70"
          />
        </div>

        {boxStatus === "checking" ? (
          <div className="flex items-center justify-center gap-2 p-8 font-mono text-xs text-fg-muted">
            <Spinner size={14} /> Checking box connection…
          </div>
        ) : boxStatus === "offline" ? (
          <div className="space-y-3 p-5" role="alert">
            <SectionLabel>Box unreachable</SectionLabel>
            <p className="text-xs leading-relaxed text-fg-secondary">
              This box isn&apos;t connected right now — it may be powered off or have lost its network
              connection. Nothing is wrong with your account.
            </p>
            <div className="flex items-center gap-2 rounded-md border border-connecting/40 bg-connecting/10 px-3 py-2 font-mono text-[11px] text-amber-200">
              <Spinner size={12} />
              <span>Retrying automatically — this page will unlock the moment the box is back.</span>
            </div>
          </div>
        ) : (
        <form onSubmit={handleSubmit} className="space-y-4 p-5">
          <div>
            <SectionLabel>Authenticate</SectionLabel>
            <p className="mt-1 text-xs leading-relaxed text-fg-secondary">
              This console requires an operator account. Sign in to access the gate.
            </p>
          </div>

          <Field label="Username" htmlFor="login-username">
            <Input
              id="login-username"
              value={username}
              onChange={(e) => setUsername(e.target.value)}
              autoComplete="username"
              placeholder="guard01"
              autoFocus
            />
          </Field>

          <Field label="Password" htmlFor="login-password">
            <Input
              id="login-password"
              type="password"
              value={password}
              onChange={(e) => setPassword(e.target.value)}
              autoComplete="current-password"
              placeholder="••••••••"
            />
          </Field>

          {error && (
            <div
              role="alert"
              className="flex items-start gap-2 rounded-md border border-danger/40 bg-danger/10 px-3 py-2 font-mono text-xs text-red-300"
            >
              <svg
                viewBox="0 0 16 16"
                width="14"
                height="14"
                fill="none"
                stroke="currentColor"
                strokeWidth="1.5"
                strokeLinecap="round"
                strokeLinejoin="round"
                aria-hidden="true"
                className="mt-0.5 shrink-0"
              >
                <path d="M8 1.5l6.5 11.5H1.5z" />
                <path d="M8 6.5v3.5" />
                <path d="M8 11.6v.4" />
              </svg>
              <span className="break-words">{error}</span>
            </div>
          )}

          {TURNSTILE_SITE_KEY && <div ref={widgetRef} className="flex justify-center" />}

          <Button type="submit" variant="primary" disabled={submitting} className="w-full">
            {submitting ? (
              <>
                <Spinner size={14} />
                Signing in…
              </>
            ) : (
              "Sign in"
            )}
          </Button>
        </form>
        )}
      </div>
    </div>
  );
}

export default Login;
