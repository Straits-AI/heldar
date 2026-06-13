import Hls from "hls.js";
import { useEffect, useRef, useState } from "react";
import { cx, Spinner, StatusLed } from "./ui";

interface Props {
  /** HLS .m3u8 URL from the liveview endpoint. */
  hlsUrl?: string | null;
  className?: string;
  poster?: string;
  /** Camera name shown in the on-image overlay. */
  name?: string;
  /** Camera status state for the overlay LED (e.g. "recording"). */
  state?: string;
  /** Parent is currently negotiating the live URL (pre-HLS). */
  loading?: boolean;
  /** Re-request the live stream (wired to the player's retry / start control). */
  onRetry?: () => void;
}

function PlayGlyph({ className }: { className?: string }) {
  return (
    <svg viewBox="0 0 24 24" fill="currentColor" className={className} aria-hidden="true">
      <path d="M8 5.5v13a.75.75 0 0 0 1.14.64l10.5-6.5a.75.75 0 0 0 0-1.28L9.14 4.86A.75.75 0 0 0 8 5.5Z" />
    </svg>
  );
}

/**
 * HLS live player. Attaches an hls.js instance (or native HLS on Safari) to a
 * <video>, and tears it down cleanly when the URL changes or the component
 * unmounts. Fatal network/media errors are auto-recovered before giving up.
 *
 * Presentation only: a LIVE LED badge, connecting / paused / error overlays, a
 * play-or-retry control, and an on-image name + status overlay. The transport
 * logic below is unchanged.
 */
