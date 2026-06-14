// Heldar Core — operator sign-in.
// Rendered by the access-control console when the API reports auth is enabled (401 on /auth/me).
// Flow: api.login -> server sets the HttpOnly session cookie -> re-fetch the Principal -> hand it to
// the parent. The token is NOT persisted in JS storage; the cookie carries the session (XSS-safe).

import { useState } from "react";
import type { FormEvent } from "react";
import { api, ApiError, setAuthToken } from "../lib/api";
import type { Principal } from "../lib/types";
import { Button, Field, Input, SectionLabel, Spinner } from "./ui";

export function Login({ onSuccess }: { onSuccess: (principal: Principal) => void }) {
  const [username, setUsername] = useState("");
  const [password, setPassword] = useState("");
  const [submitting, setSubmitting] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function handleSubmit(e: FormEvent) {
    e.preventDefault();
    if (submitting) return;
    if (!username.trim() || !password) {
      setError("Username and password are required.");
      return;
    }
    setSubmitting(true);
    setError(null);
    try {
      const result = await api.login(username.trim(), password);
      setAuthToken(result.token);
      const principal = await api.me();
      onSuccess(principal);
      // Parent unmounts this form on success; no further state writes here.
    } catch (err) {
      setAuthToken(null);
      setError(err instanceof ApiError ? err.message : String(err));
      setSubmitting(false);
    }
  }

  return (
    <div className="mx-auto flex min-h-[72vh] max-w-sm flex-col justify-center px-4 py-10">
      <div className="animate-rise overflow-hidden rounded-panel border border-line bg-panel shadow-panel">
        {/* Wordmark header */}
        <div className="flex items-center gap-3 border-b border-line px-5 py-4">
          <span className="relative flex h-9 w-9 items-center justify-center rounded-md border border-accent/40 bg-canvas">
            <svg viewBox="0 0 24 24" className="h-5 w-5" fill="none" aria-hidden="true">
              <circle cx="12" cy="12" r="8" stroke="#f59e0b" strokeWidth="1.8" />
              <circle cx="12" cy="12" r="2.4" fill="#f59e0b" />
            </svg>
          </span>
          <div className="leading-none">
            <div className="font-display text-[15px] font-extrabold tracking-wider text-fg">
              HELDAR
            </div>
            <div className="mt-1 font-mono text-[9px] uppercase tracking-micro text-accent">
              Operator sign-in
            </div>
          </div>
        </div>

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
      </div>
    </div>
  );
}

export default Login;
