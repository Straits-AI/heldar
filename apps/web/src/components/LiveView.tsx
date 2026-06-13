import Hls from "hls.js";
import { useEffect, useRef, useState } from "react";

interface Props {
  /** HLS .m3u8 URL from the liveview endpoint. */
  hlsUrl?: string | null;
  className?: string;
  poster?: string;
}

/**
 * HLS live player. Attaches an hls.js instance (or native HLS on Safari) to a
 * <video>, and tears it down cleanly when the URL changes or the component
 * unmounts. Fatal network/media errors are auto-recovered before giving up.
 */
export function LiveView({ hlsUrl, className = "", poster }: Props) {
  const videoRef = useRef<HTMLVideoElement>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    const video = videoRef.current;
    if (!video || !hlsUrl) return;

    setError(null);
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

  return (
    <div
      className={`relative aspect-video w-full overflow-hidden rounded-md bg-black ${className}`}
    >
      <video
        ref={videoRef}
        className="h-full w-full bg-black"
        poster={poster}
        muted
        playsInline
        controls
      />
      {!hlsUrl && !error && (
        <div className="absolute inset-0 flex items-center justify-center text-sm text-slate-500">
          No live stream attached
        </div>
      )}
      {error && (
        <div className="absolute inset-0 flex items-center justify-center bg-black/70 px-4 text-center text-sm text-red-300">
          {error}
        </div>
      )}
    </div>
  );
}