export function LiveView({
  hlsUrl,
  className = "",
  poster,
  name,
  state,
  loading,
  onRetry,
}: Props) {
  const videoRef = useRef<HTMLVideoElement>(null);
  const [error, setError] = useState<string | null>(null);
  const [playing, setPlaying] = useState(false);
  const [ready, setReady] = useState(false);

  useEffect(() => {
    const video = videoRef.current;
    if (!video || !hlsUrl) return;

    setError(null);
    setPlaying(false);
    setReady(false);
    let hls: Hls | null = null;
    let disposed = false;

    if (Hls.isSupported()) {
      hls = new Hls({
        lowLatencyMode: true,
        liveSyncDurationCount: 3,
        manifestLoadingTimeOut: 15000,
        fragLoadingTimeOut: 15000,
      });
      hls.loadSource(hlsUrl);
      hls.attachMedia(video);
      hls.on(Hls.Events.MANIFEST_PARSED, () => {
        video.play().catch(() => {
          /* autoplay may be blocked until user interaction */
        });
      });
      hls.on(Hls.Events.ERROR, (_event, data) => {
        if (!data.fatal || disposed) return;
        if (data.type === Hls.ErrorTypes.NETWORK_ERROR) {
          hls?.startLoad();
        } else if (data.type === Hls.ErrorTypes.MEDIA_ERROR) {
          hls?.recoverMediaError();
        } else {
          setError(`Stream error: ${data.details}`);
          hls?.destroy();
        }
      });
    } else if (video.canPlayType("application/vnd.apple.mpegurl")) {
      video.src = hlsUrl;
      const onMeta = () =>
        video.play().catch(() => {
          /* autoplay may be blocked */
        });
      video.addEventListener("loadedmetadata", onMeta);
      return () => {
        disposed = true;
        video.removeEventListener("loadedmetadata", onMeta);
        video.removeAttribute("src");
        video.load();
      };
    } else {
      setError("HLS playback is not supported in this browser.");
    }

    return () => {
      disposed = true;
      if (hls) hls.destroy();
      video.removeAttribute("src");
      video.load();
    };
  }, [hlsUrl]);

  // Track playback state for the overlays (no transport effects).
  useEffect(() => {
    const video = videoRef.current;
    if (!video) return;
    const onPlaying = () => {
      setPlaying(true);
      setReady(true);
    };
    const onCanPlay = () => setReady(true);
    const onPause = () => setPlaying(false);
    const onEnded = () => setPlaying(false);
    video.addEventListener("playing", onPlaying);
    video.addEventListener("canplay", onCanPlay);
    video.addEventListener("loadeddata", onCanPlay);
    video.addEventListener("pause", onPause);
    video.addEventListener("ended", onEnded);
    return () => {
      video.removeEventListener("playing", onPlaying);
      video.removeEventListener("canplay", onCanPlay);
      video.removeEventListener("loadeddata", onCanPlay);
      video.removeEventListener("pause", onPause);
      video.removeEventListener("ended", onEnded);
    };
  }, []);

  function handlePlay() {
    videoRef.current?.play().catch(() => {
      /* user gesture should satisfy autoplay; ignore */
    });
  }

  const connecting = !error && (loading || (!!hlsUrl && !ready));
  const detached = !hlsUrl && !loading && !error;
  const paused = !error && !connecting && !!hlsUrl && ready && !playing;

  return (
    <div
      className={cx(
        "group relative aspect-video w-full overflow-hidden rounded-panel border border-line bg-black shadow-panel",
        className,
      )}
    >
      <video
        ref={videoRef}
        className="h-full w-full bg-black"
        poster={poster}
        muted
        playsInline
        controls
      />

      {/* Top overlay: name + status (left), LIVE badge (right). Non-interactive. */}
      <div className="pointer-events-none absolute inset-x-0 top-0 flex items-start justify-between gap-3 bg-gradient-to-b from-black/75 via-black/25 to-transparent px-3 py-2.5">
        <div className="flex items-center gap-2 min-w-0">
          {state != null && <StatusLed state={state} />}
          {name != null && (
            <span className="truncate font-display text-[13px] font-bold tracking-tight text-fg drop-shadow">
              {name}
            </span>
          )}
          {state != null && (
            <span className="hidden font-mono text-[10px] uppercase tracking-micro text-fg-secondary sm:inline">
              {state}
            </span>
          )}
        </div>
        {playing && (
          <span className="flex items-center gap-1.5 rounded-md border border-rec/40 bg-black/60 px-2 py-1 backdrop-blur-sm">
            <StatusLed state="recording" pulse />
            <span className="font-mono text-[10px] font-semibold uppercase tracking-micro text-rec">
              Live
            </span>
          </span>
        )}
      </div>

      {/* Connecting */}
      {connecting && (
        <div className="pointer-events-none absolute inset-0 flex flex-col items-center justify-center gap-3 bg-black/55">
          <Spinner size={26} />
          <span className="font-mono text-[11px] uppercase tracking-micro text-fg-secondary">
            Connecting stream…
          </span>
        </div>
      )}

      {/* Paused / autoplay blocked — offer a play control */}
      {paused && (
        <div className="absolute inset-0 flex items-center justify-center bg-black/45">
          <button
            type="button"
            onClick={handlePlay}
            aria-label="Play live stream"
            className="pointer-events-auto flex h-16 w-16 items-center justify-center rounded-full border border-accent/50 bg-black/60 text-accent backdrop-blur-sm transition-colors duration-150 hover:bg-accent hover:text-accent-ink focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-accent focus-visible:ring-offset-2 focus-visible:ring-offset-black"
          >
            <PlayGlyph className="ml-0.5 h-7 w-7" />
          </button>
        </div>
      )}

      {/* Detached — no stream requested */}
      {detached && (
        <div className="absolute inset-0 flex flex-col items-center justify-center gap-3 bg-black/40 text-center">
          <span className="font-mono text-[11px] uppercase tracking-micro text-fg-muted">
            No live stream attached
          </span>
          {onRetry && (
            <button
              type="button"
              onClick={onRetry}
              className="pointer-events-auto inline-flex items-center gap-1.5 rounded-md border border-accent bg-accent px-3 py-1.5 font-mono text-[11px] font-semibold uppercase tracking-micro text-accent-ink transition-colors duration-150 hover:bg-accent-soft focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-accent focus-visible:ring-offset-2 focus-visible:ring-offset-black"
            >
              <PlayGlyph className="h-3.5 w-3.5" />
              Start live
            </button>
          )}
        </div>
      )}

      {/* Error */}
      {error && (
        <div className="absolute inset-0 flex flex-col items-center justify-center gap-3 bg-black/70 px-6 text-center">
          <span className="font-mono text-[11px] uppercase tracking-micro text-danger">
            Stream error
          </span>
          <p className="max-w-md font-mono text-xs text-fg-secondary">{error}</p>
          <button
            type="button"
            onClick={() => (onRetry ? onRetry() : handlePlay())}
            className="pointer-events-auto inline-flex items-center gap-1.5 rounded-md border border-line bg-raised px-3 py-1.5 text-xs font-medium text-fg transition-colors duration-150 hover:border-[#34373e] hover:bg-[#23262c] focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-accent focus-visible:ring-offset-2 focus-visible:ring-offset-black"
          >
            Retry
          </button>
        </div>
      )}
    </div>
  );
}
