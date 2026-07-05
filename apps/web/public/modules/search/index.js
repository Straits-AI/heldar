import { useCallback as e, useEffect as t, useId as n, useMemo as r, useRef as i, useState as a } from "react";
import { Fragment as o, jsx as s, jsxs as c } from "react/jsx-runtime";
//#region src/lib/api.ts
var l = class extends Error {
	status;
	constructor(e, t) {
		super(t), this.name = "ApiError", this.status = e;
	}
}, u = null;
function d(e) {
	u = e;
}
function f(e = {}) {
	let t = new URLSearchParams();
	for (let [n, r] of Object.entries(e)) r != null && r !== "" && t.set(n, String(r));
	let n = t.toString();
	return n ? `?${n}` : "";
}
var p = 3e4;
async function m(e, t, n = p) {
	let r = { Accept: "application/json" };
	t?.body && (r["Content-Type"] = "application/json"), u && (r.Authorization = `Bearer ${u}`);
	let i = AbortSignal.timeout(n), a = t?.signal ? AbortSignal.any([t.signal, i]) : i, o;
	try {
		o = await fetch(e, {
			...t,
			signal: a,
			credentials: "include",
			headers: {
				...r,
				...t?.headers
			}
		});
	} catch (e) {
		throw t?.signal?.aborted ? e : new l(0, "Network error or request timed out");
	}
	if (!o.ok) {
		let e = `HTTP ${o.status} ${o.statusText}`;
		try {
			let t = await o.json();
			e = t.error ?? t.message ?? e;
		} catch {}
		throw new l(o.status, e);
	}
	if (o.status !== 204) return await o.json();
}
var h = encodeURIComponent, g = {
	listCameras: () => m("/api/v1/cameras"),
	getCamera: (e) => m(`/api/v1/cameras/${h(e)}`),
	createCamera: (e) => m("/api/v1/cameras", {
		method: "POST",
		body: JSON.stringify(e)
	}),
	updateCamera: (e, t) => m(`/api/v1/cameras/${h(e)}`, {
		method: "PATCH",
		body: JSON.stringify(t)
	}),
	deleteCamera: (e) => m(`/api/v1/cameras/${h(e)}`, { method: "DELETE" }),
	testCamera: (e) => m(`/api/v1/cameras/${h(e)}/test`, { method: "POST" }),
	listSegments: (e, t = {}) => m(`/api/v1/cameras/${h(e)}/segments${f(t)}`),
	getTimeline: (e, t = {}) => m(`/api/v1/cameras/${h(e)}/timeline${f(t)}`),
	cameraGaps: (e, t, n) => m(`/api/v1/cameras/${h(e)}/gaps${f({
		from: t,
		to: n
	})}`),
	exportClip: (e, t, n) => m(`/api/v1/cameras/${h(e)}/clip`, {
		method: "POST",
		body: JSON.stringify({
			from: t,
			to: n
		})
	}),
	snapshotUrl: (e, t) => `/api/v1/cameras/${h(e)}/snapshot${t ? f({ at: t }) : ""}`,
	liveview: (e) => m(`/api/v1/cameras/${h(e)}/liveview`, { method: "POST" }),
	discover: (e) => m("/api/v1/discover", {
		method: "POST",
		body: JSON.stringify(e)
	}),
	listHealth: () => m("/api/v1/health/cameras"),
	cameraHealth: (e) => m(`/api/v1/cameras/${h(e)}/health`),
	listEvents: (e = {}) => m(`/api/v1/events${f(e)}`),
	system: () => m("/api/v1/system"),
	getRetention: () => m("/api/v1/system/retention"),
	setRetention: (e) => m("/api/v1/system/retention", {
		method: "PUT",
		body: JSON.stringify(e)
	}),
	modules: () => m("/api/v1/modules"),
	moduleDetail: (e) => m(`/api/v1/modules/${h(e)}`),
	registerModule: (e) => m("/api/v1/modules", {
		method: "POST",
		body: JSON.stringify(e)
	}),
	unregisterModule: (e) => m(`/api/v1/modules/${h(e)}`, { method: "DELETE" }),
	registry: () => m("/api/v1/registry"),
	refreshRegistry: () => m("/api/v1/registry/refresh", { method: "POST" }),
	listWebhooks: () => m("/api/v1/webhooks"),
	createWebhook: (e) => m("/api/v1/webhooks", {
		method: "POST",
		body: JSON.stringify(e)
	}),
	updateWebhook: (e, t) => m(`/api/v1/webhooks/${h(e)}`, {
		method: "PATCH",
		body: JSON.stringify(t)
	}),
	deleteWebhook: (e) => m(`/api/v1/webhooks/${h(e)}`, { method: "DELETE" }),
	testWebhook: (e) => m(`/api/v1/webhooks/${h(e)}/test`, { method: "POST" }),
	webhookDeliveries: (e, t) => m(`/api/v1/webhooks/${h(e)}/deliveries${f({ limit: t })}`),
	eventTypes: () => m("/api/v1/events/types"),
	listAiTasks: (e) => m(`/api/v1/cameras/${h(e)}/ai-tasks`),
	createAiTask: (e, t) => m(`/api/v1/cameras/${h(e)}/ai-tasks`, {
		method: "POST",
		body: JSON.stringify(t)
	}),
	updateAiTask: (e, t) => m(`/api/v1/ai-tasks/${h(e)}`, {
		method: "PATCH",
		body: JSON.stringify(t)
	}),
	deleteAiTask: (e) => m(`/api/v1/ai-tasks/${h(e)}`, { method: "DELETE" }),
	aiTasks: () => m("/api/v1/ai/tasks"),
	samplers: () => m("/api/v1/ai/samplers"),
	cameraDetections: (e, t = {}) => m(`/api/v1/cameras/${h(e)}/detections${f(t)}`),
	frameUrl: (e, t) => `/api/v1/cameras/${h(e)}/frame${t ? f({ profile: t }) : ""}`,
	listZones: (e) => m(`/api/v1/cameras/${h(e)}/zones`),
	createZone: (e, t) => m(`/api/v1/cameras/${h(e)}/zones`, {
		method: "POST",
		body: JSON.stringify(t)
	}),
	updateZone: (e, t) => m(`/api/v1/zones/${h(e)}`, {
		method: "PATCH",
		body: JSON.stringify(t)
	}),
	deleteZone: (e) => m(`/api/v1/zones/${h(e)}`, { method: "DELETE" }),
	cameraZoneEvents: (e, t = {}) => m(`/api/v1/cameras/${h(e)}/zone-events${f(t)}`),
	login: (e, t, n) => m("/api/v1/auth/login", {
		method: "POST",
		body: JSON.stringify({
			username: e,
			password: t
		}),
		headers: n ? { "cf-turnstile-response": n } : void 0
	}),
	logout: () => m("/api/v1/auth/logout", { method: "POST" }),
	me: () => m("/api/v1/auth/me"),
	listUsers: () => m("/api/v1/users"),
	createUser: (e) => m("/api/v1/users", {
		method: "POST",
		body: JSON.stringify(e)
	}),
	updateUser: (e, t) => m(`/api/v1/users/${h(e)}`, {
		method: "PATCH",
		body: JSON.stringify(t)
	}),
	deleteUser: (e) => m(`/api/v1/users/${h(e)}`, { method: "DELETE" }),
	listApiKeys: () => m("/api/v1/api-keys"),
	createApiKey: (e, t) => m("/api/v1/api-keys", {
		method: "POST",
		body: JSON.stringify({
			name: e,
			role: t
		})
	}),
	deleteApiKey: (e) => m(`/api/v1/api-keys/${h(e)}`, { method: "DELETE" }),
	listVehicles: (e = {}) => m(`/api/v1/vehicles${f(e)}`),
	getVehicle: (e) => m(`/api/v1/vehicles/${h(e)}`),
	createVehicle: (e) => m("/api/v1/vehicles", {
		method: "POST",
		body: JSON.stringify(e)
	}),
	updateVehicle: (e, t) => m(`/api/v1/vehicles/${h(e)}`, {
		method: "PATCH",
		body: JSON.stringify(t)
	}),
	deleteVehicle: (e) => m(`/api/v1/vehicles/${h(e)}`, { method: "DELETE" }),
	listPasses: (e = {}) => m(`/api/v1/passes${f(e)}`),
	getPass: (e) => m(`/api/v1/passes/${h(e)}`),
	createPass: (e) => m("/api/v1/passes", {
		method: "POST",
		body: JSON.stringify(e)
	}),
	updatePass: (e, t) => m(`/api/v1/passes/${h(e)}`, {
		method: "PATCH",
		body: JSON.stringify(t)
	}),
	deletePass: (e) => m(`/api/v1/passes/${h(e)}`, { method: "DELETE" }),
	checkinPass: (e) => m(`/api/v1/passes/${h(e)}/checkin`, { method: "POST" }),
	checkoutPass: (e) => m(`/api/v1/passes/${h(e)}/checkout`, { method: "POST" }),
	listWatchlist: () => m("/api/v1/watchlist"),
	createWatch: (e) => m("/api/v1/watchlist", {
		method: "POST",
		body: JSON.stringify(e)
	}),
	updateWatch: (e, t) => m(`/api/v1/watchlist/${h(e)}`, {
		method: "PATCH",
		body: JSON.stringify(t)
	}),
	deleteWatch: (e) => m(`/api/v1/watchlist/${h(e)}`, { method: "DELETE" }),
	listEntryEvents: (e = {}) => m(`/api/v1/entry-events${f(e)}`),
	getEntryEvent: (e) => m(`/api/v1/entry-events/${h(e)}`),
	confirmEntryEvent: (e, t) => m(`/api/v1/entry-events/${h(e)}/confirm`, {
		method: "POST",
		body: JSON.stringify({ note: t })
	}),
	rejectEntryEvent: (e, t) => m(`/api/v1/entry-events/${h(e)}/reject`, {
		method: "POST",
		body: JSON.stringify({ note: t })
	}),
	reportEntryLog: (e = {}) => m(`/api/v1/reports/entry-log${f(e)}`),
	reportExceptions: (e = {}) => m(`/api/v1/reports/exceptions${f(e)}`),
	listAudit: (e = {}) => m(`/api/v1/audit${f(e)}`),
	bakeryObservations: (e = {}) => m(`/api/v1/bakery/observations${f(e)}`),
	bakeryReports: (e = {}) => m(`/api/v1/bakery/reports${f(e)}`),
	generateBakeryReport: (e, t) => m("/api/v1/bakery/reports", {
		method: "POST",
		body: JSON.stringify({
			date: e,
			scope: t
		})
	}),
	bakerySummary: (e) => m(`/api/v1/bakery/summary${f({ date: e })}`),
	triggerBakeryRollup: () => m("/api/v1/bakery/rollup", { method: "POST" }),
	movementLinks: () => m("/api/v1/movement/links"),
	createMovementLink: (e) => m("/api/v1/movement/links", {
		method: "POST",
		body: JSON.stringify(e)
	}),
	deleteMovementLink: (e) => m(`/api/v1/movement/links/${h(e)}`, { method: "DELETE" }),
	movementCandidates: (e = {}) => m(`/api/v1/movement/candidates${f(e)}`),
	confirmMovementCandidate: (e) => m(`/api/v1/movement/candidates/${h(e)}/confirm`, { method: "POST" }),
	rejectMovementCandidate: (e) => m(`/api/v1/movement/candidates/${h(e)}/reject`, { method: "POST" }),
	movementBreaches: (e = {}) => m(`/api/v1/movement/breaches${f(e)}`),
	ackBreach: (e) => m(`/api/v1/movement/breaches/${h(e)}/ack`, { method: "POST" }),
	resolveBreach: (e) => m(`/api/v1/movement/breaches/${h(e)}/resolve`, { method: "POST" }),
	searchPlate: (e) => m(`/api/v1/movement/search/plate/${h(e)}`),
	triggerMovement: () => m("/api/v1/movement/run", { method: "POST" }),
	searchEvents: (e) => m("/api/v1/search/events", {
		method: "POST",
		body: JSON.stringify(e)
	}),
	searchNl: (e) => m("/api/v1/search/nl", {
		method: "POST",
		body: JSON.stringify({ query: e })
	}),
	searchPlan: (e) => m("/api/v1/search/plan", {
		method: "POST",
		body: JSON.stringify({ query: e })
	}),
	lockSegmentEvidence: (e, t) => m(`/api/v1/segments/${h(e)}/evidence-lock`, {
		method: "POST",
		body: JSON.stringify({ incident_id: t ?? null })
	}),
	unlockSegmentEvidence: (e) => m(`/api/v1/segments/${h(e)}/evidence-lock`, { method: "DELETE" }),
	tagSegmentIncident: (e, t) => m(`/api/v1/segments/${h(e)}/incident`, {
		method: "PATCH",
		body: JSON.stringify({ incident_id: t })
	}),
	listIncidents: () => m("/api/v1/incidents"),
	incidentSegments: (e) => m(`/api/v1/incidents/${h(e)}/segments`),
	listSchedules: (e) => m(`/api/v1/cameras/${h(e)}/schedules`),
	createSchedule: (e, t) => m(`/api/v1/cameras/${h(e)}/schedules`, {
		method: "POST",
		body: JSON.stringify(t)
	}),
	updateSchedule: (e, t) => m(`/api/v1/schedules/${h(e)}`, {
		method: "PATCH",
		body: JSON.stringify(t)
	}),
	deleteSchedule: (e) => m(`/api/v1/schedules/${h(e)}`, { method: "DELETE" }),
	triggerRecord: (e) => m(`/api/v1/cameras/${h(e)}/record-trigger`, { method: "POST" }),
	createPlaybackSession: (e, t, n) => m(`/api/v1/cameras/${h(e)}/playback/sessions`, {
		method: "POST",
		body: JSON.stringify({
			from: t,
			to: n
		})
	}),
	deletePlaybackSession: (e) => m(`/api/v1/playback/sessions/${h(e)}`, { method: "DELETE" }),
	listSnapshotSchedules: (e) => m(`/api/v1/cameras/${h(e)}/snapshot-schedules`),
	createSnapshotSchedule: (e, t) => m(`/api/v1/cameras/${h(e)}/snapshot-schedules`, {
		method: "POST",
		body: JSON.stringify(t)
	}),
	updateSnapshotSchedule: (e, t) => m(`/api/v1/snapshot-schedules/${h(e)}`, {
		method: "PATCH",
		body: JSON.stringify(t)
	}),
	deleteSnapshotSchedule: (e) => m(`/api/v1/snapshot-schedules/${h(e)}`, { method: "DELETE" }),
	listSnapshots: (e, t = {}) => m(`/api/v1/cameras/${h(e)}/snapshots${f(t)}`),
	listBackupDestinations: () => m("/api/v1/backup/destinations"),
	createBackupDestination: (e) => m("/api/v1/backup/destinations", {
		method: "POST",
		body: JSON.stringify(e)
	}),
	updateBackupDestination: (e, t) => m(`/api/v1/backup/destinations/${h(e)}`, {
		method: "PATCH",
		body: JSON.stringify(t)
	}),
	deleteBackupDestination: (e) => m(`/api/v1/backup/destinations/${h(e)}`, { method: "DELETE" }),
	testDestination: (e) => m(`/api/v1/backup/destinations/${h(e)}/test`, { method: "POST" }),
	listBackupPolicies: () => m("/api/v1/backup/policies"),
	createBackupPolicy: (e) => m("/api/v1/backup/policies", {
		method: "POST",
		body: JSON.stringify(e)
	}),
	updateBackupPolicy: (e, t) => m(`/api/v1/backup/policies/${h(e)}`, {
		method: "PATCH",
		body: JSON.stringify(t)
	}),
	deleteBackupPolicy: (e) => m(`/api/v1/backup/policies/${h(e)}`, { method: "DELETE" }),
	triggerPolicy: (e) => m(`/api/v1/backup/policies/${h(e)}/trigger`, { method: "POST" }),
	listBackupJobs: (e = {}) => m(`/api/v1/backup/jobs${f(e)}`),
	getBackupJob: (e) => m(`/api/v1/backup/jobs/${h(e)}`),
	deleteBackupJob: (e) => m(`/api/v1/backup/jobs/${h(e)}`, { method: "DELETE" }),
	archiveExport: (e) => m("/api/v1/archive/export", {
		method: "POST",
		body: JSON.stringify(e)
	}),
	listArchiveExports: (e) => m(`/api/v1/archive/exports${f({ limit: e })}`),
	onvifDiscover: () => m("/api/v1/onvif/discover", { method: "POST" }),
	getCameraOnvif: (e) => m(`/api/v1/cameras/${h(e)}/onvif`),
	probeCameraOnvif: (e, t = {}) => m(`/api/v1/cameras/${h(e)}/onvif/probe`, {
		method: "POST",
		body: JSON.stringify(t)
	}),
	listPtzPresets: (e) => m(`/api/v1/cameras/${h(e)}/ptz/presets`),
	refreshPtzPresets: (e) => m(`/api/v1/cameras/${h(e)}/ptz/presets/refresh`, { method: "POST" }),
	ptzContinuous: (e, t) => m(`/api/v1/cameras/${h(e)}/ptz/continuous`, {
		method: "POST",
		body: JSON.stringify(t)
	}),
	ptzStop: (e) => m(`/api/v1/cameras/${h(e)}/ptz/stop`, { method: "POST" }),
	ptzGotoPreset: (e, t) => m(`/api/v1/cameras/${h(e)}/ptz/goto_preset`, {
		method: "POST",
		body: JSON.stringify({ token: t })
	}),
	listRecordingGaps: (e, t = {}) => m(`/api/v1/cameras/${h(e)}/recording-gaps${f(t)}`),
	retryRecordingGap: (e, t) => m(`/api/v1/cameras/${h(e)}/recording-gaps/${h(t)}/retry`, { method: "POST" }),
	getCameraDeviceInfo: (e) => m(`/api/v1/cameras/${h(e)}/config/device_info`),
	listCameraVideoConfigs: (e) => m(`/api/v1/cameras/${h(e)}/config/video`),
	getCameraVideoConfig: (e, t) => m(`/api/v1/cameras/${h(e)}/config/video/${h(String(t))}`),
	putCameraVideoConfig: (e, t, n) => m(`/api/v1/cameras/${h(e)}/config/video/${h(String(t))}`, {
		method: "PUT",
		body: JSON.stringify(n)
	}),
	getCameraTimeConfig: (e) => m(`/api/v1/cameras/${h(e)}/config/time`),
	putCameraTimeConfig: (e, t) => m(`/api/v1/cameras/${h(e)}/config/time`, {
		method: "PUT",
		body: JSON.stringify(t)
	}),
	getCameraNtpConfig: (e) => m(`/api/v1/cameras/${h(e)}/config/time/ntp`),
	putCameraNtpConfig: (e, t) => m(`/api/v1/cameras/${h(e)}/config/time/ntp`, {
		method: "PUT",
		body: JSON.stringify(t)
	}),
	syncCameraTimeNow: (e) => m(`/api/v1/cameras/${h(e)}/config/time/sync_now`, { method: "POST" }),
	getCameraOnvifSettings: (e) => m(`/api/v1/cameras/${h(e)}/config/onvif`),
	putCameraOnvifSettings: (e, t) => m(`/api/v1/cameras/${h(e)}/config/onvif`, {
		method: "PUT",
		body: JSON.stringify(t)
	}),
	ensureCameraOnvifUser: (e, t) => m(`/api/v1/cameras/${h(e)}/config/onvif/ensure_user`, {
		method: "POST",
		body: JSON.stringify(t)
	}),
	getCameraOsdConfig: (e) => m(`/api/v1/cameras/${h(e)}/config/osd`),
	putCameraOsdConfig: (e, t) => m(`/api/v1/cameras/${h(e)}/config/osd`, {
		method: "PUT",
		body: JSON.stringify(t)
	}),
	rebootCamera: (e) => m(`/api/v1/cameras/${h(e)}/config/reboot`, {
		method: "POST",
		body: JSON.stringify({ confirm: !0 })
	}),
	bulkCameraConfig: (e) => m("/api/v1/cameras/config/bulk", {
		method: "POST",
		body: JSON.stringify(e)
	}, 3e5),
	getSite: () => m("/api/v1/site"),
	listOutbox: (e = {}) => m(`/api/v1/outbox${f(e)}`)
};
//#endregion
//#region src/lib/usePoll.ts
function _(n, r, o = []) {
	let [s, c] = a(null), [l, u] = a(null), [d, f] = a(!0), p = i(n);
	p.current = n;
	let m = i(!0), h = e(async () => {
		try {
			let e = await p.current();
			if (!m.current) return;
			c(e), u(null);
		} catch (e) {
			if (!m.current) return;
			u(e instanceof Error ? e.message : String(e));
		} finally {
			m.current && f(!1);
		}
	}, []);
	return t(() => {
		m.current = !0, f(!0), h();
		let e;
		return r > 0 && (e = setInterval(() => void h(), r)), () => {
			m.current = !1, e && clearInterval(e);
		};
	}, [
		h,
		r,
		...o
	]), {
		data: s,
		error: l,
		loading: d,
		refresh: h
	};
}
//#endregion
//#region src/components/ui.tsx
function v(...e) {
	return e.filter(Boolean).join(" ");
}
function y({ size: e = 24, className: t }) {
	let r = `bifrost-${n()}`;
	return /* @__PURE__ */ c("svg", {
		viewBox: "0 0 32 32",
		width: e,
		height: e,
		className: t,
		fill: "none",
		"aria-hidden": "true",
		children: [
			/* @__PURE__ */ s("defs", { children: /* @__PURE__ */ c("linearGradient", {
				id: r,
				x1: "3",
				y1: "11",
				x2: "29",
				y2: "11",
				gradientUnits: "userSpaceOnUse",
				children: [
					/* @__PURE__ */ s("stop", {
						offset: "0%",
						stopColor: "#fbbf24"
					}),
					/* @__PURE__ */ s("stop", {
						offset: "42%",
						stopColor: "#f59e0b"
					}),
					/* @__PURE__ */ s("stop", {
						offset: "78%",
						stopColor: "#a78bfa"
					}),
					/* @__PURE__ */ s("stop", {
						offset: "100%",
						stopColor: "#2dd4bf"
					})
				]
			}) }),
			/* @__PURE__ */ s("path", {
				d: "M4.5 12.5a11.5 11.5 0 0 1 23 0",
				stroke: `url(#${r})`,
				strokeWidth: "2",
				strokeLinecap: "round"
			}),
			/* @__PURE__ */ s("circle", {
				cx: "16",
				cy: "18",
				r: "8",
				stroke: "#f59e0b",
				strokeWidth: "1.7"
			}),
			/* @__PURE__ */ c("g", {
				stroke: "#f59e0b",
				strokeWidth: "1.2",
				strokeLinecap: "round",
				opacity: "0.55",
				children: [
					/* @__PURE__ */ s("path", { d: "M16 11.2 19.4 17" }),
					/* @__PURE__ */ s("path", { d: "M22.8 19.4 16.6 19.7" }),
					/* @__PURE__ */ s("path", { d: "M19 24.4 14.6 19.9" }),
					/* @__PURE__ */ s("path", { d: "M13 24.4 16 18.3" }),
					/* @__PURE__ */ s("path", { d: "M9.2 19.4 15.4 16.6" }),
					/* @__PURE__ */ s("path", { d: "M13 11.6 16 17" })
				]
			}),
			/* @__PURE__ */ s("circle", {
				cx: "16",
				cy: "18",
				r: "2.5",
				fill: "#f59e0b"
			})
		]
	});
}
function b({ title: e, subtitle: t, actions: n, className: r, bodyClassName: i, padded: a = !0, children: o }) {
	let l = e != null || t != null || n != null;
	return /* @__PURE__ */ c("section", {
		className: v("rounded-panel border border-line bg-panel shadow-panel", r),
		children: [l && /* @__PURE__ */ c("header", {
			className: "flex items-start justify-between gap-3 border-b border-line px-4 py-3",
			children: [/* @__PURE__ */ c("div", {
				className: "min-w-0",
				children: [e != null && /* @__PURE__ */ s("h2", {
					className: "font-display text-sm font-bold tracking-tight text-fg",
					children: e
				}), t != null && /* @__PURE__ */ s("p", {
					className: "mt-0.5 truncate text-xs text-fg-secondary",
					children: t
				})]
			}), n != null && /* @__PURE__ */ s("div", {
				className: "flex shrink-0 items-center gap-2",
				children: n
			})]
		}), /* @__PURE__ */ s("div", {
			className: v(a && "p-4", i),
			children: o
		})]
	});
}
var x = "inline-flex select-none items-center justify-center gap-1.5 rounded-md border font-medium transition-[background-color,border-color,box-shadow,transform,color] duration-150 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-accent focus-visible:ring-offset-2 focus-visible:ring-offset-canvas active:translate-y-px disabled:pointer-events-none disabled:cursor-not-allowed disabled:opacity-50", S = {
	primary: "border-transparent bg-accent text-accent-ink font-semibold hover:bg-accent-soft hover:shadow-glow-soft active:bg-accent-deep active:text-fg active:shadow-none",
	default: "border-line bg-raised text-fg shadow-raised hover:border-[#34373e] hover:bg-[#23262c] active:shadow-none",
	ghost: "border-transparent bg-transparent text-fg-secondary hover:bg-raised hover:text-fg",
	danger: "border-danger/40 bg-danger/10 text-red-300 hover:bg-danger/20 hover:text-red-200 hover:border-danger/60"
}, C = {
	sm: "px-2.5 py-1 text-xs",
	md: "px-3.5 py-2 text-sm"
};
function w({ variant: e = "default", size: t = "md", className: n, type: r, ...i }) {
	return /* @__PURE__ */ s("button", {
		type: r ?? "button",
		className: v(x, S[e], C[t], n),
		...i
	});
}
var T = "w-full rounded-md border border-line bg-canvas text-sm text-fg shadow-[inset_0_1px_1px_rgba(0,0,0,0.35)] transition-[border-color,box-shadow,background-color] duration-150 placeholder:text-fg-muted/70 hover:border-[#34373e] focus:border-accent focus:outline-none focus:ring-1 focus:ring-accent disabled:cursor-not-allowed disabled:opacity-50";
function E({ className: e, ...t }) {
	return /* @__PURE__ */ s("input", {
		className: v(T, "px-3 py-2 font-mono", e),
		...t
	});
}
function D({ className: e, children: t, ...n }) {
	return /* @__PURE__ */ c("div", {
		className: "relative",
		children: [/* @__PURE__ */ s("select", {
			className: v(T, "appearance-none px-3 py-2 pr-9", e),
			...n,
			children: t
		}), /* @__PURE__ */ s("svg", {
			"aria-hidden": "true",
			viewBox: "0 0 16 16",
			className: "pointer-events-none absolute right-3 top-1/2 h-3.5 w-3.5 -translate-y-1/2 text-fg-muted",
			children: /* @__PURE__ */ s("path", {
				d: "M4 6l4 4 4-4",
				fill: "none",
				stroke: "currentColor",
				strokeWidth: "1.5",
				strokeLinecap: "round",
				strokeLinejoin: "round"
			})
		})]
	});
}
function O({ label: e, hint: t, htmlFor: n, children: r }) {
	return /* @__PURE__ */ c("div", {
		className: "flex flex-col gap-1.5",
		children: [
			/* @__PURE__ */ s("label", {
				htmlFor: n,
				className: "font-mono text-[10px] font-medium uppercase tracking-micro text-fg-secondary",
				children: e
			}),
			r,
			t != null && /* @__PURE__ */ s("p", {
				className: "text-xs leading-snug text-fg-muted",
				children: t
			})
		]
	});
}
var k = {
	recording: {
		color: "#10b981",
		text: "text-rec",
		label: "RECORDING"
	},
	connecting: {
		color: "#fbbf24",
		text: "text-connecting",
		label: "CONNECTING"
	},
	offline: {
		color: "#71717a",
		text: "text-fg-secondary",
		label: "OFFLINE"
	},
	error: {
		color: "#ef4444",
		text: "text-danger",
		label: "ERROR"
	},
	disabled: {
		color: "#3f3f46",
		text: "text-fg-muted",
		label: "DISABLED"
	},
	unknown: {
		color: "#52525b",
		text: "text-fg-muted",
		label: "UNKNOWN"
	}
};
function A(e) {
	return e in k ? e : "unknown";
}
function j({ state: e, pulse: t }) {
	let n = A(e), r = k[n];
	return /* @__PURE__ */ c("span", {
		className: "relative inline-flex h-2 w-2 shrink-0 items-center justify-center",
		children: [(t ?? (n === "recording" || n === "connecting")) && /* @__PURE__ */ s("span", {
			className: "absolute inline-flex h-full w-full rounded-full animate-led-ping",
			style: { backgroundColor: r.color }
		}), /* @__PURE__ */ s("span", {
			className: "relative inline-flex h-2 w-2 rounded-full",
			style: {
				backgroundColor: r.color,
				boxShadow: `0 0 6px 0 ${r.color}`
			}
		})]
	});
}
function M({ state: e, label: t }) {
	let n = A(e), r = k[n];
	return /* @__PURE__ */ c("span", {
		className: "inline-flex items-center gap-2 rounded-md border px-2 py-1 shadow-[inset_0_1px_0_rgba(255,255,255,0.04)]",
		style: {
			borderColor: `${r.color}40`,
			backgroundColor: `${r.color}14`
		},
		children: [/* @__PURE__ */ s(j, { state: n }), /* @__PURE__ */ s("span", {
			className: v("font-mono text-[10px] font-semibold uppercase tracking-micro leading-none", r.text),
			children: t ?? r.label
		})]
	});
}
var N = {
	default: "text-fg",
	good: "text-rec",
	warn: "text-connecting",
	bad: "text-danger"
};
function P({ label: e, value: t, unit: n, tone: r = "default" }) {
	return /* @__PURE__ */ c("div", {
		className: "flex flex-col gap-1",
		children: [/* @__PURE__ */ s("span", {
			className: "font-mono text-[10px] uppercase tracking-micro text-fg-muted",
			children: e
		}), /* @__PURE__ */ c("span", {
			className: "flex items-baseline gap-1",
			children: [/* @__PURE__ */ s("span", {
				className: v("font-mono text-lg font-semibold tabular-nums", N[r]),
				children: t
			}), n != null && /* @__PURE__ */ s("span", {
				className: "font-mono text-xs text-fg-muted",
				children: n
			})]
		})]
	});
}
function F({ size: e = 16 }) {
	return /* @__PURE__ */ c("svg", {
		width: e,
		height: e,
		viewBox: "0 0 24 24",
		className: "animate-spin-slow text-accent",
		role: "status",
		"aria-label": "Loading",
		children: [/* @__PURE__ */ s("circle", {
			cx: "12",
			cy: "12",
			r: "9",
			fill: "none",
			stroke: "currentColor",
			strokeWidth: "2.5",
			strokeOpacity: "0.18"
		}), /* @__PURE__ */ s("path", {
			d: "M21 12a9 9 0 0 0-9-9",
			fill: "none",
			stroke: "currentColor",
			strokeWidth: "2.5",
			strokeLinecap: "round"
		})]
	});
}
function I({ title: e, hint: t, action: n }) {
	return /* @__PURE__ */ c("div", {
		className: "flex flex-col items-center justify-center gap-3 rounded-panel border border-dashed border-line bg-panel/40 px-6 py-14 text-center",
		children: [
			/* @__PURE__ */ c("svg", {
				"aria-hidden": "true",
				viewBox: "0 0 48 48",
				className: "h-9 w-9 text-fg-muted",
				fill: "none",
				stroke: "currentColor",
				strokeWidth: "1.5",
				children: [
					/* @__PURE__ */ s("rect", {
						x: "7",
						y: "13",
						width: "34",
						height: "24",
						rx: "3"
					}),
					/* @__PURE__ */ s("circle", {
						cx: "24",
						cy: "25",
						r: "6"
					}),
					/* @__PURE__ */ s("path", {
						d: "M17 13l3-4h8l3 4",
						strokeLinecap: "round",
						strokeLinejoin: "round"
					})
				]
			}),
			/* @__PURE__ */ s("div", {
				className: "font-display text-sm font-bold text-fg",
				children: e
			}),
			t != null && /* @__PURE__ */ s("p", {
				className: "max-w-sm text-xs leading-relaxed text-fg-secondary",
				children: t
			}),
			n != null && /* @__PURE__ */ s("div", {
				className: "mt-1",
				children: n
			})
		]
	});
}
function L({ children: e }) {
	return /* @__PURE__ */ s("span", {
		className: "font-mono text-[10px] font-medium uppercase tracking-micro text-fg-muted",
		children: e
	});
}
//#endregion
//#region src/components/Login.tsx
var R = "";
function ee({ onSuccess: e }) {
	let [n, r] = a(""), [u, f] = a(""), [p, m] = a(!1), [h, _] = a(null), [v, b] = a(null);
	i(null), t(() => {}, []);
	async function x(t) {
		if (t.preventDefault(), !p) {
			if (!n.trim() || !u) {
				_("Username and password are required.");
				return;
			}
			m(!0), _(null);
			try {
				d((await g.login(n.trim(), u, v ?? void 0)).token), e(await g.me());
			} catch (e) {
				d(null), _(e instanceof l ? e.message : String(e)), m(!1);
			}
		}
	}
	return /* @__PURE__ */ s("div", {
		className: "mx-auto flex min-h-[72vh] max-w-sm flex-col justify-center px-4 py-10",
		children: /* @__PURE__ */ c("div", {
			className: "animate-rise overflow-hidden rounded-panel border border-line bg-panel shadow-panel",
			children: [/* @__PURE__ */ c("div", {
				className: "relative flex items-center gap-3 border-b border-line px-5 py-4",
				children: [
					/* @__PURE__ */ c("span", {
						className: "relative flex h-10 w-10 items-center justify-center rounded-lg border border-accent/35 bg-canvas shadow-[inset_0_1px_0_rgba(255,255,255,0.05),0_0_18px_-6px_rgba(245,158,11,0.5)]",
						children: [/* @__PURE__ */ s("span", { className: "pointer-events-none absolute inset-0 rounded-lg bg-bifrost-soft opacity-50" }), /* @__PURE__ */ s(y, {
							size: 24,
							className: "relative"
						})]
					}),
					/* @__PURE__ */ c("div", {
						className: "leading-none",
						children: [/* @__PURE__ */ s("div", {
							className: "font-display text-[15px] font-extrabold tracking-wider text-fg",
							children: "HELDAR"
						}), /* @__PURE__ */ s("div", {
							className: "mt-1.5 font-mono text-[9px] uppercase tracking-micro text-accent",
							children: "Operator sign-in"
						})]
					}),
					/* @__PURE__ */ s("span", {
						"aria-hidden": "true",
						className: "absolute inset-x-0 bottom-0 h-px bg-bifrost-line opacity-70"
					})
				]
			}), /* @__PURE__ */ c("form", {
				onSubmit: x,
				className: "space-y-4 p-5",
				children: [
					/* @__PURE__ */ c("div", { children: [/* @__PURE__ */ s(L, { children: "Authenticate" }), /* @__PURE__ */ s("p", {
						className: "mt-1 text-xs leading-relaxed text-fg-secondary",
						children: "This console requires an operator account. Sign in to access the gate."
					})] }),
					/* @__PURE__ */ s(O, {
						label: "Username",
						htmlFor: "login-username",
						children: /* @__PURE__ */ s(E, {
							id: "login-username",
							value: n,
							onChange: (e) => r(e.target.value),
							autoComplete: "username",
							placeholder: "guard01",
							autoFocus: !0
						})
					}),
					/* @__PURE__ */ s(O, {
						label: "Password",
						htmlFor: "login-password",
						children: /* @__PURE__ */ s(E, {
							id: "login-password",
							type: "password",
							value: u,
							onChange: (e) => f(e.target.value),
							autoComplete: "current-password",
							placeholder: "••••••••"
						})
					}),
					h && /* @__PURE__ */ c("div", {
						role: "alert",
						className: "flex items-start gap-2 rounded-md border border-danger/40 bg-danger/10 px-3 py-2 font-mono text-xs text-red-300",
						children: [/* @__PURE__ */ c("svg", {
							viewBox: "0 0 16 16",
							width: "14",
							height: "14",
							fill: "none",
							stroke: "currentColor",
							strokeWidth: "1.5",
							strokeLinecap: "round",
							strokeLinejoin: "round",
							"aria-hidden": "true",
							className: "mt-0.5 shrink-0",
							children: [
								/* @__PURE__ */ s("path", { d: "M8 1.5l6.5 11.5H1.5z" }),
								/* @__PURE__ */ s("path", { d: "M8 6.5v3.5" }),
								/* @__PURE__ */ s("path", { d: "M8 11.6v.4" })
							]
						}), /* @__PURE__ */ s("span", {
							className: "break-words",
							children: h
						})]
					}),
					R,
					/* @__PURE__ */ s(w, {
						type: "submit",
						variant: "primary",
						disabled: p,
						className: "w-full",
						children: p ? /* @__PURE__ */ c(o, { children: [/* @__PURE__ */ s(F, { size: 14 }), "Signing in…"] }) : "Sign in"
					})
				]
			})]
		})
	});
}
//#endregion
//#region src/lib/format.ts
function z(e) {
	if (!e) return "—";
	let t = new Date(e);
	return Number.isNaN(t.getTime()) ? "—" : t.toLocaleString();
}
function B(e) {
	if (!e) return null;
	let t = new Date(e);
	return Number.isNaN(t.getTime()) ? null : t.toISOString();
}
//#endregion
//#region src/pages/Search.tsx
var V = {
	matched: "recording",
	exception: "connecting",
	blocked: "error",
	unmatched: "offline"
}, H = {
	matched: "#10b981",
	exception: "#fbbf24",
	blocked: "#ef4444",
	unmatched: "#52525b"
}, U = {
	entry: "#38bdf8",
	zone: "#a78bfa",
	breach: "#ef4444"
}, W = {
	llm: "#f59e0b",
	rules: "#38bdf8",
	structured: "#a78bfa"
}, G = {
	inference: {
		color: "#fbbf24",
		blurb: "Interpretation — the only inference"
	},
	aggregate: {
		color: "#38bdf8",
		blurb: "Deterministic query over stored facts"
	},
	event: {
		color: "#10b981",
		blurb: "Event-level facts, each with provenance"
	},
	track: {
		color: "#10b981",
		blurb: "Track-level provenance"
	},
	observation: {
		color: "#10b981",
		blurb: "Raw observation"
	}
}, K = {
	inference: 0,
	aggregate: 1,
	event: 2,
	track: 3,
	observation: 4
}, q = [
	"white cars entering after 6pm last week",
	"unauthorized vehicles today",
	"red zone breaches yesterday"
];
function J({ label: e, color: t }) {
	return /* @__PURE__ */ s("span", {
		className: "inline-flex shrink-0 items-center rounded border px-1.5 py-0.5 font-mono text-[9px] font-semibold uppercase tracking-micro leading-none",
		style: {
			color: t,
			borderColor: `${t}55`,
			backgroundColor: `${t}1a`
		},
		children: e
	});
}
function Y({ className: e }) {
	return /* @__PURE__ */ c("svg", {
		viewBox: "0 0 16 16",
		width: "14",
		height: "14",
		fill: "none",
		stroke: "currentColor",
		strokeWidth: "1.5",
		strokeLinecap: "round",
		strokeLinejoin: "round",
		"aria-hidden": "true",
		className: e,
		children: [
			/* @__PURE__ */ s("path", { d: "M8 1.5l6.5 11.5H1.5z" }),
			/* @__PURE__ */ s("path", { d: "M8 6.5v3.5" }),
			/* @__PURE__ */ s("path", { d: "M8 11.6v.4" })
		]
	});
}
function X({ children: e }) {
	return /* @__PURE__ */ c("div", {
		role: "alert",
		className: "flex items-start gap-2 rounded-md border border-danger/40 bg-danger/10 px-3 py-2 font-mono text-xs text-red-300",
		children: [/* @__PURE__ */ s(Y, { className: "mt-0.5 shrink-0" }), /* @__PURE__ */ s("span", {
			className: "break-words",
			children: e
		})]
	});
}
function Z(e, t) {
	if (!e) return null;
	let n = e[t];
	return typeof n == "string" ? n.trim() ? n : null : typeof n == "number" || typeof n == "boolean" ? String(n) : null;
}
function te({ path: e, alt: t }) {
	let [n, r] = a(!1);
	return n ? null : /* @__PURE__ */ s("img", {
		src: e,
		alt: t,
		loading: "lazy",
		onError: () => r(!0),
		className: "h-16 w-24 shrink-0 rounded-md border border-line bg-black object-cover"
	});
}
function ne({ children: e }) {
	return /* @__PURE__ */ c("div", {
		className: "flex items-start gap-3 rounded-panel border border-line bg-panel px-4 py-3",
		children: [/* @__PURE__ */ c("svg", {
			viewBox: "0 0 20 20",
			className: "mt-0.5 h-4 w-4 shrink-0 text-accent",
			fill: "none",
			stroke: "currentColor",
			strokeWidth: "1.6",
			strokeLinecap: "round",
			strokeLinejoin: "round",
			"aria-hidden": "true",
			children: [
				/* @__PURE__ */ s("circle", {
					cx: "10",
					cy: "10",
					r: "7.5"
				}),
				/* @__PURE__ */ s("path", { d: "M10 9v4" }),
				/* @__PURE__ */ s("path", { d: "M10 6.6v.4" })
			]
		}), /* @__PURE__ */ s("p", {
			className: "font-mono text-[11px] leading-relaxed text-fg-secondary",
			children: e
		})]
	});
}
function Q(e) {
	return `${String(e).padStart(2, "0")}:00`;
}
function re({ label: e, value: t }) {
	return /* @__PURE__ */ c("span", {
		className: "inline-flex items-center gap-1.5 rounded-md border border-line bg-canvas px-2 py-1 leading-none",
		children: [/* @__PURE__ */ s("span", {
			className: "font-mono text-[9px] uppercase tracking-micro text-fg-muted",
			children: e
		}), /* @__PURE__ */ s("span", {
			className: "font-mono text-[11px] font-semibold text-fg",
			children: t
		})]
	});
}
function ie(e, t) {
	let n = [];
	return e.from && n.push({
		label: "From",
		value: z(e.from)
	}), e.to && n.push({
		label: "To",
		value: z(e.to)
	}), e.hour_min != null && n.push({
		label: "After",
		value: `${Q(e.hour_min)} UTC`
	}), e.hour_max != null && n.push({
		label: "Before",
		value: `${Q(e.hour_max)} UTC`
	}), e.cameras && e.cameras.length > 0 && n.push({
		label: "Cameras",
		value: e.cameras.map((e) => t(e)).join(", ")
	}), e.sources && e.sources.length > 0 && n.push({
		label: "Sources",
		value: e.sources.join(" · ")
	}), e.plate && n.push({
		label: "Plate",
		value: e.plate
	}), e.color && n.push({
		label: "Color",
		value: e.color
	}), e.vehicle_type && n.push({
		label: "Vehicle",
		value: e.vehicle_type
	}), e.subject_type && n.push({
		label: "Subject",
		value: e.subject_type
	}), e.auth_status && e.auth_status.length > 0 && n.push({
		label: "Auth",
		value: e.auth_status.join(" · ")
	}), e.event_type && n.push({
		label: "Event",
		value: e.event_type
	}), e.zone_kind && n.push({
		label: "Zone kind",
		value: e.zone_kind
	}), e.text && n.push({
		label: "Text",
		value: e.text
	}), e.limit != null && n.push({
		label: "Limit",
		value: String(e.limit)
	}), n;
}
function ae({ plan: e, planner: t, nameFor: n, dryRun: i }) {
	let a = r(() => ie(e, n), [e, n]), o = W[t] ?? "#71717a";
	return /* @__PURE__ */ c(b, {
		title: "Interpreted as",
		subtitle: "The structured plan your question was turned into — the only inference in the answer",
		actions: /* @__PURE__ */ c("div", {
			className: "flex items-center gap-2",
			children: [i && /* @__PURE__ */ s(J, {
				label: "Dry run",
				color: "#fbbf24"
			}), /* @__PURE__ */ s(J, {
				label: `Planner · ${t}`,
				color: o
			})]
		}),
		children: [a.length === 0 ? /* @__PURE__ */ c("p", {
			className: "font-mono text-[11px] leading-relaxed text-fg-secondary",
			children: [
				"No filters were extracted — this defaults to",
				" ",
				/* @__PURE__ */ s("span", {
					className: "text-fg",
					children: "all sources"
				}),
				" over the last ~7 days. Add detail (color, time, camera, authorization) to narrow it."
			]
		}) : /* @__PURE__ */ s("div", {
			className: "flex flex-wrap gap-2",
			children: a.map((e) => /* @__PURE__ */ s(re, {
				label: e.label,
				value: e.value
			}, e.label))
		}), /* @__PURE__ */ c("p", {
			className: "mt-3 flex items-start gap-1.5 border-t border-line pt-3 font-mono text-[10px] leading-relaxed text-fg-muted",
			children: [/* @__PURE__ */ s(Y, { className: "mt-0.5 shrink-0 text-fg-muted/80" }), /* @__PURE__ */ c("span", { children: [
				"Verify this reflects your intent — the planner only decides",
				" ",
				/* @__PURE__ */ s("span", {
					className: "text-fg-secondary",
					children: "how to query"
				}),
				". The results are exactly what this plan selected, nothing more."
			] })]
		})]
	});
}
function $(e, t) {
	let n = e[t];
	return typeof n == "string" && n.trim() ? n : null;
}
function oe({ proof: e }) {
	let t = r(() => {
		let t = [...e.claim_levels ?? []];
		return t.sort((e, t) => (K[String(e.level ?? "")] ?? 99) - (K[String(t.level ?? "")] ?? 99)), t;
	}, [e.claim_levels]);
	return /* @__PURE__ */ c(b, {
		title: "Proof",
		subtitle: "Why this answer can be trusted — facts at the bottom, interpretation at the top",
		children: [
			/* @__PURE__ */ c("p", {
				className: "mb-4 rounded-md border border-accent/30 bg-accent/[0.06] px-3 py-2 text-xs leading-relaxed text-fg-secondary",
				children: [
					/* @__PURE__ */ s("span", {
						className: "font-semibold text-fg",
						children: "The answers are facts; the interpretation is the only inference."
					}),
					" ",
					"Each rung below states a claim, its confidence, and the caveat that bounds it."
				]
			}),
			/* @__PURE__ */ c("ol", {
				className: "relative space-y-3 pl-5",
				children: [/* @__PURE__ */ s("span", {
					className: "absolute left-[5px] top-2 bottom-2 w-px bg-line",
					"aria-hidden": "true"
				}), t.map((e, t) => {
					let n = $(e, "level") ?? "—", r = G[n] ?? {
						color: "#71717a",
						blurb: ""
					}, i = $(e, "statement"), a = $(e, "confidence"), o = $(e, "caveat"), l = $(e, "basis"), u = $(e, "provenance");
					return /* @__PURE__ */ c("li", {
						className: "relative",
						children: [/* @__PURE__ */ s("span", {
							className: "absolute -left-5 top-1.5 h-2.5 w-2.5 rounded-full border-2 border-canvas",
							style: { backgroundColor: r.color },
							"aria-hidden": "true"
						}), /* @__PURE__ */ c("div", {
							className: "rounded-md border border-line bg-panel2/40 p-3",
							style: {
								borderLeftColor: r.color,
								borderLeftWidth: 3
							},
							children: [
								/* @__PURE__ */ c("div", {
									className: "flex flex-wrap items-center gap-2",
									children: [
										/* @__PURE__ */ s(J, {
											label: n,
											color: r.color
										}),
										r.blurb && /* @__PURE__ */ s("span", {
											className: "font-mono text-[10px] text-fg-muted",
											children: r.blurb
										}),
										a && /* @__PURE__ */ c("span", {
											className: "ml-auto whitespace-nowrap font-mono text-[10px] text-fg-secondary",
											children: ["confidence:\xA0", /* @__PURE__ */ s("span", {
												className: "text-fg",
												children: a
											})]
										})
									]
								}),
								i && /* @__PURE__ */ s("p", {
									className: "mt-2 text-xs leading-relaxed text-fg-secondary",
									children: i
								}),
								l && /* @__PURE__ */ c("p", {
									className: "mt-1.5 font-mono text-[10px] leading-relaxed text-fg-muted",
									children: ["basis: ", l]
								}),
								u && /* @__PURE__ */ c("p", {
									className: "mt-1.5 font-mono text-[10px] leading-relaxed text-fg-muted",
									children: ["provenance: ", u]
								}),
								o && /* @__PURE__ */ c("p", {
									className: "mt-2 flex items-start gap-1.5 rounded border border-connecting/30 bg-connecting/[0.06] px-2 py-1.5 font-mono text-[10px] leading-relaxed text-connecting",
									children: [/* @__PURE__ */ s(Y, { className: "mt-0.5 shrink-0" }), /* @__PURE__ */ s("span", { children: o })]
								})
							]
						})]
					}, `${n}-${t}`);
				})]
			}),
			e.note && /* @__PURE__ */ s("p", {
				className: "mt-4 border-t border-line pt-3 font-mono text-[10px] leading-relaxed text-fg-muted",
				children: e.note
			})
		]
	});
}
function se({ hit: e, nameFor: t }) {
	let n = U[e.source] ?? "#71717a", r = (e.auth_status ? H[e.auth_status] : void 0) ?? n, i = Z(e.subject, "color"), a = Z(e.subject, "vehicle_type"), o = Z(e.subject, "label"), l = Z(e.subject, "subject_type") ?? Z(e.subject, "type"), u = Z(e.subject, "severity");
	return /* @__PURE__ */ c("div", {
		className: "flex gap-3 rounded-md border border-line bg-panel2/40 p-3 transition-colors duration-150 hover:border-[#34373e]",
		style: {
			borderLeftColor: r,
			borderLeftWidth: 3
		},
		children: [e.evidence_path && /* @__PURE__ */ s(te, {
			path: e.evidence_path,
			alt: `${e.source} ${e.plate ?? e.id}`
		}), /* @__PURE__ */ c("div", {
			className: "min-w-0 flex-1",
			children: [
				/* @__PURE__ */ c("div", {
					className: "flex flex-wrap items-center gap-2",
					children: [
						/* @__PURE__ */ s(J, {
							label: e.source,
							color: n
						}),
						/* @__PURE__ */ s("span", {
							className: "font-mono text-[10px] uppercase tracking-micro text-fg-muted",
							children: e.kind
						}),
						e.claim_level && /* @__PURE__ */ s(J, {
							label: e.claim_level,
							color: "#52525b"
						}),
						/* @__PURE__ */ s("span", {
							className: "ml-auto whitespace-nowrap font-mono text-[10px] text-fg-muted",
							children: z(e.timestamp)
						})
					]
				}),
				/* @__PURE__ */ c("div", {
					className: "mt-2 flex flex-wrap items-center gap-2",
					children: [e.plate ? /* @__PURE__ */ s("span", {
						className: "font-mono text-base font-semibold tracking-wide text-fg",
						children: e.plate
					}) : /* @__PURE__ */ s("span", {
						className: "font-mono text-sm text-fg-secondary",
						children: o ?? l ?? "—"
					}), e.auth_status && /* @__PURE__ */ s(M, {
						state: V[e.auth_status] ?? "unknown",
						label: e.auth_status
					})]
				}),
				/* @__PURE__ */ c("div", {
					className: "mt-1.5 flex flex-wrap gap-x-3 gap-y-0.5 font-mono text-[10px] text-fg-secondary",
					children: [
						/* @__PURE__ */ c("span", {
							className: "text-fg-muted",
							children: ["camera:\xA0", /* @__PURE__ */ s("span", {
								className: "text-fg-secondary",
								children: t(e.camera_id)
							})]
						}),
						e.zone && /* @__PURE__ */ c("span", {
							className: "text-fg-muted",
							children: [
								"zone:\xA0",
								/* @__PURE__ */ s("span", {
									className: "text-fg-secondary",
									children: e.zone
								}),
								e.zone_kind ? /* @__PURE__ */ c("span", {
									className: "text-fg-muted",
									children: [
										" (",
										e.zone_kind,
										")"
									]
								}) : null
							]
						}),
						l && e.plate && /* @__PURE__ */ c("span", {
							className: "text-fg-muted",
							children: ["subject:\xA0", /* @__PURE__ */ s("span", {
								className: "text-fg-secondary",
								children: l
							})]
						}),
						a && /* @__PURE__ */ c("span", {
							className: "text-fg-muted",
							children: ["type:\xA0", /* @__PURE__ */ s("span", {
								className: "text-fg-secondary",
								children: a
							})]
						}),
						i && /* @__PURE__ */ c("span", {
							className: "text-fg-muted",
							children: ["color:\xA0", /* @__PURE__ */ s("span", {
								className: "text-fg-secondary",
								children: i
							})]
						}),
						u && /* @__PURE__ */ c("span", {
							className: "text-fg-muted",
							children: ["severity:\xA0", /* @__PURE__ */ s("span", {
								className: "text-fg-secondary",
								children: u
							})]
						})
					]
				})
			]
		})]
	});
}
function ce({ result: e, nameFor: t }) {
	let n = r(() => {
		let t = 0, n = 0, r = 0;
		for (let i of e.hits) i.source === "entry" ? t += 1 : i.source === "zone" ? n += 1 : i.source === "breach" && (r += 1);
		return {
			entry: t,
			zone: n,
			breach: r
		};
	}, [e.hits]);
	return /* @__PURE__ */ c(o, { children: [/* @__PURE__ */ c("div", {
		className: "grid grid-cols-2 gap-px overflow-hidden rounded-panel border border-line bg-line sm:grid-cols-4",
		children: [
			/* @__PURE__ */ s("div", {
				className: "bg-panel px-4 py-3",
				children: /* @__PURE__ */ s(P, {
					label: "Matches",
					value: e.count
				})
			}),
			/* @__PURE__ */ s("div", {
				className: "bg-panel px-4 py-3",
				children: /* @__PURE__ */ s(P, {
					label: "Entry",
					value: n.entry
				})
			}),
			/* @__PURE__ */ s("div", {
				className: "bg-panel px-4 py-3",
				children: /* @__PURE__ */ s(P, {
					label: "Zone",
					value: n.zone
				})
			}),
			/* @__PURE__ */ s("div", {
				className: "bg-panel px-4 py-3",
				children: /* @__PURE__ */ s(P, {
					label: "Breach",
					value: n.breach,
					tone: n.breach > 0 ? "bad" : "default"
				})
			})
		]
	}), /* @__PURE__ */ s(b, {
		title: "Results",
		subtitle: "Stored events matching the executed plan — newest first",
		actions: /* @__PURE__ */ s("span", {
			className: "font-mono text-[11px] tabular-nums text-fg-muted",
			children: e.count
		}),
		children: e.hits.length === 0 ? /* @__PURE__ */ s(I, {
			title: "No matching events",
			hint: "The plan ran cleanly but no stored events matched. Loosen the filters above, widen the time window, or check the interpreted plan."
		}) : /* @__PURE__ */ s("div", {
			className: "space-y-2.5",
			children: e.hits.map((e) => /* @__PURE__ */ s(se, {
				hit: e,
				nameFor: t
			}, `${e.source}-${e.id}`))
		})
	})] });
}
function le({ busy: e, onRun: t }) {
	let [n, r] = a(""), [i, l] = a(""), [u, d] = a(""), [f, p] = a(""), [m, h] = a("");
	function g(e) {
		e.preventDefault();
		let r = {};
		n && (r.sources = [n]), i && (r.auth_status = [i]), u.trim() && (r.color = u.trim());
		let a = B(f);
		a && (r.from = a);
		let o = B(m);
		o && (r.to = o), t(r);
	}
	return /* @__PURE__ */ c("form", {
		onSubmit: g,
		className: "space-y-4",
		children: [/* @__PURE__ */ c("div", {
			className: "grid grid-cols-1 gap-3 sm:grid-cols-2 lg:grid-cols-3",
			children: [
				/* @__PURE__ */ s(O, {
					label: "Source",
					htmlFor: "sf-source",
					children: /* @__PURE__ */ c(D, {
						id: "sf-source",
						value: n,
						onChange: (e) => r(e.target.value),
						children: [
							/* @__PURE__ */ s("option", {
								value: "",
								children: "Any source"
							}),
							/* @__PURE__ */ s("option", {
								value: "entry",
								children: "Entry"
							}),
							/* @__PURE__ */ s("option", {
								value: "zone",
								children: "Zone"
							}),
							/* @__PURE__ */ s("option", {
								value: "breach",
								children: "Breach"
							})
						]
					})
				}),
				/* @__PURE__ */ s(O, {
					label: "Authorization",
					htmlFor: "sf-auth",
					children: /* @__PURE__ */ c(D, {
						id: "sf-auth",
						value: i,
						onChange: (e) => l(e.target.value),
						children: [
							/* @__PURE__ */ s("option", {
								value: "",
								children: "Any status"
							}),
							/* @__PURE__ */ s("option", {
								value: "matched",
								children: "Matched"
							}),
							/* @__PURE__ */ s("option", {
								value: "exception",
								children: "Exception"
							}),
							/* @__PURE__ */ s("option", {
								value: "unmatched",
								children: "Unmatched"
							}),
							/* @__PURE__ */ s("option", {
								value: "blocked",
								children: "Blocked"
							})
						]
					})
				}),
				/* @__PURE__ */ s(O, {
					label: "Color",
					htmlFor: "sf-color",
					children: /* @__PURE__ */ s(E, {
						id: "sf-color",
						value: u,
						onChange: (e) => d(e.target.value),
						placeholder: "white",
						autoComplete: "off"
					})
				}),
				/* @__PURE__ */ s(O, {
					label: "From",
					htmlFor: "sf-from",
					children: /* @__PURE__ */ s(E, {
						id: "sf-from",
						type: "datetime-local",
						step: 1,
						value: f,
						onChange: (e) => p(e.target.value)
					})
				}),
				/* @__PURE__ */ s(O, {
					label: "To",
					htmlFor: "sf-to",
					children: /* @__PURE__ */ s(E, {
						id: "sf-to",
						type: "datetime-local",
						step: 1,
						value: m,
						onChange: (e) => h(e.target.value)
					})
				})
			]
		}), /* @__PURE__ */ s("div", {
			className: "flex justify-end",
			children: /* @__PURE__ */ s(w, {
				type: "submit",
				variant: "primary",
				disabled: e,
				children: e ? /* @__PURE__ */ c(o, { children: [/* @__PURE__ */ s(F, { size: 14 }), "Running…"] }) : "Run structured query"
			})
		})]
	});
}
function ue({ nameFor: t }) {
	let [n, r] = a(""), [i, u] = a(null), [d, f] = a(null), [p, m] = a(null), [h, _] = a(null), [y, x] = a(!1), [S, C] = a(!1), T = e(async (e) => {
		let t = e.trim();
		if (t) {
			r(t), m("nl"), _(null), f(null);
			try {
				u(await g.searchNl(t)), x(!0);
			} catch (e) {
				_(e instanceof l ? e.message : String(e)), u(null), x(!0);
			} finally {
				m(null);
			}
		}
	}, []);
	async function D() {
		let e = n.trim();
		if (e) {
			m("plan"), _(null), u(null);
			try {
				f(await g.searchPlan(e)), x(!0);
			} catch (e) {
				_(e instanceof l ? e.message : String(e)), f(null), x(!0);
			} finally {
				m(null);
			}
		}
	}
	let k = e(async (e) => {
		m("structured"), _(null), f(null);
		try {
			u(await g.searchEvents(e)), x(!0);
		} catch (e) {
			_(e instanceof l ? e.message : String(e)), u(null), x(!0);
		} finally {
			m(null);
		}
	}, []);
	function A(e) {
		e.preventDefault(), T(n);
	}
	let j = d?.plan ?? i?.plan ?? null, M = d?.planner ?? i?.planner ?? "rules";
	return /* @__PURE__ */ c("div", {
		className: "stagger space-y-4",
		children: [
			/* @__PURE__ */ c(ne, { children: [
				/* @__PURE__ */ s("span", {
					className: "text-fg",
					children: "Ask in plain language; the answer is the data."
				}),
				" A planner (transparent rules, or an optional LLM) translates your question into a structured query — that interpretation is the only inference. The plan then runs deterministically over the kernel's stored events, so",
				" ",
				/* @__PURE__ */ s("span", {
					className: "text-fg",
					children: "the answers are facts, the interpretation is the only inference"
				}),
				". Every search is logged; plate-targeted queries are audited."
			] }),
			/* @__PURE__ */ c(b, {
				title: "Ask",
				subtitle: "Natural-language search over entry, zone & breach events",
				children: [
					/* @__PURE__ */ c("form", {
						onSubmit: A,
						className: "flex flex-col gap-3 sm:flex-row sm:items-end",
						children: [/* @__PURE__ */ s("div", {
							className: "min-w-0 flex-1",
							children: /* @__PURE__ */ s(O, {
								label: "Query",
								htmlFor: "nl-query",
								children: /* @__PURE__ */ s(E, {
									id: "nl-query",
									value: n,
									onChange: (e) => r(e.target.value),
									placeholder: "white cars entering after 6pm last week",
									autoComplete: "off"
								})
							})
						}), /* @__PURE__ */ c("div", {
							className: "flex shrink-0 items-center gap-2",
							children: [/* @__PURE__ */ s(w, {
								type: "submit",
								variant: "primary",
								disabled: p !== null || !n.trim(),
								children: p === "nl" ? /* @__PURE__ */ c(o, { children: [/* @__PURE__ */ s(F, { size: 14 }), "Searching…"] }) : "Search"
							}), /* @__PURE__ */ s(w, {
								type: "button",
								onClick: () => void D(),
								disabled: p !== null || !n.trim(),
								children: p === "plan" ? /* @__PURE__ */ c(o, { children: [/* @__PURE__ */ s(F, { size: 14 }), "Planning…"] }) : "Plan only (dry-run)"
							})]
						})]
					}),
					/* @__PURE__ */ c("div", {
						className: "mt-3 flex flex-wrap items-center gap-2",
						children: [/* @__PURE__ */ s("span", {
							className: "font-mono text-[9px] uppercase tracking-micro text-fg-muted",
							children: "Try"
						}), q.map((e) => /* @__PURE__ */ s("button", {
							type: "button",
							disabled: p !== null,
							onClick: () => void T(e),
							className: "rounded-full border border-line bg-canvas px-2.5 py-1 font-mono text-[10px] text-fg-secondary transition-colors duration-150 hover:border-accent/50 hover:text-fg disabled:cursor-not-allowed disabled:opacity-50",
							children: e
						}, e))]
					}),
					/* @__PURE__ */ c("p", {
						className: "mt-3 flex items-center gap-1.5 font-mono text-[10px] uppercase tracking-micro text-fg-muted",
						children: [/* @__PURE__ */ c("svg", {
							viewBox: "0 0 16 16",
							width: "12",
							height: "12",
							fill: "none",
							stroke: "currentColor",
							strokeWidth: "1.5",
							"aria-hidden": "true",
							children: [/* @__PURE__ */ s("rect", {
								x: "3",
								y: "7",
								width: "10",
								height: "7",
								rx: "1.5"
							}), /* @__PURE__ */ s("path", {
								d: "M5.5 7V5a2.5 2.5 0 0 1 5 0v2",
								strokeLinecap: "round"
							})]
						}), "Searches are logged · plate-targeted queries are audited."]
					}),
					h && /* @__PURE__ */ s("div", {
						className: "mt-3",
						children: /* @__PURE__ */ s(X, { children: h })
					}),
					/* @__PURE__ */ c("div", {
						className: "mt-4 border-t border-line pt-3",
						children: [/* @__PURE__ */ c("button", {
							type: "button",
							onClick: () => C((e) => !e),
							className: "flex items-center gap-1.5 font-mono text-[10px] font-semibold uppercase tracking-micro text-fg-secondary transition-colors duration-150 hover:text-fg",
							children: [/* @__PURE__ */ s("svg", {
								viewBox: "0 0 16 16",
								width: "12",
								height: "12",
								fill: "none",
								stroke: "currentColor",
								strokeWidth: "1.6",
								strokeLinecap: "round",
								strokeLinejoin: "round",
								"aria-hidden": "true",
								className: v("transition-transform duration-150", S && "rotate-90"),
								children: /* @__PURE__ */ s("path", { d: "M6 4l4 4-4 4" })
							}), "Structured filters"]
						}), S && /* @__PURE__ */ s("div", {
							className: "mt-3",
							children: /* @__PURE__ */ s(le, {
								busy: p === "structured",
								onRun: (e) => void k(e)
							})
						})]
					})
				]
			}),
			j && /* @__PURE__ */ s(ae, {
				plan: j,
				planner: M,
				nameFor: t,
				dryRun: d != null
			}),
			d && /* @__PURE__ */ c(b, {
				title: "Dry run — not executed",
				subtitle: "The plan above was generated but no query was run",
				children: [/* @__PURE__ */ s("p", {
					className: "text-xs leading-relaxed text-fg-secondary",
					children: "Nothing was read from the fact tables. Review the interpreted plan, then execute it exactly as shown."
				}), /* @__PURE__ */ c("div", {
					className: "mt-3 flex items-center gap-2",
					children: [/* @__PURE__ */ s(w, {
						variant: "primary",
						disabled: p !== null,
						onClick: () => void k(d.plan),
						children: p === "structured" ? /* @__PURE__ */ c(o, { children: [/* @__PURE__ */ s(F, { size: 14 }), "Running…"] }) : "Run this plan"
					}), /* @__PURE__ */ s(w, {
						disabled: p !== null,
						onClick: () => void T(n),
						children: "Re-run as search"
					})]
				})]
			}),
			i && /* @__PURE__ */ c(o, { children: [/* @__PURE__ */ s(ce, {
				result: i,
				nameFor: t
			}), /* @__PURE__ */ s(oe, { proof: i.proof })] }),
			!y && !j && /* @__PURE__ */ s(I, {
				title: "Ask a question to begin",
				hint: "Search in plain language across entry, zone and breach events. The interpreted plan and a proof ladder are shown with every result, so you can always see how the question was read and why the answer holds."
			})
		]
	});
}
function de() {
	let [n, i] = a(null), [o, u] = a(!0), [f, p] = a(!1), [m, h] = a(null), v = e(async () => {
		u(!0), h(null);
		try {
			i(await g.me()), p(!1);
		} catch (e) {
			e instanceof l && e.status === 401 ? (i(null), p(!0)) : h(e instanceof Error ? e.message : String(e));
		} finally {
			u(!1);
		}
	}, []);
	t(() => {
		v();
	}, [v]);
	let y = _(() => g.listCameras(), 0), x = y.data ?? [], S = r(() => {
		let e = /* @__PURE__ */ new Map();
		for (let t of x) e.set(t.id, t.name);
		return e;
	}, [y.data]), C = e((e) => e ? S.get(e) ?? e : "—", [S]);
	async function T() {
		try {
			await g.logout();
		} catch {}
		d(null), i(null), p(!0);
	}
	return f ? /* @__PURE__ */ s(ee, { onSuccess: (e) => {
		i(e), p(!1), h(null);
	} }) : o && !n ? /* @__PURE__ */ c("div", {
		className: "flex min-h-[60vh] items-center justify-center gap-3 text-fg-secondary",
		children: [/* @__PURE__ */ s(F, {}), /* @__PURE__ */ s("span", {
			className: "font-mono text-xs uppercase tracking-micro",
			children: "Authenticating…"
		})]
	}) : m && !n ? /* @__PURE__ */ s("div", {
		className: "mx-auto max-w-md px-4 py-20",
		children: /* @__PURE__ */ c(b, {
			title: "Console unavailable",
			children: [/* @__PURE__ */ s(X, { children: m }), /* @__PURE__ */ s("div", {
				className: "mt-3 flex justify-end",
				children: /* @__PURE__ */ s(w, {
					variant: "primary",
					onClick: () => void v(),
					children: "Retry"
				})
			})]
		})
	}) : n ? /* @__PURE__ */ c("div", {
		className: "mx-auto max-w-[1600px] px-4 py-6 sm:px-6",
		children: [/* @__PURE__ */ s("header", {
			className: "animate-rise",
			children: /* @__PURE__ */ c("div", {
				className: "flex flex-wrap items-end justify-between gap-4",
				children: [/* @__PURE__ */ c("div", {
					className: "min-w-0",
					children: [/* @__PURE__ */ s(L, { children: "Intelligence · Search" }), /* @__PURE__ */ s("h1", {
						className: "mt-1 font-display text-2xl font-extrabold tracking-tight text-fg",
						children: "Semantic Search"
					})]
				}), /* @__PURE__ */ c("div", {
					className: "flex items-center gap-3",
					children: [/* @__PURE__ */ c("div", {
						className: "flex flex-col items-end leading-none",
						children: [/* @__PURE__ */ s("span", {
							className: "font-mono text-[12px] font-semibold text-fg",
							children: n.name
						}), /* @__PURE__ */ c("span", {
							className: "mt-1 font-mono text-[9px] uppercase tracking-micro text-accent",
							children: [n.role, n.kind === "system" && /* @__PURE__ */ s("span", {
								className: "text-fg-muted",
								children: " · auth off"
							})]
						})]
					}), n.kind === "user" && /* @__PURE__ */ s(w, {
						size: "sm",
						onClick: () => void T(),
						children: "Sign out"
					})]
				})]
			})
		}), /* @__PURE__ */ s("div", {
			className: "mt-5",
			children: /* @__PURE__ */ s(ue, { nameFor: C })
		})]
	}) : null;
}
//#endregion
//#region src/modules/search/entry.tsx
var fe = de;
//#endregion
export { fe as default };
