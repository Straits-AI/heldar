import Hls from "hls.js";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { api } from "../lib/api";
import { usePoll } from "../lib/usePoll";
import type { PlaybackSession } from "../lib/types";
import { Button, EmptyState, SectionLabel, Spinner, Stat } from "../components/ui";

// Multi-camera SYNCHRONIZED playback. Each selected camera gets its own segment-spanning HLS VOD
// session over the same [from,to] window (so every playlist starts at the same wall-clock instant);
// a single transport then drives play/pause/seek/speed across all players in lockstep. The first
// player is the clock master — its time updates the scrubber and nudges the others back into sync
// when they drift (HLS players run independent clocks). Audio is muted: this is a visual wall.

const SPEEDS = [0.5, 1, 2, 4] as const;
const DRIFT_TOLERANCE_S = 0.4;

function toLocalInput(d: Date): string {
  // datetime-local wants local "YYYY-MM-DDTHH:mm".
  const p = (n: number) => String(n).padStart(2, "0");
  return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())}T${p(d.getHours())}:${p(d.getMinutes())}`;
}

function gridCols(n: number): string {
  if (n <= 1) return "grid-cols-1";
  if (n <= 4) return "grid-cols-2";
  if (n <= 9) return "grid-cols-3";
  return "grid-cols-4";
}

export function Playback() {
  const cameras = usePoll(() => api.listCameras(), 30000);
  const camList = cameras.data ?? [];

  const now = useMemo(() => new Date(), []);
  const [from, setFrom] = useState(() => toLocalInput(new Date(now.getTime() - 30 * 60_000)));
  const [to, setTo] = useState(() => toLocalInput(now));
  const [selected, setSelected] = useState<Set<string>>(new Set());

  const [sessions, setSessions] = useState<PlaybackSession[] | null>(null);
  const [opening, setOpening] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [skipped, setSkipped] = useState<string[]>([]);

  // transport
  const [playing, setPlaying] = useState(false);
  const [rate, setRate] = useState<number>(1);
  const [cursor, setCursor] = useState(0); // seconds into the window
  const duration = useMemo(
    () => (sessions && sessions.length ? Math.max(...sessions.map((s) => s.duration_s)) : 0),
    [sessions],
  );

  const videoRefs = useRef<Map<string, HTMLVideoElement>>(new Map());
  const hlsRefs = useRef<Hls[]>([]);
  const activeSessions = useRef<PlaybackSession[]>([]);

  const releaseSessions = useCallback(() => {
    for (const h of hlsRefs.current) h.destroy();
    hlsRefs.current = [];
    for (const s of activeSessions.current) void api.deletePlaybackSession(s.id).catch(() => {});
    activeSessions.current = [];
  }, []);

  // Release server-side sessions (and their segment read-locks) when leaving the page.
  useEffect(() => releaseSessions, [releaseSessions]);

  const toggle = (id: string) =>
    setSelected((prev) => {
      const next = new Set(prev);
      next.has(id) ? next.delete(id) : next.add(id);
      return next;
    });

  async function open() {
    if (selected.size === 0) return;
    setOpening(true);
    setError(null);
    setSkipped([]);
    releaseSessions();
    setSessions(null);
    setPlaying(false);
    setCursor(0);
    const fromIso = new Date(from).toISOString();
    const toIso = new Date(to).toISOString();
    if (new Date(fromIso) >= new Date(toIso)) {
      setError("`from` must be before `to`.");
      setOpening(false);
      return;
    }
    const ids = [...selected];
    const results = await Promise.all(
      ids.map((id) =>
        api
          .createPlaybackSession(id, fromIso, toIso)
          .then((s) => ({ id, session: s }))
          .catch(() => ({ id, session: null as PlaybackSession | null })),
      ),
    );
    const ok = results.filter((r) => r.session && r.session.segment_count > 0).map((r) => r.session!);
    const noFootage = results.filter((r) => !r.session || r.session.segment_count === 0).map((r) => r.id);
    activeSessions.current = ok;
    setSkipped(noFootage);
    if (ok.length === 0) {
      setError("No recorded footage for the selected cameras in that window.");
    }
    setSessions(ok);
    setOpening(false);
  }

  // Attach an hls.js VOD player to each session's <video> once they render.
  useEffect(() => {
    if (!sessions || sessions.length === 0) return;
    const created: Hls[] = [];
    for (const s of sessions) {
      const video = videoRefs.current.get(s.id);
      if (!video) continue;
      if (Hls.isSupported()) {
        const hls = new Hls({ enableWorker: true });
        hls.loadSource(s.playlist_url);
        hls.attachMedia(video);
        created.push(hls);
      } else if (video.canPlayType("application/vnd.apple.mpegurl")) {
        video.src = s.playlist_url;
      }
      video.playbackRate = rate;
    }
    hlsRefs.current = created;
    return () => {
      for (const h of created) h.destroy();
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [sessions]);

  const eachVideo = (fn: (v: HTMLVideoElement) => void) => videoRefs.current.forEach(fn);

  const playAll = () => {
    eachVideo((v) => void v.play().catch(() => {}));
    setPlaying(true);
  };
  const pauseAll = () => {
    eachVideo((v) => v.pause());
    setPlaying(false);
  };
  const seekAll = (t: number) => {
    const clamped = Math.max(0, Math.min(duration || 0, t));
    eachVideo((v) => {
      try {
        v.currentTime = clamped;
      } catch {
        /* not seekable yet */
      }
    });
    setCursor(clamped);
  };
  const setRateAll = (r: number) => {
    eachVideo((v) => (v.playbackRate = r));
    setRate(r);
  };

  // Clock master = first session's video. Its timeupdate advances the scrubber and corrects drift on
  // the others (each HLS player runs an independent clock).
  useEffect(() => {
    if (!sessions || sessions.length === 0) return;
    const masterId = sessions[0].id;
    const master = videoRefs.current.get(masterId);
    if (!master) return;
    const onTime = () => {
      setCursor(master.currentTime);
      videoRefs.current.forEach((v, id) => {
        if (id === masterId) return;
        if (Math.abs(v.currentTime - master.currentTime) > DRIFT_TOLERANCE_S && !v.seeking) {
          try {
            v.currentTime = master.currentTime;
          } catch {
            /* ignore */
          }
        }
      });
    };
    master.addEventListener("timeupdate", onTime);
    return () => master.removeEventListener("timeupdate", onTime);
  }, [sessions]);

  const fmt = (s: number) => {
    const m = Math.floor(s / 60);
    const sec = Math.floor(s % 60);
    return `${m}:${String(sec).padStart(2, "0")}`;
  };

  return (
    <div className="mx-auto max-w-[1600px] px-4 py-6 sm:px-6">
      <header className="animate-rise">
        <SectionLabel>Operations · Playback</SectionLabel>
        <h1 className="mt-1 font-display text-2xl font-extrabold tracking-tight text-fg">
          Synchronized Playback
        </h1>
        <p className="mt-1 max-w-2xl text-sm text-fg-secondary">
          Play several cameras back over the same time window in lockstep — one transport drives every
          stream. Footage is read-locked for the session and released when you leave.
        </p>
      </header>

      {/* Setup bar */}
      <div className="mt-5 rounded-panel border border-line bg-panel p-4">
        <div className="flex flex-wrap items-end gap-4">
          <label className="flex flex-col gap-1">
            <span className="font-mono text-[10px] uppercase tracking-micro text-fg-secondary">From</span>
            <input
              type="datetime-local"
              value={from}
              onChange={(e) => setFrom(e.target.value)}
              className="input"
              data-testid="pb-from"
            />
          </label>
          <label className="flex flex-col gap-1">
            <span className="font-mono text-[10px] uppercase tracking-micro text-fg-secondary">To</span>
            <input
              type="datetime-local"
              value={to}
              onChange={(e) => setTo(e.target.value)}
              className="input"
              data-testid="pb-to"
            />
          </label>
          <Button
            onClick={() => void open()}
            disabled={opening || selected.size === 0}
            data-testid="pb-open"
          >
            {opening ? <Spinner size={14} /> : null}
            Open {selected.size > 0 ? `${selected.size} camera${selected.size > 1 ? "s" : ""}` : ""}
          </Button>
        </div>

        {/* Camera picker */}
        <div className="mt-4 flex flex-wrap gap-2" data-testid="pb-camera-picker">
          {camList.length === 0 ? (
            <span className="font-mono text-xs text-fg-muted">No cameras registered.</span>
          ) : (
            camList.map((c) => {
              const on = selected.has(c.id);
              return (
                <button
                  key={c.id}
                  type="button"
                  onClick={() => toggle(c.id)}
                  aria-pressed={on}
                  data-testid={`pb-cam-${c.id}`}
                  className={`rounded-md border px-3 py-1.5 font-mono text-xs transition-colors ${
                    on
                      ? "border-accent bg-accent/15 text-fg"
                      : "border-line bg-canvas text-fg-secondary hover:border-[#34373e] hover:text-fg"
                  }`}
                >
                  {c.name}
                </button>
              );
            })
          )}
        </div>
      </div>

      {/* Body */}
      <div className="mt-5">
        {error && (
          <div className="mb-4 rounded-md border border-danger/40 bg-danger/10 px-3 py-2 font-mono text-xs text-red-300">
            {error}
          </div>
        )}
        {skipped.length > 0 && (
          <div className="mb-4 rounded-md border border-line bg-panel px-3 py-2 font-mono text-[11px] text-fg-muted">
            No footage in window for: {skipped.join(", ")}
          </div>
        )}

        {opening ? (
          <div className="flex items-center justify-center gap-3 rounded-panel border border-line bg-panel py-16 text-fg-secondary">
            <Spinner />
            <span className="font-mono text-xs uppercase tracking-micro">Building sessions…</span>
          </div>
        ) : !sessions ? (
          <EmptyState
            title="Pick cameras and a window"
            hint="Select one or more cameras above, choose a time range, then Open to play them back together."
          />
        ) : sessions.length === 0 ? null : (
          <>
            {/* Transport */}
            <div
              className="mb-3 flex flex-wrap items-center gap-3 rounded-panel border border-line bg-panel px-3 py-2"
              data-testid="pb-transport"
            >
              <Button onClick={playing ? pauseAll : playAll} data-testid="pb-playpause">
                {playing ? "Pause" : "Play"}
              </Button>
              <input
                type="range"
                min={0}
                max={Math.max(1, Math.floor(duration))}
                value={Math.floor(cursor)}
                onChange={(e) => seekAll(Number(e.target.value))}
                className="h-1 flex-1 cursor-pointer accent-accent"
                aria-label="Seek"
                data-testid="pb-seek"
              />
              <span className="font-mono text-xs tabular-nums text-fg-secondary">
                {fmt(cursor)} / {fmt(duration)}
              </span>
              <div className="flex items-center gap-px overflow-hidden rounded-md border border-line">
                {SPEEDS.map((s) => (
                  <button
                    key={s}
                    type="button"
                    onClick={() => setRateAll(s)}
                    aria-pressed={rate === s}
                    className={`px-2 py-1 font-mono text-[11px] ${
                      rate === s ? "bg-accent text-accent-ink" : "bg-canvas text-fg-secondary hover:text-fg"
                    }`}
                  >
                    {s}×
                  </button>
                ))}
              </div>
              <Stat label="Streams" value={sessions.length} />
            </div>

            {/* Player grid */}
            <div className={`grid gap-2 ${gridCols(sessions.length)}`} data-testid="pb-grid">
              {sessions.map((s) => {
                const cam = camList.find((c) => c.id === s.camera_id);
                return (
                  <div
                    key={s.id}
                    className="relative aspect-video overflow-hidden rounded-md border border-line bg-black"
                  >
                    <video
                      ref={(el) => {
                        if (el) videoRefs.current.set(s.id, el);
                        else videoRefs.current.delete(s.id);
                      }}
                      className="h-full w-full bg-black"
                      muted
                      playsInline
                    />
                    <div className="pointer-events-none absolute left-0 top-0 bg-gradient-to-b from-black/70 to-transparent px-2 py-1 font-mono text-[11px] text-fg drop-shadow">
                      {cam?.name ?? s.camera_id}
                    </div>
                  </div>
                );
              })}
            </div>
          </>
        )}
      </div>
    </div>
  );
}
