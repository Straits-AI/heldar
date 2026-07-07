/**
 * Browser-side helpers for remote camera viewing via the Heldar WebRTC rendezvous (ADR 0003).
 *
 * For a box behind CGNAT, the viewer can't reach it directly. Instead it fetches short-lived Cloudflare
 * TURN ICE servers from the rendezvous Worker, then relays its WHEP offer through the Worker to the box
 * (which bridges it to its own MediaMTX). Pair `fetchRendezvousIce` (ICE) with `rendezvousExchange`
 * (offer→answer relay) and hand both to `startWhep` (the `iceServers` + `exchange` options).
 *
 * STATUS: building blocks for the **P3** integration of remote viewing into this dashboard's `LiveView`
 * (when the dashboard learns the box's rendezvous URL + a viewing ticket). The *current* remote viewer is
 * the standalone `/view` page served by the Worker itself (`apps/edge/src/index.ts`), so these helpers
 * are not yet wired into a component. Kept here so the dashboard path is a small step, not a rewrite.
 */

/** Where a remote box's rendezvous lives, as the viewer is configured/handed it. */
export interface RendezvousTarget {
  /** Deployed rendezvous Worker base URL, e.g. `https://heldar-rendezvous.<acct>.workers.dev`. */
  url: string;
  /** The box's stable site id — its Durable Object key. */
  siteId: string;
}

const trimSlash = (u: string) => u.replace(/\/+$/, "");

/** Fetch short-lived ICE servers (STUN + Cloudflare TURN) for the WebRTC peer to gather against. */
export async function fetchRendezvousIce(rendezvousUrl: string): Promise<RTCIceServer[]> {
  const res = await fetch(`${trimSlash(rendezvousUrl)}/api/v1/rtc/turn`);
  if (!res.ok) throw new Error(`rendezvous TURN ${res.status}`);
  const data = (await res.json()) as { iceServers?: RTCIceServer | RTCIceServer[] };
  if (!data.iceServers) return [];
  // Cloudflare returns a single iceServers object ({urls, username, credential}); normalize to an array.
  return Array.isArray(data.iceServers) ? data.iceServers : [data.iceServers];
}

/**
 * Build a WHEP `exchange` that relays the offer through the rendezvous to the box for `cameraId`,
 * returning the answer SDP. Usable as `startWhep(video, "", { exchange, iceServers })`.
 */
export function rendezvousExchange(
  target: RendezvousTarget,
  cameraId: string,
): (offerSdp: string) => Promise<string> {
  const url = `${trimSlash(target.url)}/api/v1/rtc/session`;
  return async (offerSdp: string) => {
    const res = await fetch(url, {
      method: "POST",
      headers: { "Content-Type": "application/json", Accept: "application/sdp" },
      body: JSON.stringify({ site_id: target.siteId, camera_id: cameraId, sdp_offer: offerSdp }),
    });
    if (!res.ok) {
      let detail = "";
      try {
        detail = ((await res.json()) as { error?: string }).error ?? "";
      } catch {
        /* non-JSON error body */
      }
      throw new Error(`rendezvous session ${res.status}${detail ? `: ${detail}` : ""}`);
    }
    return await res.text();
  };
}
