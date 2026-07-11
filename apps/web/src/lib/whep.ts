/**
 * Minimal WHEP (WebRTC-HTTP Egress Protocol, draft-ietf-wish-whep) client for the MediaMTX WebRTC
 * endpoint the kernel exposes via `LiveUrls.webrtc_url` (`{base}/{path}` → WHEP at `{base}/{path}/whep`).
 *
 * Flow: build a recvonly offer, gather ICE locally (non-trickle — we send the full offer once gathering
 * settles, which MediaMTX accepts because it embeds its own server candidates in the answer), POST the
 * SDP, apply the answer, and pipe the remote tracks into a `<video>`. A watchdog surfaces a stall so the
 * caller can fall back (the dashboard drops to HLS); `close()` tears down the peer connection and
 * best-effort DELETEs the WHEP resource.
 *
 * Live transport for remote viewing (ADR 0003): sub-second, browser-native, no app. On a hostile NAT
 * the media path needs STUN/TURN — inject those via `iceServers` (wired up in P2; empty here = LAN/host
 * candidates only, which won't traverse CGNAT, hence the watchdog + HLS fallback).
 */
export interface WhepHandle {
  close: () => void;
}

export interface WhepOptions {
  /** ICE servers (STUN/TURN). Empty for LAN/host-only; P2 supplies a TURN-backed list. */
  iceServers?: RTCIceServer[];
  /**
   * Override how the offer SDP is exchanged for an answer. Default (when omitted): POST the offer to
   * `whepUrl` directly (LAN). For remote viewing (ADR 0003 P2) the caller passes a rendezvous exchange
   * that relays the offer through the Cloudflare Worker to the box — see `lib/rendezvous.ts`.
   */
  exchange?: (offerSdp: string) => Promise<string>;
  /** Fired once the peer connection reaches `connected`. */
  onConnected?: () => void;
  /** Fired on any setup/connection failure or stall so the caller can fall back (e.g. to HLS). */
  onError?: (err: Error) => void;
}

/** How long to wait for the peer to reach `connected` after the answer before declaring a stall. */
const CONNECT_WATCHDOG_MS = 10_000;

/** How long to wait for the SDP answer (WHEP POST or rendezvous exchange) before giving up. Without
 * this, a media/relay server that accepts the socket but never answers hangs the offer exchange forever
 * — the post-answer watchdog never even starts, so the UI is stuck on "Connecting" with no HLS fallback. */
const EXCHANGE_TIMEOUT_MS = 8_000;

/** Reject `p` after `ms` (for a caller-supplied exchange we can't abort — the fetch path uses AbortController). */
function withTimeout<T>(p: Promise<T>, ms: number, message: string): Promise<T> {
  return new Promise<T>((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error(message)), ms);
    p.then(
      (v) => {
        clearTimeout(timer);
        resolve(v);
      },
      (e) => {
        clearTimeout(timer);
        reject(e);
      },
    );
  });
}

/** Resolve when ICE gathering completes, or after `timeoutMs` (so a slow/half-open gather can't hang). */
function waitForIceGathering(pc: RTCPeerConnection, timeoutMs: number): Promise<void> {
  if (pc.iceGatheringState === "complete") return Promise.resolve();
  return new Promise((resolve) => {
    const finish = () => {
      clearTimeout(timer);
      pc.removeEventListener("icegatheringstatechange", check);
      resolve();
    };
    const check = () => {
      if (pc.iceGatheringState === "complete") finish();
    };
    const timer = setTimeout(finish, timeoutMs);
    pc.addEventListener("icegatheringstatechange", check);
  });
}

/** Resolve a WHEP `Location` (often relative) against the request URL into an absolute resource URL. */
function resolveResource(location: string | null, whepUrl: string): string | null {
  if (!location) return null;
  try {
    return new URL(location, whepUrl).toString();
  } catch {
    return null;
  }
}

