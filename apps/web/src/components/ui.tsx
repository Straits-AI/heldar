// Heldar Core — shared UI primitives (Operations console / SOC).
// Phase 2 page clusters import from here; keep these names + signatures stable.

import type {
  ButtonHTMLAttributes,
  InputHTMLAttributes,
  ReactNode,
  SelectHTMLAttributes,
  TextareaHTMLAttributes,
} from "react";

/* ---------------------------------------------------------------- */
/* cx — tiny conditional className joiner                            */
/* ---------------------------------------------------------------- */
export function cx(...classes: (string | false | null | undefined)[]): string {
  return classes.filter(Boolean).join(" ");
}

/* ---------------------------------------------------------------- */
/* Panel — hairline-bordered surface with optional header           */
/* ---------------------------------------------------------------- */
export function Panel({
  title,
  subtitle,
  actions,
  className,
  bodyClassName,
  padded = true,
  children,
}: {
  title?: ReactNode;
  subtitle?: ReactNode;
  actions?: ReactNode;
  className?: string;
  bodyClassName?: string;
  padded?: boolean;
  children: ReactNode;
}) {
  const hasHeader = title != null || subtitle != null || actions != null;
  return (
    <section
      className={cx(
        "rounded-panel border border-line bg-panel shadow-panel",
        className,
      )}
    >
      {hasHeader && (
        <header className="flex items-start justify-between gap-3 border-b border-line px-4 py-3">
          <div className="min-w-0">
            {title != null && (
              <h2 className="font-display text-sm font-bold tracking-tight text-fg">
                {title}
              </h2>
            )}
            {subtitle != null && (
              <p className="mt-0.5 truncate text-xs text-fg-secondary">{subtitle}</p>
            )}
          </div>
          {actions != null && (
            <div className="flex shrink-0 items-center gap-2">{actions}</div>
          )}
        </header>
      )}
      <div className={cx(padded && "p-4", bodyClassName)}>{children}</div>
    </section>
  );
}

/* ---------------------------------------------------------------- */
/* Button                                                           */
/* ---------------------------------------------------------------- */
const BUTTON_BASE =
  "inline-flex items-center justify-center gap-1.5 rounded-md border font-medium transition-colors duration-150 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-accent focus-visible:ring-offset-2 focus-visible:ring-offset-canvas disabled:cursor-not-allowed disabled:opacity-50";

const BUTTON_VARIANTS = {
  primary:
    "border-transparent bg-accent text-accent-ink font-semibold hover:bg-accent-soft active:bg-accent-deep active:text-fg",
  default:
    "border-line bg-raised text-fg hover:border-[#34373e] hover:bg-[#23262c]",
  ghost:
    "border-transparent bg-transparent text-fg-secondary hover:bg-raised hover:text-fg",
  danger:
    "border-danger/40 bg-danger/10 text-red-300 hover:bg-danger/20 hover:text-red-200",
} as const;

const BUTTON_SIZES = {
  sm: "px-2.5 py-1 text-xs",
  md: "px-3.5 py-2 text-sm",
} as const;

export function Button({
  variant = "default",
  size = "md",
  className,
  type,
  ...props
}: ButtonHTMLAttributes<HTMLButtonElement> & {
  variant?: "primary" | "default" | "ghost" | "danger";
  size?: "sm" | "md";
}) {
  return (
    <button
      type={type ?? "button"}
      className={cx(BUTTON_BASE, BUTTON_VARIANTS[variant], BUTTON_SIZES[size], className)}
      {...props}
    />
  );
}

/* ---------------------------------------------------------------- */
/* Form controls                                                    */
/* ---------------------------------------------------------------- */
const CONTROL_BASE =
  "w-full rounded-md border border-line bg-canvas text-sm text-fg transition-colors duration-150 placeholder:text-fg-muted/70 focus:border-accent focus:outline-none focus:ring-1 focus:ring-accent disabled:cursor-not-allowed disabled:opacity-50";

export function Input({ className, ...props }: InputHTMLAttributes<HTMLInputElement>) {
  return <input className={cx(CONTROL_BASE, "px-3 py-2 font-mono", className)} {...props} />;
}

export function Textarea({
  className,
  ...props
}: TextareaHTMLAttributes<HTMLTextAreaElement>) {
  return (
    <textarea
      className={cx(CONTROL_BASE, "px-3 py-2 font-mono leading-relaxed", className)}
      {...props}
    />
  );
}

export function Select({
  className,
  children,
  ...props
}: SelectHTMLAttributes<HTMLSelectElement> & { children: ReactNode }) {
  return (
    <div className="relative">
      <select
        className={cx(CONTROL_BASE, "appearance-none px-3 py-2 pr-9", className)}
        {...props}
      >
        {children}
      </select>
      <svg
        aria-hidden="true"
        viewBox="0 0 16 16"
        className="pointer-events-none absolute right-3 top-1/2 h-3.5 w-3.5 -translate-y-1/2 text-fg-muted"
      >
        <path
          d="M4 6l4 4 4-4"
          fill="none"
          stroke="currentColor"
          strokeWidth="1.5"
          strokeLinecap="round"
          strokeLinejoin="round"
        />
      </svg>
    </div>
  );
}

/* ---------------------------------------------------------------- */
/* Field — labelled control wrapper                                 */
/* ---------------------------------------------------------------- */
export function Field({
  label,
  hint,
  htmlFor,
  children,
}: {
  label: ReactNode;
  hint?: ReactNode;
  htmlFor?: string;
  children: ReactNode;
}) {
  return (
    <div className="flex flex-col gap-1.5">
      <label
        htmlFor={htmlFor}
        className="font-mono text-[10px] font-medium uppercase tracking-micro text-fg-secondary"
      >
        {label}
      </label>
      {children}
      {hint != null && <p className="text-xs leading-snug text-fg-muted">{hint}</p>}
    </div>
  );
}

