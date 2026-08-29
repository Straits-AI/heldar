import { useCallback as e, useEffect as t, useMemo as n, useState as r } from "react";
import { ApiError as i, Button as a, EmptyState as o, Field as s, Input as c, Login as l, Panel as u, SectionLabel as d, Select as f, Spinner as p, Stat as m, StatusLed as h, StatusPill as g, api as _, cx as v, formatClock as y, formatDuration as b, setAuthToken as x, timeAgo as S, usePoll as C } from "@heldar/shell";
import { Fragment as w, jsx as T, jsxs as E } from "react/jsx-runtime";
//#region src/modules/movement/page.tsx
var D = {
	info: "#71717a",
	warning: "#fbbf24",
	critical: "#ef4444"
}, O = {
	info: "offline",
	warning: "connecting",
	critical: "error"
}, k = {
	open: "#f59e0b",
	acknowledged: "#fbbf24",
	resolved: "#10b981"
}, A = {
	pending: "#fbbf24",
	confirmed: "#10b981",
	rejected: "#ef4444"
}, j = [
	"plate_exact",
	"transit",
	"color_match",
	"type_match",
	"appearance_score"
], M = {
	plate_exact: "Plate exact",
	transit: "Transit",
	color_match: "Color match",
	type_match: "Type match",
	appearance_score: "Appearance"
};
function N({ children: e, className: t }) {
	return /* @__PURE__ */ T("th", {
		className: v("whitespace-nowrap px-3 py-2 text-left font-mono text-[10px] font-medium uppercase tracking-micro text-fg-muted", t),
		children: e
	});
}
function P({ children: e, className: t }) {
	return /* @__PURE__ */ T("td", {
		className: v("px-3 py-2.5 align-top", t),
		children: e
	});
}
function F({ label: e, color: t }) {
	return /* @__PURE__ */ T("span", {
		className: "inline-flex shrink-0 items-center rounded border px-1.5 py-0.5 font-mono text-[9px] font-semibold uppercase tracking-micro leading-none",
		style: {
			color: t,
			borderColor: `${t}55`,
			backgroundColor: `${t}1a`
		},
		children: e
	});
}
function I({ className: e }) {
	return /* @__PURE__ */ E("svg", {
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
			/* @__PURE__ */ T("path", { d: "M8 1.5l6.5 11.5H1.5z" }),
			/* @__PURE__ */ T("path", { d: "M8 6.5v3.5" }),
			/* @__PURE__ */ T("path", { d: "M8 11.6v.4" })
		]
	});
}
function L({ children: e }) {
	return /* @__PURE__ */ E("div", {
		role: "alert",
		className: "flex items-start gap-2 rounded-md border border-danger/40 bg-danger/10 px-3 py-2 font-mono text-xs text-red-300",
		children: [/* @__PURE__ */ T(I, { className: "mt-0.5 shrink-0" }), /* @__PURE__ */ T("span", {
			className: "break-words",
			children: e
		})]
	});
}
function R({ label: e }) {
	return /* @__PURE__ */ E("div", {
		className: "flex items-center gap-2 px-1 py-2 font-mono text-xs text-fg-muted",
		children: [
			/* @__PURE__ */ T(p, { size: 14 }),
			" Loading ",
			e,
			"…"
		]
	});
}
function z(e, t) {
	if (!e) return null;
	let n = e[t];
	return typeof n == "string" ? n.trim() ? n : null : typeof n == "number" || typeof n == "boolean" ? String(n) : null;
}
function B({ path: e, alt: t }) {
	let [n, i] = r(!1);
	return n ? null : /* @__PURE__ */ T("img", {
		src: e,
		alt: t,
		loading: "lazy",
		onError: () => i(!0),
		className: "h-16 w-24 shrink-0 rounded-md border border-line bg-black object-cover"
	});
}
function V({ children: e }) {
	return /* @__PURE__ */ E("div", {
		className: "flex items-start gap-3 rounded-panel border border-line bg-panel px-4 py-3",
		children: [/* @__PURE__ */ E("svg", {
			viewBox: "0 0 20 20",
			className: "mt-0.5 h-4 w-4 shrink-0 text-accent",
			fill: "none",
			stroke: "currentColor",
			strokeWidth: "1.6",
			strokeLinecap: "round",
			strokeLinejoin: "round",
			"aria-hidden": "true",
			children: [
				/* @__PURE__ */ T("circle", {
					cx: "10",
					cy: "10",
					r: "7.5"
				}),
				/* @__PURE__ */ T("path", { d: "M10 9v4" }),
				/* @__PURE__ */ T("path", { d: "M10 6.6v.4" })
			]
		}), /* @__PURE__ */ T("p", {
			className: "font-mono text-[11px] leading-relaxed text-fg-secondary",
			children: e
		})]
	});
}
function H(e) {
	return typeof e == "boolean" ? {
		text: e ? "yes" : "no",
		ok: e
	} : typeof e == "number" && Number.isFinite(e) ? e >= -1 && e <= 1 ? {
		text: `${Math.round(e * 100)}%`,
		ok: e >= .5
	} : {
		text: Number.isInteger(e) ? String(e) : e.toFixed(2),
		ok: null
	} : typeof e == "string" ? {
		text: e,
		ok: null
	} : e == null ? {
		text: "—",
		ok: null
	} : {
		text: String(e),
		ok: null
	};
}
function U(e) {
	return M[e] ?? e.replace(/_/g, " ");
}
function W({ signals: e }) {
	let t = [...j.filter((t) => t in e), ...Object.keys(e).filter((e) => !j.includes(e))];
	return t.length === 0 ? /* @__PURE__ */ T("span", {
		className: "font-mono text-[10px] text-fg-muted",
		children: "No per-signal evidence."
	}) : /* @__PURE__ */ T("div", {
		className: "flex flex-wrap gap-1.5",
		children: t.map((t) => {
			let { text: n, ok: r } = H(e[t]), i = r === !0 ? "#10b981" : r === !1 ? "#52525b" : "#71717a";
			return /* @__PURE__ */ E("span", {
				className: "inline-flex items-center gap-1 rounded border border-line bg-canvas px-1.5 py-0.5 font-mono text-[9px] leading-none",
				children: [/* @__PURE__ */ T("span", {
					className: "uppercase tracking-micro text-fg-muted",
					children: U(t)
				}), /* @__PURE__ */ T("span", {
					className: "font-semibold",
					style: { color: i },
					children: n
				})]
			}, t);
		})
	});
}
function G({ score: e }) {
	let t = Math.max(0, Math.min(100, Math.round(e * 100))), n = t >= 75 ? "#10b981" : t >= 50 ? "#fbbf24" : "#71717a";
	return /* @__PURE__ */ E("div", {
		className: "flex items-center gap-2",
		children: [/* @__PURE__ */ T("div", {
			className: "h-1.5 flex-1 overflow-hidden rounded-full bg-line",
			children: /* @__PURE__ */ T("div", {
				className: "h-full rounded-full transition-[width] duration-500",
				style: {
					width: `${t}%`,
					backgroundColor: n
				}
			})
		}), /* @__PURE__ */ E("span", {
			className: "w-10 shrink-0 text-right font-mono text-xs font-semibold tabular-nums text-fg",
			children: [t, "%"]
		})]
	});
}
function K(e, t) {
	return !e && !t ? "unknown" : e && t ? `${e}: ${t}` : t ?? e ?? "unknown";
}
function q({ b: e, nameFor: t, acting: n, onAck: r, onResolve: i }) {
	let o = D[e.severity] ?? "#52525b", s = z(e.detail, "message") ?? z(e.detail, "reason") ?? z(e.detail, "note"), c = e.status === "open";
	return /* @__PURE__ */ E("div", {
		className: "flex gap-3 rounded-md border border-line bg-panel2/40 p-3 transition-colors duration-150 hover:border-[#34373e]",
		style: {
			borderLeftColor: o,
			borderLeftWidth: 3
		},
		children: [e.evidence_path && /* @__PURE__ */ T(B, {
			path: e.evidence_path,
			alt: `Breach ${e.rule}`
		}), /* @__PURE__ */ E("div", {
			className: "min-w-0 flex-1",
			children: [
				/* @__PURE__ */ E("div", {
					className: "flex flex-wrap items-center gap-2",
					children: [
						c && /* @__PURE__ */ T(h, {
							state: O[e.severity] ?? "error",
							pulse: !0
						}),
						/* @__PURE__ */ T(g, {
							state: O[e.severity] ?? "unknown",
							label: e.severity
						}),
						/* @__PURE__ */ T(F, {
							label: e.status,
							color: k[e.status] ?? "#71717a"
						}),
						/* @__PURE__ */ T("span", {
							className: "ml-auto whitespace-nowrap font-mono text-[10px] text-fg-muted",
							children: y(e.created_at)
						})
					]
				}),
				/* @__PURE__ */ T("div", {
					className: "mt-2 font-display text-sm font-semibold leading-snug text-fg",
					children: e.rule
				}),
				/* @__PURE__ */ E("div", {
					className: "mt-1.5 flex flex-wrap gap-x-3 gap-y-0.5 font-mono text-[10px] text-fg-secondary",
					children: [
						/* @__PURE__ */ E("span", {
							className: "text-fg-muted",
							children: ["subject:\xA0", /* @__PURE__ */ T("span", {
								className: "text-fg-secondary",
								children: K(e.subject_type, e.subject)
							})]
						}),
						/* @__PURE__ */ E("span", {
							className: "text-fg-muted",
							children: ["zone:\xA0", /* @__PURE__ */ T("span", {
								className: "text-fg-secondary",
								children: e.zone_name ?? e.zone_id ?? "—"
							})]
						}),
						/* @__PURE__ */ E("span", {
							className: "text-fg-muted",
							children: ["camera:\xA0", /* @__PURE__ */ T("span", {
								className: "text-fg-secondary",
								children: t(e.camera_id)
							})]
						}),
						e.track_id && /* @__PURE__ */ E("span", {
							className: "text-fg-muted",
							children: ["track:\xA0", /* @__PURE__ */ T("span", {
								className: "text-fg-secondary",
								children: e.track_id
							})]
						})
					]
				}),
				s && /* @__PURE__ */ T("p", {
					className: "mt-1.5 text-xs leading-relaxed text-fg-secondary",
					children: s
				}),
				e.status !== "resolved" && /* @__PURE__ */ E("div", {
					className: "mt-2.5 flex items-center gap-2",
					children: [
						e.status === "open" && /* @__PURE__ */ T(a, {
							size: "sm",
							disabled: n,
							onClick: () => r(e),
							children: "Acknowledge"
						}),
						/* @__PURE__ */ T(a, {
							size: "sm",
							variant: "primary",
							disabled: n,
							onClick: () => i(e),
							children: "Resolve"
						}),
						n && /* @__PURE__ */ T(p, { size: 13 })
					]
				}),
				e.status === "resolved" && e.resolved_at && /* @__PURE__ */ E("div", {
					className: "mt-2 font-mono text-[10px] text-fg-muted",
					children: [
						"resolved ",
						S(e.resolved_at),
						e.resolved_by ? ` · by ${e.resolved_by}` : ""
					]
				})
			]
		})]
	});
}
function J({ reloadKey: e, nameFor: t }) {
	let [a, s] = r("open"), c = C(() => _.movementBreaches(a === "all" ? { limit: 200 } : {
		status: a,
		limit: 200
	}), 5e3, [a, e]), [l, d] = r(null), [p, h] = r(null);
	async function g(e, t) {
		d(e.id), h(null);
		try {
			t === "ack" ? await _.ackBreach(e.id) : await _.resolveBreach(e.id), await c.refresh();
		} catch (e) {
			h(e instanceof i ? e.message : String(e));
		} finally {
			d(null);
		}
	}
	let v = c.data ?? [], y = n(() => {
		let e = 0, t = 0, n = 0;
		for (let r of v) r.severity === "critical" ? e += 1 : r.severity === "warning" ? t += 1 : n += 1;
		return {
			critical: e,
			warning: t,
			info: n
		};
	}, [v]);
	return /* @__PURE__ */ E("div", {
		className: "stagger space-y-4",
		children: [/* @__PURE__ */ E("div", {
			className: "grid grid-cols-2 gap-px overflow-hidden rounded-panel border border-line bg-line sm:grid-cols-4",
			children: [
				/* @__PURE__ */ T("div", {
					className: "bg-panel px-4 py-3",
					children: /* @__PURE__ */ T(m, {
						label: "Showing",
						value: v.length
					})
				}),
				/* @__PURE__ */ T("div", {
					className: "bg-panel px-4 py-3",
					children: /* @__PURE__ */ T(m, {
						label: "Critical",
						value: y.critical,
						tone: y.critical > 0 ? "bad" : "default"
					})
				}),
				/* @__PURE__ */ T("div", {
					className: "bg-panel px-4 py-3",
					children: /* @__PURE__ */ T(m, {
						label: "Warning",
						value: y.warning,
						tone: y.warning > 0 ? "warn" : "default"
					})
				}),
				/* @__PURE__ */ T("div", {
					className: "bg-panel px-4 py-3",
					children: /* @__PURE__ */ T(m, {
						label: "Info",
						value: y.info
					})
				})
			]
		}), /* @__PURE__ */ E(u, {
			title: "Red-Zone Breaches",
			subtitle: "Correlated incidents · refreshes every 5s",
			actions: /* @__PURE__ */ E("div", {
				className: "flex items-center gap-2",
				children: [/* @__PURE__ */ T("div", {
					className: "w-40",
					children: /* @__PURE__ */ E(f, {
						"aria-label": "Breach status filter",
						value: a,
						onChange: (e) => s(e.target.value),
						children: [
							/* @__PURE__ */ T("option", {
								value: "open",
								children: "Open"
							}),
							/* @__PURE__ */ T("option", {
								value: "acknowledged",
								children: "Acknowledged"
							}),
							/* @__PURE__ */ T("option", {
								value: "resolved",
								children: "Resolved"
							}),
							/* @__PURE__ */ T("option", {
								value: "all",
								children: "All"
							})
						]
					})
				}), /* @__PURE__ */ T("span", {
					className: "font-mono text-[11px] tabular-nums text-fg-muted",
					children: v.length
				})]
			}),
			children: [p && /* @__PURE__ */ T("div", {
				className: "mb-3",
				children: /* @__PURE__ */ T(L, { children: p })
			}), c.error && !c.data ? /* @__PURE__ */ E(L, { children: ["Failed to load breaches: ", c.error] }) : v.length === 0 ? c.loading ? /* @__PURE__ */ T(R, { label: "breaches" }) : /* @__PURE__ */ T(o, {
				title: "No breaches",
				hint: "Movement breaches are raised when a tracked subject crosses a restricted zone or violates a movement rule. Confirmed ones appear here colour-coded by severity."
			}) : /* @__PURE__ */ T("div", {
				className: "space-y-2.5",
				children: v.map((e) => /* @__PURE__ */ T(q, {
					b: e,
					nameFor: t,
					acting: l === e.id,
					onAck: (e) => void g(e, "ack"),
					onResolve: (e) => void g(e, "resolve")
				}, e.id))
			})]
		})]
	});
}
function Y({ c: e, nameFor: t, acting: n, onConfirm: r, onReject: i }) {
	let o = A[e.status] ?? "#71717a", s = e.status === "pending" && !!r && !!i;
	return /* @__PURE__ */ E("div", {
		className: "flex flex-col gap-2.5 rounded-md border border-line bg-panel2/40 p-3.5 transition-colors duration-150 hover:border-[#34373e]",
		style: {
			borderLeftColor: o,
			borderLeftWidth: 3
		},
		children: [
			/* @__PURE__ */ E("div", {
				className: "flex flex-wrap items-center gap-2",
				children: [
					/* @__PURE__ */ T(F, {
						label: e.subject_type,
						color: "#71717a"
					}),
					/* @__PURE__ */ T(F, {
						label: e.status,
						color: A[e.status] ?? "#71717a"
					}),
					/* @__PURE__ */ T("span", {
						className: "ml-auto whitespace-nowrap font-mono text-[10px] text-fg-muted",
						children: S(e.created_at)
					})
				]
			}),
			/* @__PURE__ */ E("div", {
				className: "flex items-baseline gap-2",
				children: [/* @__PURE__ */ T("span", {
					className: "font-mono text-[9px] uppercase tracking-micro text-fg-muted",
					children: "anchor"
				}), /* @__PURE__ */ T("span", {
					className: "font-mono text-base font-semibold tracking-wide text-fg",
					children: e.anchor ?? "—"
				})]
			}),
			/* @__PURE__ */ E("div", {
				className: "flex flex-wrap items-center gap-2 font-mono text-[11px] text-fg-secondary",
				children: [
					/* @__PURE__ */ T("span", {
						className: "text-fg",
						children: t(e.from_camera)
					}),
					/* @__PURE__ */ T("svg", {
						viewBox: "0 0 24 12",
						className: "h-2.5 w-6 text-fg-muted",
						fill: "none",
						"aria-hidden": "true",
						children: /* @__PURE__ */ T("path", {
							d: "M0 6h20M16 2l5 4-5 4",
							stroke: "currentColor",
							strokeWidth: "1.6",
							strokeLinecap: "round",
							strokeLinejoin: "round"
						})
					}),
					/* @__PURE__ */ T("span", {
						className: "text-fg",
						children: t(e.to_camera)
					}),
					/* @__PURE__ */ E("span", {
						className: "text-fg-muted",
						children: ["· transit\xA0", /* @__PURE__ */ T("span", {
							className: "text-fg-secondary",
							children: e.transit_seconds == null ? "—" : b(e.transit_seconds)
						})]
					})
				]
			}),
			/* @__PURE__ */ E("div", { children: [/* @__PURE__ */ T("span", {
				className: "font-mono text-[9px] uppercase tracking-micro text-fg-muted",
				children: "Match score"
			}), /* @__PURE__ */ T("div", {
				className: "mt-1",
				children: /* @__PURE__ */ T(G, { score: e.score })
			})] }),
			/* @__PURE__ */ E("div", { children: [/* @__PURE__ */ T("span", {
				className: "font-mono text-[9px] uppercase tracking-micro text-fg-muted",
				children: "Per-signal evidence"
			}), /* @__PURE__ */ T("div", {
				className: "mt-1",
				children: /* @__PURE__ */ T(W, { signals: e.signals })
			})] }),
			/* @__PURE__ */ E("p", {
				className: "flex items-start gap-1.5 border-t border-line pt-2.5 text-[11px] leading-relaxed text-fg-muted",
				children: [/* @__PURE__ */ T(I, { className: "mt-0.5 shrink-0 text-fg-muted/80" }), /* @__PURE__ */ E("span", { children: [
					"This is a ",
					/* @__PURE__ */ T("span", {
						className: "text-fg-secondary",
						children: "candidate, not an identity"
					}),
					" — a probabilistic cross-camera correlation that requires a human decision."
				] })]
			}),
			s ? /* @__PURE__ */ E("div", {
				className: "flex items-center gap-2",
				children: [
					/* @__PURE__ */ T(a, {
						size: "sm",
						variant: "primary",
						disabled: n,
						onClick: () => r(e),
						children: "Confirm"
					}),
					/* @__PURE__ */ T(a, {
						size: "sm",
						variant: "danger",
						disabled: n,
						onClick: () => i(e),
						children: "Reject"
					}),
					n && /* @__PURE__ */ T(p, { size: 13 })
				]
			}) : e.reviewed_at && /* @__PURE__ */ E("div", {
				className: "font-mono text-[10px] text-fg-muted",
				children: [
					"reviewed ",
					S(e.reviewed_at),
					e.reviewed_by ? ` · by ${e.reviewed_by}` : ""
				]
			})
		]
	});
}
function X({ reloadKey: e, nameFor: t }) {
	let n = C(() => _.movementCandidates({
		status: "pending",
		limit: 100
	}), 8e3, [e]), [a, s] = r(null), [c, l] = r(null);
	async function d(e, t) {
		s(e.id), l(null);
		try {
			t === "confirm" ? await _.confirmMovementCandidate(e.id) : await _.rejectMovementCandidate(e.id), await n.refresh();
		} catch (e) {
			l(e instanceof i ? e.message : String(e));
		} finally {
			s(null);
		}
	}
	let f = n.data ?? [];
	return /* @__PURE__ */ E("div", {
		className: "stagger space-y-4",
		children: [/* @__PURE__ */ E(V, { children: [/* @__PURE__ */ T("span", {
			className: "text-fg",
			children: "Candidates, not identities."
		}), " Each card below is a probabilistic match between two camera appearances, anchored on a plate and scored from independent signals. It is a lead for review — not a confirmed identity. Confirm only when the evidence warrants it; reject otherwise. Every decision is attributed and audited."] }), /* @__PURE__ */ E(u, {
			title: "ReID Candidates",
			subtitle: "Pending cross-camera matches awaiting a human decision",
			actions: /* @__PURE__ */ T("span", {
				className: "font-mono text-[11px] tabular-nums text-fg-muted",
				children: f.length
			}),
			children: [c && /* @__PURE__ */ T("div", {
				className: "mb-3",
				children: /* @__PURE__ */ T(L, { children: c })
			}), n.error && !n.data ? /* @__PURE__ */ E(L, { children: ["Failed to load candidates: ", n.error] }) : f.length === 0 ? n.loading ? /* @__PURE__ */ T(R, { label: "candidates" }) : /* @__PURE__ */ T(o, {
				title: "No pending candidates",
				hint: "When movement correlation finds a plausible cross-camera match it queues a candidate here for confirm/reject. Use Recompute to re-run correlation over recent activity."
			}) : /* @__PURE__ */ T("div", {
				className: "grid grid-cols-1 gap-3 md:grid-cols-2 xl:grid-cols-3",
				children: f.map((e) => /* @__PURE__ */ T(Y, {
					c: e,
					nameFor: t,
					acting: a === e.id,
					onConfirm: (e) => void d(e, "confirm"),
					onReject: (e) => void d(e, "reject")
				}, e.id))
			})]
		})]
	});
}
function Z({ id: e, value: t, cameras: n, onChange: r }) {
	return n.length === 0 ? /* @__PURE__ */ T(c, {
		id: e,
		value: t,
		onChange: (e) => r(e.target.value),
		placeholder: "camera-id"
	}) : /* @__PURE__ */ E(f, {
		id: e,
		value: t,
		onChange: (e) => r(e.target.value),
		children: [/* @__PURE__ */ T("option", {
			value: "",
			children: "Select camera…"
		}), n.map((e) => /* @__PURE__ */ T("option", {
			value: e.id,
			children: e.name
		}, e.id))]
	});
}
function Q({ cameras: e, nameFor: t }) {
	let n = C(() => _.movementLinks(), 0), [l, d] = r(""), [m, h] = r(""), [g, v] = r("60"), [y, x] = r(!0), [S, D] = r(""), [O, k] = r(!1), [A, j] = r(null), [M, I] = r(null);
	async function z(e) {
		if (e.preventDefault(), !l.trim() || !m.trim()) {
			j("Both a from-camera and a to-camera are required.");
			return;
		}
		if (l.trim() === m.trim()) {
			j("A link must connect two different cameras.");
			return;
		}
		let t = {
			from_camera: l.trim(),
			to_camera: m.trim(),
			bidirectional: y
		}, r = Number(g);
		g.trim() && Number.isFinite(r) && r > 0 && (t.transit_seconds = r), S.trim() && (t.note = S.trim()), k(!0), j(null);
		try {
			await _.createMovementLink(t), d(""), h(""), v("60"), x(!0), D(""), await n.refresh();
		} catch (e) {
			j(e instanceof i ? e.message : String(e));
		} finally {
			k(!1);
		}
	}
	async function B(e, t) {
		if (window.confirm(`Delete camera link ${t}?`)) {
			I(e);
			try {
				await _.deleteMovementLink(e), await n.refresh();
			} catch {} finally {
				I(null);
			}
		}
	}
	let V = n.data ?? [];
	return /* @__PURE__ */ E("div", {
		className: "grid grid-cols-1 gap-4 lg:grid-cols-3",
		children: [/* @__PURE__ */ T("div", {
			className: "stagger space-y-4 lg:col-span-1",
			children: /* @__PURE__ */ T(u, {
				title: "Add Camera Link",
				subtitle: "Define a plausible transit between two cameras",
				children: /* @__PURE__ */ E("form", {
					onSubmit: z,
					className: "space-y-4",
					children: [
						/* @__PURE__ */ T(s, {
							label: /* @__PURE__ */ E(w, { children: ["From camera ", /* @__PURE__ */ T("span", {
								className: "text-accent",
								children: "*"
							})] }),
							htmlFor: "ml-from",
							children: /* @__PURE__ */ T(Z, {
								id: "ml-from",
								value: l,
								cameras: e,
								onChange: d
							})
						}),
						/* @__PURE__ */ T(s, {
							label: /* @__PURE__ */ E(w, { children: ["To camera ", /* @__PURE__ */ T("span", {
								className: "text-accent",
								children: "*"
							})] }),
							htmlFor: "ml-to",
							children: /* @__PURE__ */ T(Z, {
								id: "ml-to",
								value: m,
								cameras: e,
								onChange: h
							})
						}),
						/* @__PURE__ */ E("div", {
							className: "grid grid-cols-2 gap-3",
							children: [/* @__PURE__ */ T(s, {
								label: "Transit (s)",
								htmlFor: "ml-transit",
								hint: "Expected travel time",
								children: /* @__PURE__ */ T(c, {
									id: "ml-transit",
									type: "number",
									min: 1,
									value: g,
									onChange: (e) => v(e.target.value),
									placeholder: "60"
								})
							}), /* @__PURE__ */ T(s, {
								label: "Direction",
								htmlFor: "ml-dir",
								children: /* @__PURE__ */ E(f, {
									id: "ml-dir",
									value: y ? "both" : "one",
									onChange: (e) => x(e.target.value === "both"),
									children: [/* @__PURE__ */ T("option", {
										value: "both",
										children: "Both directions"
									}), /* @__PURE__ */ T("option", {
										value: "one",
										children: "One-way"
									})]
								})
							})]
						}),
						/* @__PURE__ */ T(s, {
							label: "Note",
							htmlFor: "ml-note",
							children: /* @__PURE__ */ T(c, {
								id: "ml-note",
								value: S,
								onChange: (e) => D(e.target.value),
								placeholder: "Lobby → car park ramp"
							})
						}),
						A && /* @__PURE__ */ T(L, { children: A }),
						/* @__PURE__ */ T("div", {
							className: "flex justify-end",
							children: /* @__PURE__ */ T(a, {
								type: "submit",
								variant: "primary",
								disabled: O,
								children: O ? /* @__PURE__ */ E(w, { children: [/* @__PURE__ */ T(p, { size: 14 }), "Adding…"] }) : "Add link"
							})
						})
					]
				})
			})
		}), /* @__PURE__ */ T("div", {
			className: "stagger space-y-4 lg:col-span-2",
			children: /* @__PURE__ */ T(u, {
				title: "Camera Topology",
				subtitle: "Links that bound plausible cross-camera transits",
				padded: !1,
				actions: V.length > 0 ? /* @__PURE__ */ T("span", {
					className: "font-mono text-[11px] tabular-nums text-fg-muted",
					children: V.length
				}) : void 0,
				children: n.error && !n.data ? /* @__PURE__ */ T("div", {
					className: "p-4",
					children: /* @__PURE__ */ E(L, { children: ["Failed to load links: ", n.error] })
				}) : V.length === 0 ? /* @__PURE__ */ T("div", {
					className: "p-4",
					children: n.loading ? /* @__PURE__ */ T(R, { label: "topology" }) : /* @__PURE__ */ T(o, {
						title: "No camera links",
						hint: "Add a link on the left so movement correlation knows which cameras a subject can plausibly travel between, and how long that transit should take."
					})
				}) : /* @__PURE__ */ T("div", {
					className: "overflow-x-auto",
					children: /* @__PURE__ */ E("table", {
						className: "w-full border-collapse",
						children: [/* @__PURE__ */ T("thead", { children: /* @__PURE__ */ E("tr", { children: [
							/* @__PURE__ */ T(N, { children: "From" }),
							/* @__PURE__ */ T(N, { children: "To" }),
							/* @__PURE__ */ T(N, { children: "Transit" }),
							/* @__PURE__ */ T(N, { children: "Direction" }),
							/* @__PURE__ */ T(N, { children: "Note" }),
							/* @__PURE__ */ T(N, {
								className: "text-right",
								children: "Action"
							})
						] }) }), /* @__PURE__ */ T("tbody", { children: V.map((e) => /* @__PURE__ */ E("tr", {
							className: "border-t border-line transition-colors duration-150 hover:bg-raised/40",
							children: [
								/* @__PURE__ */ T(P, { children: /* @__PURE__ */ T("span", {
									className: "font-mono text-xs font-semibold text-fg",
									children: t(e.from_camera)
								}) }),
								/* @__PURE__ */ T(P, { children: /* @__PURE__ */ T("span", {
									className: "font-mono text-xs font-semibold text-fg",
									children: t(e.to_camera)
								}) }),
								/* @__PURE__ */ T(P, { children: /* @__PURE__ */ T("span", {
									className: "whitespace-nowrap font-mono text-[11px] text-fg-secondary",
									children: b(e.transit_seconds)
								}) }),
								/* @__PURE__ */ T(P, { children: /* @__PURE__ */ T(F, {
									label: e.bidirectional ? "both" : "one-way",
									color: e.bidirectional ? "#10b981" : "#71717a"
								}) }),
								/* @__PURE__ */ T(P, { children: /* @__PURE__ */ T("span", {
									className: "font-mono text-[11px] text-fg-secondary",
									children: e.note ?? "—"
								}) }),
								/* @__PURE__ */ T(P, {
									className: "text-right",
									children: /* @__PURE__ */ T(a, {
										size: "sm",
										variant: "danger",
										disabled: M === e.id,
										onClick: () => void B(e.id, `${t(e.from_camera)} → ${t(e.to_camera)}`),
										children: "Delete"
									})
								})
							]
						}, e.id)) })]
					})
				})
			})
		})]
	});
}
function $({ result: e, nameFor: t }) {
	let r = n(() => [...e.appearances].sort((e, t) => new Date(e.timestamp).getTime() - new Date(t.timestamp).getTime()), [e.appearances]);
	return r.length === 0 ? /* @__PURE__ */ T(o, {
		title: "No appearances",
		hint: "No entry/exit events were recorded for this plate. It may not have passed a camera, or the read may have differed."
	}) : /* @__PURE__ */ E("ol", {
		className: "relative space-y-3 pl-5",
		children: [/* @__PURE__ */ T("span", {
			className: "absolute left-[5px] top-1 bottom-1 w-px bg-line",
			"aria-hidden": "true"
		}), r.map((e) => /* @__PURE__ */ E("li", {
			className: "relative",
			children: [/* @__PURE__ */ T("span", {
				className: "absolute -left-5 top-1.5 h-2.5 w-2.5 rounded-full border-2 border-canvas bg-accent",
				"aria-hidden": "true"
			}), /* @__PURE__ */ E("div", {
				className: "flex flex-wrap items-center gap-2",
				children: [
					/* @__PURE__ */ T("span", {
						className: "font-mono text-xs font-semibold text-fg",
						children: t(e.camera_id)
					}),
					/* @__PURE__ */ T(F, {
						label: e.event_type,
						color: "#71717a"
					}),
					e.direction && e.direction !== "unknown" && /* @__PURE__ */ T("span", {
						className: "font-mono text-[10px] uppercase tracking-micro text-fg-muted",
						children: e.direction
					}),
					e.auth_status && /* @__PURE__ */ T("span", {
						className: "font-mono text-[10px] uppercase tracking-micro text-fg-muted",
						children: e.auth_status
					}),
					/* @__PURE__ */ T("span", {
						className: "ml-auto whitespace-nowrap font-mono text-[10px] text-fg-muted",
						children: y(e.timestamp)
					})
				]
			})]
		}, e.event_id))]
	});
}
function ee({ nameFor: e }) {
	let [t, n] = r(""), [l, d] = r(null), [f, m] = r(!1), [h, g] = r(null), [v, y] = r(!1);
	async function b(e) {
		if (e.preventDefault(), t.trim()) {
			m(!0), g(null);
			try {
				let e = await _.searchPlate(t.trim());
				d(e), y(!0);
			} catch (e) {
				g(e instanceof i ? e.message : String(e)), d(null), y(!0);
			} finally {
				m(!1);
			}
		}
	}
	return /* @__PURE__ */ E("div", {
		className: "stagger space-y-4",
		children: [/* @__PURE__ */ E(u, {
			title: "Plate Trail Search",
			subtitle: "Reconstruct where a plate was seen, in time order",
			children: [
				/* @__PURE__ */ E("form", {
					onSubmit: b,
					className: "flex flex-wrap items-end gap-3",
					children: [/* @__PURE__ */ T("div", {
						className: "w-64",
						children: /* @__PURE__ */ T(s, {
							label: "Plate",
							htmlFor: "search-plate",
							children: /* @__PURE__ */ T(c, {
								id: "search-plate",
								value: t,
								onChange: (e) => n(e.target.value),
								placeholder: "ABC1234",
								autoComplete: "off"
							})
						})
					}), /* @__PURE__ */ T(a, {
						type: "submit",
						variant: "primary",
						disabled: f || !t.trim(),
						children: f ? /* @__PURE__ */ E(w, { children: [/* @__PURE__ */ T(p, { size: 14 }), "Searching…"] }) : "Search"
					})]
				}),
				/* @__PURE__ */ E("p", {
					className: "mt-3 flex items-center gap-1.5 font-mono text-[10px] uppercase tracking-micro text-fg-muted",
					children: [/* @__PURE__ */ E("svg", {
						viewBox: "0 0 16 16",
						width: "12",
						height: "12",
						fill: "none",
						stroke: "currentColor",
						strokeWidth: "1.5",
						"aria-hidden": "true",
						children: [/* @__PURE__ */ T("rect", {
							x: "3",
							y: "7",
							width: "10",
							height: "7",
							rx: "1.5"
						}), /* @__PURE__ */ T("path", {
							d: "M5.5 7V5a2.5 2.5 0 0 1 5 0v2",
							strokeLinecap: "round"
						})]
					}), "This query is audited — the plate, the operator, and the time are recorded."]
				}),
				h && /* @__PURE__ */ T("div", {
					className: "mt-3",
					children: /* @__PURE__ */ T(L, { children: h })
				})
			]
		}), v ? l ? /* @__PURE__ */ E(w, { children: [
			/* @__PURE__ */ E(V, { children: [
				/* @__PURE__ */ T("span", {
					className: "text-fg",
					children: "Probabilistic, not identity."
				}),
				" ",
				l.note
			] }),
			/* @__PURE__ */ T(u, {
				title: "Appearance Trail",
				subtitle: `Plate ${l.plate} · ${l.appearances.length} appearance${l.appearances.length === 1 ? "" : "s"}`,
				children: /* @__PURE__ */ T($, {
					result: l,
					nameFor: e
				})
			}),
			/* @__PURE__ */ T(u, {
				title: "Related Candidates",
				subtitle: "Cross-camera matches anchored on this plate",
				actions: l.candidates.length > 0 ? /* @__PURE__ */ T("span", {
					className: "font-mono text-[11px] tabular-nums text-fg-muted",
					children: l.candidates.length
				}) : void 0,
				children: l.candidates.length === 0 ? /* @__PURE__ */ T(o, {
					title: "No related candidates",
					hint: "No cross-camera correlation candidates reference this plate yet."
				}) : /* @__PURE__ */ T("div", {
					className: "grid grid-cols-1 gap-3 md:grid-cols-2 xl:grid-cols-3",
					children: l.candidates.map((t) => /* @__PURE__ */ T(Y, {
						c: t,
						nameFor: e
					}, t.id))
				})
			})
		] }) : null : /* @__PURE__ */ T(o, {
			title: "Search a plate to begin",
			hint: "Enter a plate above to retrieve its time-ordered appearances across cameras, plus any cross-camera candidates. Results are probabilistic and never assert identity."
		})]
	});
}
function te({ active: e, onClick: t, children: n }) {
	return /* @__PURE__ */ T("button", {
		type: "button",
		onClick: t,
		className: v("relative -mb-px whitespace-nowrap border-b-2 px-3.5 py-2.5 font-mono text-[11px] font-semibold uppercase tracking-micro transition-colors duration-150 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-accent focus-visible:ring-offset-2 focus-visible:ring-offset-canvas", e ? "border-accent text-fg" : "border-transparent text-fg-muted hover:text-fg-secondary"),
		children: n
	});
}
function ne() {
	let [o, s] = r(null), [c, f] = r(!0), [m, h] = r(!1), [g, v] = r(null), [y, b] = r("breaches"), S = e(async () => {
		f(!0), v(null);
		try {
			let e = await _.me();
			s(e), h(!1);
		} catch (e) {
			e instanceof i && e.status === 401 ? (s(null), h(!0)) : v(e instanceof Error ? e.message : String(e));
		} finally {
			f(!1);
		}
	}, []);
	t(() => {
		S();
	}, [S]);
	let D = C(() => _.listCameras(), 0), O = D.data ?? [], k = n(() => {
		let e = /* @__PURE__ */ new Map();
		for (let t of O) e.set(t.id, t.name);
		return e;
	}, [D.data]), A = e((e) => e ? k.get(e) ?? e : "—", [k]), [j, M] = r(0), [N, P] = r(!1), [F, I] = r(null);
	async function R() {
		P(!0), I(null);
		try {
			await _.triggerMovement(), M((e) => e + 1);
		} catch (e) {
			I(e instanceof i ? e.message : String(e));
		} finally {
			P(!1);
		}
	}
	async function z() {
		try {
			await _.logout();
		} catch {}
		x(null), s(null), h(!0);
	}
	return m ? /* @__PURE__ */ T(l, { onSuccess: (e) => {
		s(e), h(!1), v(null);
	} }) : c && !o ? /* @__PURE__ */ E("div", {
		className: "flex min-h-[60vh] items-center justify-center gap-3 text-fg-secondary",
		children: [/* @__PURE__ */ T(p, {}), /* @__PURE__ */ T("span", {
			className: "font-mono text-xs uppercase tracking-micro",
			children: "Authenticating…"
		})]
	}) : g && !o ? /* @__PURE__ */ T("div", {
		className: "mx-auto max-w-md px-4 py-20",
		children: /* @__PURE__ */ E(u, {
			title: "Console unavailable",
			children: [/* @__PURE__ */ T(L, { children: g }), /* @__PURE__ */ T("div", {
				className: "mt-3 flex justify-end",
				children: /* @__PURE__ */ T(a, {
					variant: "primary",
					onClick: () => void S(),
					children: "Retry"
				})
			})]
		})
	}) : o ? /* @__PURE__ */ E("div", {
		className: "mx-auto max-w-[1600px] px-4 py-6 sm:px-6",
		children: [
			/* @__PURE__ */ E("header", {
				className: "animate-rise",
				children: [/* @__PURE__ */ E("div", {
					className: "flex flex-wrap items-end justify-between gap-4",
					children: [/* @__PURE__ */ E("div", {
						className: "min-w-0",
						children: [/* @__PURE__ */ T(d, { children: "Intelligence · Movement" }), /* @__PURE__ */ T("h1", {
							className: "mt-1 font-display text-2xl font-extrabold tracking-tight text-fg",
							children: "Movement Intelligence"
						})]
					}), /* @__PURE__ */ E("div", {
						className: "flex items-center gap-3",
						children: [
							/* @__PURE__ */ T(a, {
								onClick: () => void R(),
								disabled: N,
								children: N ? /* @__PURE__ */ E(w, { children: [/* @__PURE__ */ T(p, { size: 14 }), "Recomputing…"] }) : "Recompute"
							}),
							/* @__PURE__ */ E("div", {
								className: "flex flex-col items-end leading-none",
								children: [/* @__PURE__ */ T("span", {
									className: "font-mono text-[12px] font-semibold text-fg",
									children: o.name
								}), /* @__PURE__ */ E("span", {
									className: "mt-1 font-mono text-[9px] uppercase tracking-micro text-accent",
									children: [o.role, o.kind === "system" && /* @__PURE__ */ T("span", {
										className: "text-fg-muted",
										children: " · auth off"
									})]
								})]
							}),
							o.kind === "user" && /* @__PURE__ */ T(a, {
								size: "sm",
								onClick: () => void z(),
								children: "Sign out"
							})
						]
					})]
				}), /* @__PURE__ */ T("div", {
					className: "mt-5 flex flex-wrap gap-1 overflow-x-auto border-b border-line",
					children: [
						{
							key: "breaches",
							label: "Breaches"
						},
						{
							key: "candidates",
							label: "ReID Candidates"
						},
						{
							key: "topology",
							label: "Topology"
						},
						{
							key: "search",
							label: "Search"
						}
					].map((e) => /* @__PURE__ */ T(te, {
						active: y === e.key,
						onClick: () => b(e.key),
						children: e.label
					}, e.key))
				})]
			}),
			/* @__PURE__ */ T("div", {
				className: "mt-5 animate-rise",
				children: /* @__PURE__ */ E(V, { children: [
					/* @__PURE__ */ T("span", {
						className: "text-fg",
						children: "Probabilistic, human-in-the-loop."
					}),
					" Movement intelligence correlates anonymous tracks and plate reads across cameras to surface plausible transits and red-zone breaches. Nothing here is a confirmed identity — correlations are",
					" ",
					/* @__PURE__ */ T("span", {
						className: "text-fg",
						children: "candidates an operator confirms or rejects"
					}),
					", breach reviews are gated by role, and every decision and search is audited."
				] })
			}),
			F && /* @__PURE__ */ T("div", {
				className: "mt-3 animate-rise",
				children: /* @__PURE__ */ T(L, { children: F })
			}),
			/* @__PURE__ */ E("div", {
				className: "mt-5",
				children: [
					y === "breaches" && /* @__PURE__ */ T(J, {
						reloadKey: j,
						nameFor: A
					}),
					y === "candidates" && /* @__PURE__ */ T(X, {
						reloadKey: j,
						nameFor: A
					}),
					y === "topology" && /* @__PURE__ */ T(Q, {
						cameras: O,
						nameFor: A
					}),
					y === "search" && /* @__PURE__ */ T(ee, { nameFor: A })
				]
			})
		]
	}) : null;
}
//#endregion
//#region src/modules/movement/entry.tsx
var re = ne;
//#endregion
export { re as default };