export function startWhep(
  video: HTMLVideoElement,
  whepUrl: string,
  opts: WhepOptions = {},
): WhepHandle {
  let closed = false;
  let resourceUrl: string | null = null;
  let watchdog: ReturnType<typeof setTimeout> | null = null;
  let errored = false;
  const hasIceServers = (opts.iceServers?.length ?? 0) > 0;

  const pc = new RTCPeerConnection({ iceServers: opts.iceServers ?? [] });
  // Receive-only: the dashboard plays the camera; it never sends media. Order (video, then audio)
  // matches MediaMTX's reader so the m-lines line up; MediaMTX rejects the audio m-line for video-only
  // cameras, which is fine — only the video track then fires `ontrack`.
  pc.addTransceiver("video", { direction: "recvonly" });
  pc.addTransceiver("audio", { direction: "recvonly" });

  const stream = new MediaStream();
  pc.ontrack = (e) => {
    stream.addTrack(e.track);
    if (video.srcObject !== stream) video.srcObject = stream;
  };

  const clearWatchdog = () => {
    if (watchdog !== null) {
      clearTimeout(watchdog);
      watchdog = null;
    }
  };
  // Single-shot failure: clears the watchdog and fires onError at most once, so a terminal 'failed'
  // can't also trip the connect-timeout watchdog (~10s later) and double-fire the caller's fallback.
  const fail = (err: Error) => {
    if (closed || errored) return;
    errored = true;
    clearWatchdog();
    opts.onError?.(err);
  };

  pc.onconnectionstatechange = () => {
    if (closed) return;
    const st = pc.connectionState;
    if (st === "connected") {
      clearWatchdog();
      opts.onConnected?.();
    } else if (st === "failed") {
      // 'failed' is terminal. 'disconnected' may be a transient blip, so we don't fall back on it
      // directly — the post-answer watchdog covers a connect that stalls and never recovers.
      fail(new Error("WebRTC connection failed"));
    }
  };

  void (async () => {
    try {
      await pc.setLocalDescription(await pc.createOffer());
      // Non-trickle: send the full offer once ICE settles. Allow longer when STUN/TURN is configured
      // (P2) so reflexive/relay candidates can land before the one-shot POST (we don't PATCH-trickle).
      await waitForIceGathering(pc, hasIceServers ? 5000 : 2000);
      if (closed) return;
      const offerSdp = pc.localDescription?.sdp ?? "";
      let answer: string;
      if (opts.exchange) {
        // Remote path: relay the offer through the rendezvous (no WHEP resource to DELETE later).
        answer = await withTimeout(
          opts.exchange(offerSdp),
          EXCHANGE_TIMEOUT_MS,
          "WHEP exchange timed out",
        );
      } else {
        // Abort the POST if the media server accepts the socket but never answers — otherwise this
        // await hangs forever, the watchdog below never starts, and the caller never falls back to HLS.
        const ctrl = new AbortController();
        const timer = setTimeout(() => ctrl.abort(), EXCHANGE_TIMEOUT_MS);
        let res: Response;
        try {
          res = await fetch(whepUrl, {
            method: "POST",
            headers: { "Content-Type": "application/sdp", Accept: "application/sdp" },
            body: offerSdp,
            signal: ctrl.signal,
          });
        } finally {
          clearTimeout(timer);
        }
        if (!res.ok) throw new Error(`WHEP POST ${res.status}`);
        resourceUrl = resolveResource(res.headers.get("Location"), whepUrl);
        answer = await res.text();
      }
      // If close() ran while the POST was in flight it saw resourceUrl === null and couldn't DELETE the
      // WHEP session the server just created — tear it down here so it doesn't leak on MediaMTX.
      if (closed) {
        if (resourceUrl) void fetch(resourceUrl, { method: "DELETE" }).catch(() => {});
        return;
      }
      if (!answer.trim().startsWith("v=0")) throw new Error("WHEP answer was not SDP");
      await pc.setRemoteDescription({ type: "answer", sdp: answer });
      // If the peer never reaches `connected` (e.g. no reachable candidates without TURN), surface an
      // error so the caller falls back instead of hanging on "connecting".
      watchdog = setTimeout(() => {
        if (closed) return;
        const st = pc.connectionState;
        if (st !== "connected") fail(new Error("WebRTC connect timeout"));
      }, CONNECT_WATCHDOG_MS);
    } catch (err) {
      fail(err instanceof Error ? err : new Error(String(err)));
    }
  })();

  return {
    close: () => {
      if (closed) return;
      closed = true;
      clearWatchdog();
      pc.ontrack = null;
      pc.onconnectionstatechange = null;
      try {
        pc.close();
      } catch {
        /* already closed */
      }
      if (resourceUrl) void fetch(resourceUrl, { method: "DELETE" }).catch(() => {});
      try {
        video.srcObject = null;
      } catch {
        /* ignore */
      }
    },
  };
}