/* ---------------------------------------------------------------- */
/* Status — LED + pill                                              */
/* ---------------------------------------------------------------- */
export type CameraState =
  | "recording"
  | "connecting"
  | "offline"
  | "error"
  | "disabled"
  | "unknown";

interface StateMeta {
  color: string;
  text: string;
  label: string;
}

const STATE_META: Record<CameraState, StateMeta> = {
  recording: { color: "#10b981", text: "text-rec", label: "RECORDING" },
  connecting: { color: "#fbbf24", text: "text-connecting", label: "CONNECTING" },
  offline: { color: "#71717a", text: "text-fg-secondary", label: "OFFLINE" },
  error: { color: "#ef4444", text: "text-danger", label: "ERROR" },
  disabled: { color: "#3f3f46", text: "text-fg-muted", label: "DISABLED" },
  unknown: { color: "#52525b", text: "text-fg-muted", label: "UNKNOWN" },
};

function normalizeState(state: string): CameraState {
  return (state in STATE_META ? state : "unknown") as CameraState;
}

export function StatusLed({ state, pulse }: { state: string; pulse?: boolean }) {
  const s = normalizeState(state);
  const meta = STATE_META[s];
  const shouldPulse = pulse ?? (s === "recording" || s === "connecting");
  return (
    <span className="relative inline-flex h-2 w-2 shrink-0 items-center justify-center">
      {shouldPulse && (
        <span
          className="absolute inline-flex h-full w-full rounded-full animate-led-ping"
          style={{ backgroundColor: meta.color }}
        />
      )}
      <span
        className="relative inline-flex h-2 w-2 rounded-full"
        style={{
          backgroundColor: meta.color,
          boxShadow: `0 0 6px 0 ${meta.color}`,
        }}
      />
    </span>
  );
}

export function StatusPill({ state, label }: { state: string; label?: string }) {
  const s = normalizeState(state);
  const meta = STATE_META[s];
  return (
    <span
      className="inline-flex items-center gap-2 rounded-md border px-2 py-1"
      style={{
        borderColor: `${meta.color}40`,
        backgroundColor: `${meta.color}14`,
      }}
    >
      <StatusLed state={s} />
      <span
        className={cx(
          "font-mono text-[10px] font-semibold uppercase tracking-micro leading-none",
          meta.text,
        )}
      >
        {label ?? meta.label}
      </span>
    </span>
  );
}

/* ---------------------------------------------------------------- */
/* Stat — micro-label + big mono value                              */
/* ---------------------------------------------------------------- */
const STAT_TONE = {
  default: "text-fg",
  good: "text-rec",
  warn: "text-connecting",
  bad: "text-danger",
} as const;

export function Stat({
  label,
  value,
  unit,
  tone = "default",
}: {
  label: ReactNode;
  value: ReactNode;
  unit?: ReactNode;
  tone?: "default" | "good" | "warn" | "bad";
}) {
  return (
    <div className="flex flex-col gap-1">
      <span className="font-mono text-[10px] uppercase tracking-micro text-fg-muted">
        {label}
      </span>
      <span className="flex items-baseline gap-1">
        <span className={cx("font-mono text-lg font-semibold tabular-nums", STAT_TONE[tone])}>
          {value}
        </span>
        {unit != null && (
          <span className="font-mono text-xs text-fg-muted">{unit}</span>
        )}
      </span>
    </div>
  );
}

/* ---------------------------------------------------------------- */
/* Spinner                                                          */
/* ---------------------------------------------------------------- */
export function Spinner({ size = 16 }: { size?: number }) {
  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 24 24"
      className="animate-spin-slow text-accent"
      role="status"
      aria-label="Loading"
    >
      <circle
        cx="12"
        cy="12"
        r="9"
        fill="none"
        stroke="currentColor"
        strokeWidth="2.5"
        strokeOpacity="0.18"
      />
      <path
        d="M21 12a9 9 0 0 0-9-9"
        fill="none"
        stroke="currentColor"
        strokeWidth="2.5"
        strokeLinecap="round"
      />
    </svg>
  );
}

/* ---------------------------------------------------------------- */
/* EmptyState                                                       */
/* ---------------------------------------------------------------- */
export function EmptyState({
  title,
  hint,
  action,
}: {
  title: ReactNode;
  hint?: ReactNode;
  action?: ReactNode;
}) {
  return (
    <div className="flex flex-col items-center justify-center gap-3 rounded-panel border border-dashed border-line bg-panel/40 px-6 py-14 text-center">
      <svg
        aria-hidden="true"
        viewBox="0 0 48 48"
        className="h-9 w-9 text-fg-muted"
        fill="none"
        stroke="currentColor"
        strokeWidth="1.5"
      >
        <rect x="7" y="13" width="34" height="24" rx="3" />
        <circle cx="24" cy="25" r="6" />
        <path d="M17 13l3-4h8l3 4" strokeLinecap="round" strokeLinejoin="round" />
      </svg>
      <div className="font-display text-sm font-bold text-fg">{title}</div>
      {hint != null && (
        <p className="max-w-sm text-xs leading-relaxed text-fg-secondary">{hint}</p>
      )}
      {action != null && <div className="mt-1">{action}</div>}
    </div>
  );
}

/* ---------------------------------------------------------------- */
/* SectionLabel — small UPPERCASE mono label                        */
/* ---------------------------------------------------------------- */
export function SectionLabel({ children }: { children: ReactNode }) {
  return (
    <span className="font-mono text-[10px] font-medium uppercase tracking-micro text-fg-muted">
      {children}
    </span>
  );
}
