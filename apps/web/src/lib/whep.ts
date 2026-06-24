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
        answer = await opts.exchange(offerSdp);
      } else {
        const res = await fetch(whepUrl, {
          method: "POST",
          headers: { "Content-Type": "application/sdp", Accept: "application/sdp" },
          body: offerSdp,
        });
        if (!res.ok) throw new Error(`WHEP POST ${res.status}`);
        resourceUrl = resolveResource(res.headers.get("Location"), whepUrl);
        answer = await res.text();
      }
      if (closed) return;
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
