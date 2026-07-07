import { useCallback as e, useEffect as t, useMemo as n, useState as r } from "react";
import { ApiError as i, Button as a, EmptyState as o, Field as s, Input as c, Login as l, Panel as u, SectionLabel as d, Select as f, Spinner as p, Stat as m, StatusPill as h, api as g, cx as _, formatClock as v, localInputToIso as y, setAuthToken as b, timeAgo as x, usePoll as S } from "@heldar/shell";
import { Fragment as C, jsx as w, jsxs as T } from "react/jsx-runtime";
//#region src/modules/entry/page.tsx
var E = {
	matched: "recording",
	exception: "connecting",
	blocked: "error",
	unmatched: "offline"
}, D = {
	matched: "#10b981",
	exception: "#fbbf24",
	blocked: "#ef4444",
	unmatched: "#52525b"
}, O = {
	pending: "#fbbf24",
	confirmed: "#10b981",
	rejected: "#ef4444",
	auto: "#71717a"
}, k = {
	active: "#fbbf24",
	checked_in: "#10b981",
	checked_out: "#71717a",
	expired: "#52525b",
	revoked: "#ef4444"
}, A = {
	block: "#ef4444",
	vip: "#f59e0b",
	alert: "#fbbf24"
}, j = {
	info: "#71717a",
	warning: "#fbbf24",
	critical: "#ef4444"
}, M = [
	"admin",
	"manager",
	"guard",
	"viewer",
	"integration"
], N = [
	"student",
	"staff",
	"resident",
	"contractor",
	"visitor"
];
function P({ children: e, className: t }) {
	return /* @__PURE__ */ w("th", {
		className: _("whitespace-nowrap px-3 py-2 text-left font-mono text-[10px] font-medium uppercase tracking-micro text-fg-muted", t),
		children: e
	});
}
function F({ children: e, className: t }) {
	return /* @__PURE__ */ w("td", {
		className: _("px-3 py-2.5 align-top", t),
		children: e
	});
}
function I({ label: e, color: t }) {
	return /* @__PURE__ */ w("span", {
		className: "inline-flex shrink-0 items-center rounded border px-1.5 py-0.5 font-mono text-[9px] font-semibold uppercase tracking-micro leading-none",
		style: {
			color: t,
			borderColor: `${t}55`,
			backgroundColor: `${t}1a`
		},
		children: e
	});
}
function L({ className: e }) {
	return /* @__PURE__ */ T("svg", {
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
			/* @__PURE__ */ w("path", { d: "M8 1.5l6.5 11.5H1.5z" }),
			/* @__PURE__ */ w("path", { d: "M8 6.5v3.5" }),
			/* @__PURE__ */ w("path", { d: "M8 11.6v.4" })
		]
	});
}
function R({ children: e }) {
	return /* @__PURE__ */ T("div", {
		role: "alert",
		className: "flex items-start gap-2 rounded-md border border-danger/40 bg-danger/10 px-3 py-2 font-mono text-xs text-red-300",
		children: [/* @__PURE__ */ w(L, { className: "mt-0.5 shrink-0" }), /* @__PURE__ */ w("span", {
			className: "break-words",
			children: e
		})]
	});
}
function z({ label: e }) {
	return /* @__PURE__ */ T("div", {
		className: "flex items-center gap-2 px-1 py-2 font-mono text-xs text-fg-muted",
		children: [
			/* @__PURE__ */ w(p, { size: 14 }),
			" Loading ",
			e,
			"…"
		]
	});
}
function B(e, t) {
	if (!e) return null;
	let n = e[t];
	return typeof n == "string" ? n.trim() ? n : null : typeof n == "number" || typeof n == "boolean" ? String(n) : null;
}
function V({ path: e, alt: t }) {
	let [n, i] = r(!1);
	return n ? null : /* @__PURE__ */ w("img", {
		src: e,
		alt: t,
		loading: "lazy",
		onError: () => i(!0),
		className: "h-16 w-24 shrink-0 rounded-md border border-line bg-black object-cover"
	});
}
function H({ ev: e, canOperate: t, acting: n, onConfirm: r, onReject: i }) {
	let o = D[e.auth_status] ?? "#52525b", s = B(e.evidence, "snapshot_path"), c = B(e.authorization, "source"), l = B(e.subject, "vehicle_type"), u = B(e.subject, "color"), d = e.workflow_status === "pending" && !!t && !!r && !!i;
	return /* @__PURE__ */ T("div", {
		className: "flex gap-3 rounded-md border border-line bg-panel2/40 p-3 transition-colors duration-150 hover:border-[#34373e]",
		style: {
			borderLeftColor: o,
			borderLeftWidth: 3
		},
		children: [s && /* @__PURE__ */ w(V, {
			path: s,
			alt: `Entry ${e.plate ?? e.id}`
		}), /* @__PURE__ */ T("div", {
			className: "min-w-0 flex-1",
			children: [
				/* @__PURE__ */ T("div", {
					className: "flex flex-wrap items-center gap-2",
					children: [
						/* @__PURE__ */ w(h, {
							state: E[e.auth_status] ?? "unknown",
							label: e.auth_status
						}),
						/* @__PURE__ */ w(I, {
							label: e.workflow_status,
							color: O[e.workflow_status] ?? "#71717a"
						}),
						/* @__PURE__ */ w("span", {
							className: "ml-auto whitespace-nowrap font-mono text-[10px] text-fg-muted",
							children: v(e.timestamp)
						})
					]
				}),
				/* @__PURE__ */ T("div", {
					className: "mt-2 flex flex-wrap items-baseline gap-x-2 gap-y-1",
					children: [
						/* @__PURE__ */ w("span", {
							className: "font-mono text-base font-semibold tracking-wide text-fg",
							children: e.plate ?? "—"
						}),
						e.plate_confidence != null && /* @__PURE__ */ T("span", {
							className: "font-mono text-[10px] text-fg-muted",
							children: [(e.plate_confidence * 100).toFixed(0), "%"]
						}),
						/* @__PURE__ */ w("span", {
							className: "font-mono text-[10px] uppercase tracking-micro text-fg-muted",
							children: e.event_type
						})
					]
				}),
				/* @__PURE__ */ T("div", {
					className: "mt-1 flex flex-wrap gap-x-3 gap-y-0.5 font-mono text-[10px] text-fg-secondary",
					children: [
						/* @__PURE__ */ T("span", {
							className: "text-fg-muted",
							children: ["dir:\xA0", /* @__PURE__ */ w("span", {
								className: "text-fg-secondary",
								children: e.direction
							})]
						}),
						l && /* @__PURE__ */ T("span", {
							className: "text-fg-muted",
							children: ["type:\xA0", /* @__PURE__ */ w("span", {
								className: "text-fg-secondary",
								children: l
							})]
						}),
						u && /* @__PURE__ */ T("span", {
							className: "text-fg-muted",
							children: ["color:\xA0", /* @__PURE__ */ w("span", {
								className: "text-fg-secondary",
								children: u
							})]
						}),
						c && /* @__PURE__ */ T("span", {
							className: "text-fg-muted",
							children: ["src:\xA0", /* @__PURE__ */ w("span", {
								className: "text-fg-secondary",
								children: c
							})]
						})
					]
				}),
				d && /* @__PURE__ */ T("div", {
					className: "mt-2.5 flex items-center gap-2",
					children: [
						/* @__PURE__ */ w(a, {
							size: "sm",
							variant: "primary",
							disabled: n,
							onClick: () => r(e),
							children: "Confirm"
						}),
						/* @__PURE__ */ w(a, {
							size: "sm",
							variant: "danger",
							disabled: n,
							onClick: () => i(e),
							children: "Reject"
						}),
						n && /* @__PURE__ */ w(p, { size: 13 })
					]
				})
			]
		})]
	});
}
function U({ canOperate: e }) {
	let t = S(() => g.listEntryEvents({ limit: 50 }), 3e3), [n, a] = r(null), [s, c] = r(null);
	async function l(e, n) {
		let r = window.prompt(`${n === "confirm" ? "Confirm" : "Reject"} entry ${e.plate ?? ""} — optional note:`, "");
		if (r === null) return;
		let o = r.trim() ? r.trim() : void 0;
		a(e.id), c(null);
		try {
			n === "confirm" ? await g.confirmEntryEvent(e.id, o) : await g.rejectEntryEvent(e.id, o), await t.refresh();
		} catch (e) {
			c(e instanceof i ? e.message : String(e));
		} finally {
			a(null);
		}
	}
	let d = t.data ?? [], f = d.filter((e) => e.workflow_status === "pending").length;
	return /* @__PURE__ */ T("div", {
		className: "stagger space-y-4",
		children: [/* @__PURE__ */ T("div", {
			className: "grid grid-cols-2 gap-px overflow-hidden rounded-panel border border-line bg-line sm:grid-cols-4",
			children: [
				/* @__PURE__ */ w("div", {
					className: "bg-panel px-4 py-3",
					children: /* @__PURE__ */ w(m, {
						label: "Events",
						value: d.length
					})
				}),
				/* @__PURE__ */ w("div", {
					className: "bg-panel px-4 py-3",
					children: /* @__PURE__ */ w(m, {
						label: "Pending",
						value: f,
						tone: f > 0 ? "warn" : "default"
					})
				}),
				/* @__PURE__ */ w("div", {
					className: "bg-panel px-4 py-3",
					children: /* @__PURE__ */ w(m, {
						label: "Blocked",
						value: d.filter((e) => e.auth_status === "blocked").length,
						tone: d.some((e) => e.auth_status === "blocked") ? "bad" : "default"
					})
				}),
				/* @__PURE__ */ w("div", {
					className: "bg-panel px-4 py-3",
					children: /* @__PURE__ */ w(m, {
						label: "Matched",
						value: d.filter((e) => e.auth_status === "matched").length,
						tone: "good"
					})
				})
			]
		}), /* @__PURE__ */ T(u, {
			title: "Live Entry Feed",
			subtitle: "Newest first · refreshes every 3s",
			actions: /* @__PURE__ */ w("span", {
				className: "font-mono text-[11px] tabular-nums text-fg-muted",
				children: d.length
			}),
			children: [s && /* @__PURE__ */ w("div", {
				className: "mb-3",
				children: /* @__PURE__ */ w(R, { children: s })
			}), t.error && !t.data ? /* @__PURE__ */ T(R, { children: ["Failed to load entry events: ", t.error] }) : d.length === 0 ? t.loading ? /* @__PURE__ */ w(z, { label: "entry feed" }) : /* @__PURE__ */ w(o, {
				title: "No entry events",
				hint: "Entry and exit events from the gate cameras appear here as they are detected."
			}) : /* @__PURE__ */ w("div", {
				className: "space-y-2.5",
				children: d.map((t) => /* @__PURE__ */ w(H, {
					ev: t,
					canOperate: e,
					acting: n === t.id,
					onConfirm: (e) => void l(e, "confirm"),
					onReject: (e) => void l(e, "reject")
				}, t.id))
			})]
		})]
	});
}
function W() {
	let e = S(() => g.listPasses({ limit: 100 }), 8e3), [t, n] = r(""), [l, d] = r(""), [f, m] = r(""), [h, _] = r(""), [b, x] = r(""), [E, D] = r(""), [O, A] = r(""), [j, M] = r(!1), [N, L] = r(null), [B, V] = r(null);
	async function H(r) {
		if (r.preventDefault(), !t.trim()) {
			L("Visitor name is required.");
			return;
		}
		let a = { visitor_name: t.trim() };
		l.trim() && (a.phone = l.trim()), f.trim() && (a.host = f.trim()), h.trim() && (a.purpose = h.trim()), b.trim() && (a.plate = b.trim());
		let o = y(E);
		o && (a.valid_from = o);
		let s = y(O);
		s && (a.valid_until = s), M(!0), L(null);
		try {
			await g.createPass(a), n(""), d(""), m(""), _(""), x(""), D(""), A(""), await e.refresh();
		} catch (e) {
			L(e instanceof i ? e.message : String(e));
		} finally {
			M(!1);
		}
	}
	async function U(t, n) {
		V(t);
		try {
			n === "checkin" ? await g.checkinPass(t) : await g.checkoutPass(t), await e.refresh();
		} catch {} finally {
			V(null);
		}
	}
	let W = e.data ?? [];
	return /* @__PURE__ */ T("div", {
		className: "grid grid-cols-1 gap-4 lg:grid-cols-3",
		children: [/* @__PURE__ */ w("div", {
			className: "stagger space-y-4 lg:col-span-1",
			children: /* @__PURE__ */ w(u, {
				title: "Register Visitor",
				subtitle: "Issue a new visitor pass",
				children: /* @__PURE__ */ T("form", {
					onSubmit: H,
					className: "space-y-4",
					children: [
						/* @__PURE__ */ w(s, {
							label: /* @__PURE__ */ T(C, { children: ["Visitor name ", /* @__PURE__ */ w("span", {
								className: "text-accent",
								children: "*"
							})] }),
							htmlFor: "v-name",
							children: /* @__PURE__ */ w(c, {
								id: "v-name",
								value: t,
								onChange: (e) => n(e.target.value),
								placeholder: "Jane Doe",
								required: !0
							})
						}),
						/* @__PURE__ */ T("div", {
							className: "grid grid-cols-2 gap-3",
							children: [/* @__PURE__ */ w(s, {
								label: "Phone",
								htmlFor: "v-phone",
								children: /* @__PURE__ */ w(c, {
									id: "v-phone",
									value: l,
									onChange: (e) => d(e.target.value),
									placeholder: "+60…"
								})
							}), /* @__PURE__ */ w(s, {
								label: "Plate",
								htmlFor: "v-plate",
								children: /* @__PURE__ */ w(c, {
									id: "v-plate",
									value: b,
									onChange: (e) => x(e.target.value),
									placeholder: "ABC1234"
								})
							})]
						}),
						/* @__PURE__ */ w(s, {
							label: "Host",
							htmlFor: "v-host",
							children: /* @__PURE__ */ w(c, {
								id: "v-host",
								value: f,
								onChange: (e) => m(e.target.value),
								placeholder: "Dept / staff name"
							})
						}),
						/* @__PURE__ */ w(s, {
							label: "Purpose",
							htmlFor: "v-purpose",
							children: /* @__PURE__ */ w(c, {
								id: "v-purpose",
								value: h,
								onChange: (e) => _(e.target.value),
								placeholder: "Meeting, delivery…"
							})
						}),
						/* @__PURE__ */ T("div", {
							className: "grid grid-cols-1 gap-3 sm:grid-cols-2",
							children: [/* @__PURE__ */ w(s, {
								label: "Valid from",
								htmlFor: "v-from",
								children: /* @__PURE__ */ w(c, {
									id: "v-from",
									type: "datetime-local",
									step: 1,
									value: E,
									onChange: (e) => D(e.target.value)
								})
							}), /* @__PURE__ */ w(s, {
								label: "Valid until",
								htmlFor: "v-until",
								children: /* @__PURE__ */ w(c, {
									id: "v-until",
									type: "datetime-local",
									step: 1,
									value: O,
									onChange: (e) => A(e.target.value)
								})
							})]
						}),
						N && /* @__PURE__ */ w(R, { children: N }),
						/* @__PURE__ */ w("div", {
							className: "flex justify-end",
							children: /* @__PURE__ */ w(a, {
								type: "submit",
								variant: "primary",
								disabled: j,
								children: j ? /* @__PURE__ */ T(C, { children: [/* @__PURE__ */ w(p, { size: 14 }), "Registering…"] }) : "Register visitor"
							})
						})
					]
				})
			})
		}), /* @__PURE__ */ w("div", {
			className: "stagger space-y-4 lg:col-span-2",
			children: /* @__PURE__ */ w(u, {
				title: "Visitor Passes",
				subtitle: "Active & recent passes",
				padded: !1,
				actions: W.length > 0 ? /* @__PURE__ */ w("span", {
					className: "font-mono text-[11px] tabular-nums text-fg-muted",
					children: W.length
				}) : void 0,
				children: e.error && !e.data ? /* @__PURE__ */ w("div", {
					className: "p-4",
					children: /* @__PURE__ */ T(R, { children: ["Failed to load passes: ", e.error] })
				}) : W.length === 0 ? /* @__PURE__ */ w("div", {
					className: "p-4",
					children: e.loading ? /* @__PURE__ */ w(z, { label: "passes" }) : /* @__PURE__ */ w(o, {
						title: "No visitor passes",
						hint: "Register a visitor on the left to issue the first pass."
					})
				}) : /* @__PURE__ */ w("div", {
					className: "overflow-x-auto",
					children: /* @__PURE__ */ T("table", {
						className: "w-full border-collapse",
						children: [/* @__PURE__ */ w("thead", { children: /* @__PURE__ */ T("tr", { children: [
							/* @__PURE__ */ w(P, { children: "Visitor" }),
							/* @__PURE__ */ w(P, { children: "Host" }),
							/* @__PURE__ */ w(P, { children: "Plate" }),
							/* @__PURE__ */ w(P, { children: "Status" }),
							/* @__PURE__ */ w(P, { children: "Valid until" }),
							/* @__PURE__ */ w(P, {
								className: "text-right",
								children: "Action"
							})
						] }) }), /* @__PURE__ */ w("tbody", { children: W.map((e) => /* @__PURE__ */ T("tr", {
							className: "border-t border-line transition-colors duration-150 hover:bg-raised/40",
							children: [
								/* @__PURE__ */ T(F, { children: [/* @__PURE__ */ w("span", {
									className: "block truncate text-sm font-medium text-fg",
									children: e.visitor_name
								}), /* @__PURE__ */ w("span", {
									className: "block truncate font-mono text-[10px] text-fg-muted",
									children: e.code
								})] }),
								/* @__PURE__ */ w(F, { children: /* @__PURE__ */ w("span", {
									className: "font-mono text-xs text-fg-secondary",
									children: e.host ?? "—"
								}) }),
								/* @__PURE__ */ w(F, { children: /* @__PURE__ */ w("span", {
									className: "font-mono text-xs text-fg",
									children: e.plate ?? "—"
								}) }),
								/* @__PURE__ */ w(F, { children: /* @__PURE__ */ w(I, {
									label: e.status.replace("_", " "),
									color: k[e.status] ?? "#71717a"
								}) }),
								/* @__PURE__ */ w(F, { children: /* @__PURE__ */ w("span", {
									className: "whitespace-nowrap font-mono text-[11px] text-fg-secondary",
									children: v(e.valid_until)
								}) }),
								/* @__PURE__ */ w(F, {
									className: "text-right",
									children: e.status === "active" ? /* @__PURE__ */ w(a, {
										size: "sm",
										variant: "primary",
										disabled: B === e.id,
										onClick: () => void U(e.id, "checkin"),
										children: "Check-in"
									}) : e.status === "checked_in" ? /* @__PURE__ */ w(a, {
										size: "sm",
										disabled: B === e.id,
										onClick: () => void U(e.id, "checkout"),
										children: "Check-out"
									}) : /* @__PURE__ */ w("span", {
										className: "font-mono text-[10px] text-fg-muted",
										children: "—"
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
function G() {
	let e = S(() => g.listVehicles({ limit: 200 }), 15e3), [t, n] = r(""), [l, d] = r(""), [m, h] = r("staff"), [_, v] = r(""), [y, b] = r(""), [x, E] = r(""), [D, O] = r(""), [k, A] = r(!1), [j, M] = r(null), [L, B] = r(null);
	async function V(r) {
		if (r.preventDefault(), !t.trim()) {
			M("Plate is required.");
			return;
		}
		let a = {
			plate: t.trim(),
			owner_type: m
		};
		l.trim() && (a.owner_name = l.trim()), _.trim() && (a.vehicle_type = _.trim()), y.trim() && (a.make = y.trim()), x.trim() && (a.model = x.trim()), D.trim() && (a.color = D.trim()), A(!0), M(null);
		try {
			await g.createVehicle(a), n(""), d(""), v(""), b(""), E(""), O(""), await e.refresh();
		} catch (e) {
			M(e instanceof i ? e.message : String(e));
		} finally {
			A(!1);
		}
	}
	async function H(t, n) {
		if (window.confirm(`Delete vehicle ${n}? This cannot be undone.`)) {
			B(t);
			try {
				await g.deleteVehicle(t), await e.refresh();
			} catch {} finally {
				B(null);
			}
		}
	}
	let U = e.data ?? [];
	return /* @__PURE__ */ T("div", {
		className: "grid grid-cols-1 gap-4 lg:grid-cols-3",
		children: [/* @__PURE__ */ w("div", {
			className: "stagger space-y-4 lg:col-span-1",
			children: /* @__PURE__ */ w(u, {
				title: "Register Vehicle",
				subtitle: "Add to the authorized registry",
				children: /* @__PURE__ */ T("form", {
					onSubmit: V,
					className: "space-y-4",
					children: [
						/* @__PURE__ */ w(s, {
							label: /* @__PURE__ */ T(C, { children: ["Plate ", /* @__PURE__ */ w("span", {
								className: "text-accent",
								children: "*"
							})] }),
							htmlFor: "ve-plate",
							children: /* @__PURE__ */ w(c, {
								id: "ve-plate",
								value: t,
								onChange: (e) => n(e.target.value),
								placeholder: "ABC1234",
								required: !0
							})
						}),
						/* @__PURE__ */ w(s, {
							label: "Owner name",
							htmlFor: "ve-owner",
							children: /* @__PURE__ */ w(c, {
								id: "ve-owner",
								value: l,
								onChange: (e) => d(e.target.value),
								placeholder: "John Smith"
							})
						}),
						/* @__PURE__ */ w(s, {
							label: "Owner type",
							htmlFor: "ve-otype",
							children: /* @__PURE__ */ w(f, {
								id: "ve-otype",
								value: m,
								onChange: (e) => h(e.target.value),
								children: N.map((e) => /* @__PURE__ */ w("option", {
									value: e,
									children: e[0].toUpperCase() + e.slice(1)
								}, e))
							})
						}),
						/* @__PURE__ */ T("div", {
							className: "grid grid-cols-2 gap-3",
							children: [
								/* @__PURE__ */ w(s, {
									label: "Vehicle type",
									htmlFor: "ve-vtype",
									children: /* @__PURE__ */ w(c, {
										id: "ve-vtype",
										value: _,
										onChange: (e) => v(e.target.value),
										placeholder: "car / van"
									})
								}),
								/* @__PURE__ */ w(s, {
									label: "Color",
									htmlFor: "ve-color",
									children: /* @__PURE__ */ w(c, {
										id: "ve-color",
										value: D,
										onChange: (e) => O(e.target.value),
										placeholder: "silver"
									})
								}),
								/* @__PURE__ */ w(s, {
									label: "Make",
									htmlFor: "ve-make",
									children: /* @__PURE__ */ w(c, {
										id: "ve-make",
										value: y,
										onChange: (e) => b(e.target.value),
										placeholder: "Toyota"
									})
								}),
								/* @__PURE__ */ w(s, {
									label: "Model",
									htmlFor: "ve-model",
									children: /* @__PURE__ */ w(c, {
										id: "ve-model",
										value: x,
										onChange: (e) => E(e.target.value),
										placeholder: "Hilux"
									})
								})
							]
						}),
						j && /* @__PURE__ */ w(R, { children: j }),
						/* @__PURE__ */ w("div", {
							className: "flex justify-end",
							children: /* @__PURE__ */ w(a, {
								type: "submit",
								variant: "primary",
								disabled: k,
								children: k ? /* @__PURE__ */ T(C, { children: [/* @__PURE__ */ w(p, { size: 14 }), "Adding…"] }) : "Add vehicle"
							})
						})
					]
				})
			})
		}), /* @__PURE__ */ w("div", {
			className: "stagger space-y-4 lg:col-span-2",
			children: /* @__PURE__ */ w(u, {
				title: "Vehicle Registry",
				subtitle: "Authorized vehicles",
				padded: !1,
				actions: U.length > 0 ? /* @__PURE__ */ w("span", {
					className: "font-mono text-[11px] tabular-nums text-fg-muted",
					children: U.length
				}) : void 0,
				children: e.error && !e.data ? /* @__PURE__ */ w("div", {
					className: "p-4",
					children: /* @__PURE__ */ T(R, { children: ["Failed to load vehicles: ", e.error] })
				}) : U.length === 0 ? /* @__PURE__ */ w("div", {
					className: "p-4",
					children: e.loading ? /* @__PURE__ */ w(z, { label: "vehicles" }) : /* @__PURE__ */ w(o, {
						title: "No vehicles registered",
						hint: "Add an authorized vehicle on the left to populate the registry."
					})
				}) : /* @__PURE__ */ w("div", {
					className: "overflow-x-auto",
					children: /* @__PURE__ */ T("table", {
						className: "w-full border-collapse",
						children: [/* @__PURE__ */ w("thead", { children: /* @__PURE__ */ T("tr", { children: [
							/* @__PURE__ */ w(P, { children: "Plate" }),
							/* @__PURE__ */ w(P, { children: "Owner" }),
							/* @__PURE__ */ w(P, { children: "Type" }),
							/* @__PURE__ */ w(P, { children: "Vehicle" }),
							/* @__PURE__ */ w(P, {
								className: "text-right",
								children: "Action"
							})
						] }) }), /* @__PURE__ */ w("tbody", { children: U.map((e) => /* @__PURE__ */ T("tr", {
							className: "border-t border-line transition-colors duration-150 hover:bg-raised/40",
							children: [
								/* @__PURE__ */ w(F, { children: /* @__PURE__ */ w("span", {
									className: "font-mono text-sm font-semibold tracking-wide text-fg",
									children: e.plate
								}) }),
								/* @__PURE__ */ T(F, { children: [/* @__PURE__ */ w("span", {
									className: "block truncate text-xs text-fg-secondary",
									children: e.owner_name ?? "—"
								}), /* @__PURE__ */ w("span", {
									className: "mt-0.5 inline-block",
									children: /* @__PURE__ */ w(I, {
										label: e.owner_type,
										color: "#71717a"
									})
								})] }),
								/* @__PURE__ */ w(F, { children: /* @__PURE__ */ w("span", {
									className: "font-mono text-xs text-fg-secondary",
									children: e.vehicle_type ?? "—"
								}) }),
								/* @__PURE__ */ w(F, { children: /* @__PURE__ */ T("span", {
									className: "font-mono text-[11px] text-fg-secondary",
									children: [[e.make, e.model].filter(Boolean).join(" ") || "—", e.color ? /* @__PURE__ */ T("span", {
										className: "text-fg-muted",
										children: [" · ", e.color]
									}) : null]
								}) }),
								/* @__PURE__ */ w(F, {
									className: "text-right",
									children: /* @__PURE__ */ w(a, {
										size: "sm",
										variant: "danger",
										disabled: L === e.id,
										onClick: () => void H(e.id, e.plate),
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
function K() {
	let e = S(() => g.listWatchlist(), 15e3), [t, n] = r(""), [l, d] = r("block"), [m, h] = r(""), [_, v] = r("warning"), [y, b] = r(!1), [x, E] = r(null), [D, O] = r(null);
	async function k(r) {
		if (r.preventDefault(), !t.trim()) {
			E("Plate is required.");
			return;
		}
		let a = {
			plate: t.trim(),
			kind: l,
			severity: _
		};
		m.trim() && (a.reason = m.trim()), b(!0), E(null);
		try {
			await g.createWatch(a), n(""), h(""), await e.refresh();
		} catch (e) {
			E(e instanceof i ? e.message : String(e));
		} finally {
			b(!1);
		}
	}
	async function M(t, n) {
		if (window.confirm(`Remove ${n} from the watchlist?`)) {
			O(t);
			try {
				await g.deleteWatch(t), await e.refresh();
			} catch {} finally {
				O(null);
			}
		}
	}
	let N = e.data ?? [];
	return /* @__PURE__ */ T("div", {
		className: "grid grid-cols-1 gap-4 lg:grid-cols-3",
		children: [/* @__PURE__ */ w("div", {
			className: "stagger space-y-4 lg:col-span-1",
			children: /* @__PURE__ */ w(u, {
				title: "Add to Watchlist",
				subtitle: "Flag a plate for the gate",
				children: /* @__PURE__ */ T("form", {
					onSubmit: k,
					className: "space-y-4",
					children: [
						/* @__PURE__ */ w(s, {
							label: /* @__PURE__ */ T(C, { children: ["Plate ", /* @__PURE__ */ w("span", {
								className: "text-accent",
								children: "*"
							})] }),
							htmlFor: "w-plate",
							children: /* @__PURE__ */ w(c, {
								id: "w-plate",
								value: t,
								onChange: (e) => n(e.target.value),
								placeholder: "ABC1234",
								required: !0
							})
						}),
						/* @__PURE__ */ T("div", {
							className: "grid grid-cols-2 gap-3",
							children: [/* @__PURE__ */ w(s, {
								label: "Kind",
								htmlFor: "w-kind",
								children: /* @__PURE__ */ T(f, {
									id: "w-kind",
									value: l,
									onChange: (e) => d(e.target.value),
									children: [
										/* @__PURE__ */ w("option", {
											value: "block",
											children: "Block"
										}),
										/* @__PURE__ */ w("option", {
											value: "vip",
											children: "VIP"
										}),
										/* @__PURE__ */ w("option", {
											value: "alert",
											children: "Alert"
										})
									]
								})
							}), /* @__PURE__ */ w(s, {
								label: "Severity",
								htmlFor: "w-sev",
								children: /* @__PURE__ */ T(f, {
									id: "w-sev",
									value: _,
									onChange: (e) => v(e.target.value),
									children: [
										/* @__PURE__ */ w("option", {
											value: "info",
											children: "Info"
										}),
										/* @__PURE__ */ w("option", {
											value: "warning",
											children: "Warning"
										}),
										/* @__PURE__ */ w("option", {
											value: "critical",
											children: "Critical"
										})
									]
								})
							})]
						}),
						/* @__PURE__ */ w(s, {
							label: "Reason",
							htmlFor: "w-reason",
							children: /* @__PURE__ */ w(c, {
								id: "w-reason",
								value: m,
								onChange: (e) => h(e.target.value),
								placeholder: "Unpaid fines, stolen, …"
							})
						}),
						x && /* @__PURE__ */ w(R, { children: x }),
						/* @__PURE__ */ w("div", {
							className: "flex justify-end",
							children: /* @__PURE__ */ w(a, {
								type: "submit",
								variant: "primary",
								disabled: y,
								children: y ? /* @__PURE__ */ T(C, { children: [/* @__PURE__ */ w(p, { size: 14 }), "Adding…"] }) : "Add to watchlist"
							})
						})
					]
				})
			})
		}), /* @__PURE__ */ w("div", {
			className: "stagger space-y-4 lg:col-span-2",
			children: /* @__PURE__ */ w(u, {
				title: "Watchlist",
				subtitle: "Flagged plates",
				padded: !1,
				actions: N.length > 0 ? /* @__PURE__ */ w("span", {
					className: "font-mono text-[11px] tabular-nums text-fg-muted",
					children: N.length
				}) : void 0,
				children: e.error && !e.data ? /* @__PURE__ */ w("div", {
					className: "p-4",
					children: /* @__PURE__ */ T(R, { children: ["Failed to load watchlist: ", e.error] })
				}) : N.length === 0 ? /* @__PURE__ */ w("div", {
					className: "p-4",
					children: e.loading ? /* @__PURE__ */ w(z, { label: "watchlist" }) : /* @__PURE__ */ w(o, {
						title: "Watchlist empty",
						hint: "Flag a plate on the left to block, VIP, or alert on it at the gate."
					})
				}) : /* @__PURE__ */ w("div", {
					className: "overflow-x-auto",
					children: /* @__PURE__ */ T("table", {
						className: "w-full border-collapse",
						children: [/* @__PURE__ */ w("thead", { children: /* @__PURE__ */ T("tr", { children: [
							/* @__PURE__ */ w(P, { children: "Plate" }),
							/* @__PURE__ */ w(P, { children: "Kind" }),
							/* @__PURE__ */ w(P, { children: "Severity" }),
							/* @__PURE__ */ w(P, { children: "Reason" }),
							/* @__PURE__ */ w(P, {
								className: "text-right",
								children: "Action"
							})
						] }) }), /* @__PURE__ */ w("tbody", { children: N.map((e) => /* @__PURE__ */ T("tr", {
							className: "border-t border-line transition-colors duration-150 hover:bg-raised/40",
							children: [
								/* @__PURE__ */ w(F, { children: /* @__PURE__ */ w("span", {
									className: "font-mono text-sm font-semibold tracking-wide text-fg",
									children: e.plate
								}) }),
								/* @__PURE__ */ w(F, { children: /* @__PURE__ */ w(I, {
									label: e.kind,
									color: A[e.kind] ?? "#71717a"
								}) }),
								/* @__PURE__ */ w(F, { children: /* @__PURE__ */ w(I, {
									label: e.severity,
									color: j[e.severity] ?? "#71717a"
								}) }),
								/* @__PURE__ */ w(F, { children: /* @__PURE__ */ w("span", {
									className: "font-mono text-[11px] text-fg-secondary",
									children: e.reason ?? "—"
								}) }),
								/* @__PURE__ */ w(F, {
									className: "text-right",
									children: /* @__PURE__ */ w(a, {
										size: "sm",
										variant: "danger",
										disabled: D === e.id,
										onClick: () => void M(e.id, e.plate),
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
function q() {
	let e = /* @__PURE__ */ new Date();
	return (/* @__PURE__ */ new Date(e.getTime() - e.getTimezoneOffset() * 6e4)).toISOString().slice(0, 10);
}
var J = [
	"matched",
	"exception",
	"blocked",
	"unmatched"
], Y = {
	matched: "good",
	exception: "warn",
	blocked: "bad",
	unmatched: "default"
};
function X() {
	let [e, t] = r(() => q()), n = S(() => g.reportEntryLog({ date: e }), 0, [e]), i = S(() => g.reportExceptions({ date: e }), 0, [e]), l = n.data?.by_auth_status ?? {}, d = n.data?.events ?? [];
	return /* @__PURE__ */ T("div", {
		className: "stagger space-y-4",
		children: [/* @__PURE__ */ T(u, {
			title: "Daily Report",
			subtitle: "Entry log & exceptions for a single day",
			children: [/* @__PURE__ */ T("div", {
				className: "flex flex-wrap items-end gap-3",
				children: [/* @__PURE__ */ w("div", {
					className: "w-48",
					children: /* @__PURE__ */ w(s, {
						label: "Date",
						htmlFor: "r-date",
						children: /* @__PURE__ */ w(c, {
							id: "r-date",
							type: "date",
							value: e,
							max: q(),
							onChange: (e) => t(e.target.value)
						})
					})
				}), /* @__PURE__ */ w(a, {
					onClick: () => {
						n.refresh(), i.refresh();
					},
					disabled: n.loading,
					children: n.loading ? /* @__PURE__ */ w(p, { size: 14 }) : "Reload"
				})]
			}), n.error ? /* @__PURE__ */ w("div", {
				className: "mt-4",
				children: /* @__PURE__ */ T(R, { children: ["Failed to load report: ", n.error] })
			}) : /* @__PURE__ */ T("div", {
				className: "mt-4 grid grid-cols-2 gap-px overflow-hidden rounded-panel border border-line bg-line sm:grid-cols-3 lg:grid-cols-6",
				children: [
					/* @__PURE__ */ w("div", {
						className: "bg-panel px-4 py-3",
						children: /* @__PURE__ */ w(m, {
							label: "Total",
							value: n.data?.total ?? 0
						})
					}),
					J.map((e) => /* @__PURE__ */ w("div", {
						className: "bg-panel px-4 py-3",
						children: /* @__PURE__ */ w(m, {
							label: e,
							value: l[e] ?? 0,
							tone: Y[e]
						})
					}, e)),
					/* @__PURE__ */ w("div", {
						className: "bg-panel px-4 py-3",
						children: /* @__PURE__ */ w(m, {
							label: "Exceptions",
							value: i.data?.total ?? 0,
							tone: (i.data?.total ?? 0) > 0 ? "warn" : "default"
						})
					})
				]
			})]
		}), /* @__PURE__ */ w(u, {
			title: "Report Events",
			subtitle: n.data ? `${v(n.data.from)} → ${v(n.data.to)}` : "—",
			actions: d.length > 0 ? /* @__PURE__ */ w("span", {
				className: "font-mono text-[11px] tabular-nums text-fg-muted",
				children: d.length
			}) : void 0,
			children: n.loading && !n.data ? /* @__PURE__ */ w(z, { label: "report" }) : d.length === 0 ? /* @__PURE__ */ w(o, {
				title: "No events for this day",
				hint: "Pick another date, or wait for gate activity to be recorded."
			}) : /* @__PURE__ */ w("div", {
				className: "space-y-2.5",
				children: d.map((e) => /* @__PURE__ */ w(H, { ev: e }, e.id))
			})
		})]
	});
}
function Z() {
	let e = S(() => g.listUsers(), 0), t = S(() => g.listApiKeys(), 0), [n, o] = r(""), [l, d] = r(""), [m, h] = r(""), [_, v] = r("guard"), [y, b] = r(!1), [E, D] = r(null), [O, k] = r(null);
	async function A(t) {
		if (t.preventDefault(), !n.trim() || !l) {
			D("Username and password are required.");
			return;
		}
		let r = {
			username: n.trim(),
			password: l,
			role: _
		};
		m.trim() && (r.display_name = m.trim()), b(!0), D(null);
		try {
			await g.createUser(r), o(""), d(""), h(""), await e.refresh();
		} catch (e) {
			D(e instanceof i ? e.message : String(e));
		} finally {
			b(!1);
		}
	}
	async function j(t, n) {
		k(t);
		try {
			await g.updateUser(t, { active: !n }), await e.refresh();
		} catch {} finally {
			k(null);
		}
	}
	let [N, B] = r(""), [V, H] = r("integration"), [U, W] = r(!1), [G, K] = r(null), [q, J] = r(null), [Y, X] = r(!1), [Z, Q] = r(null);
	async function $(e) {
		if (e.preventDefault(), !N.trim()) {
			K("Key name is required.");
			return;
		}
		W(!0), K(null);
		try {
			J(await g.createApiKey(N.trim(), V)), X(!1), B(""), await t.refresh();
		} catch (e) {
			K(e instanceof i ? e.message : String(e));
		} finally {
			W(!1);
		}
	}
	async function ee() {
		if (q) try {
			await navigator.clipboard.writeText(q.key), X(!0);
		} catch {}
	}
	async function te(e, n) {
		if (window.confirm(`Revoke API key “${n}”? Integrations using it will stop working.`)) {
			Q(e);
			try {
				await g.deleteApiKey(e), await t.refresh();
			} catch {} finally {
				Q(null);
			}
		}
	}
	let ne = e.data ?? [], re = t.data ?? [];
	return /* @__PURE__ */ T("div", {
		className: "stagger space-y-4",
		children: [/* @__PURE__ */ T(u, {
			title: "Users",
			subtitle: "Operator accounts & roles",
			children: [
				/* @__PURE__ */ T("form", {
					onSubmit: A,
					className: "grid grid-cols-1 gap-3 sm:grid-cols-2 lg:grid-cols-5 lg:items-end",
					children: [
						/* @__PURE__ */ w(s, {
							label: "Username",
							htmlFor: "u-name",
							children: /* @__PURE__ */ w(c, {
								id: "u-name",
								value: n,
								onChange: (e) => o(e.target.value),
								autoComplete: "off",
								placeholder: "guard01"
							})
						}),
						/* @__PURE__ */ w(s, {
							label: "Password",
							htmlFor: "u-pass",
							children: /* @__PURE__ */ w(c, {
								id: "u-pass",
								type: "password",
								value: l,
								onChange: (e) => d(e.target.value),
								autoComplete: "new-password",
								placeholder: "••••••••"
							})
						}),
						/* @__PURE__ */ w(s, {
							label: "Display name",
							htmlFor: "u-disp",
							children: /* @__PURE__ */ w(c, {
								id: "u-disp",
								value: m,
								onChange: (e) => h(e.target.value),
								placeholder: "Optional"
							})
						}),
						/* @__PURE__ */ w(s, {
							label: "Role",
							htmlFor: "u-role",
							children: /* @__PURE__ */ w(f, {
								id: "u-role",
								value: _,
								onChange: (e) => v(e.target.value),
								children: M.map((e) => /* @__PURE__ */ w("option", {
									value: e,
									children: e[0].toUpperCase() + e.slice(1)
								}, e))
							})
						}),
						/* @__PURE__ */ w(a, {
							type: "submit",
							variant: "primary",
							disabled: y,
							children: y ? /* @__PURE__ */ T(C, { children: [/* @__PURE__ */ w(p, { size: 14 }), "Creating…"] }) : "Create user"
						})
					]
				}),
				E && /* @__PURE__ */ w("div", {
					className: "mt-3",
					children: /* @__PURE__ */ w(R, { children: E })
				}),
				/* @__PURE__ */ w("div", {
					className: "mt-4 overflow-x-auto rounded-md border border-line",
					children: e.error && !e.data ? /* @__PURE__ */ w("div", {
						className: "p-4",
						children: /* @__PURE__ */ T(R, { children: ["Failed to load users: ", e.error] })
					}) : ne.length === 0 ? /* @__PURE__ */ w("div", {
						className: "p-4",
						children: e.loading ? /* @__PURE__ */ w(z, { label: "users" }) : /* @__PURE__ */ w("p", {
							className: "font-mono text-xs text-fg-muted",
							children: "No users."
						})
					}) : /* @__PURE__ */ T("table", {
						className: "w-full border-collapse",
						children: [/* @__PURE__ */ w("thead", { children: /* @__PURE__ */ T("tr", { children: [
							/* @__PURE__ */ w(P, { children: "User" }),
							/* @__PURE__ */ w(P, { children: "Role" }),
							/* @__PURE__ */ w(P, { children: "Status" }),
							/* @__PURE__ */ w(P, { children: "Created" }),
							/* @__PURE__ */ w(P, {
								className: "text-right",
								children: "Action"
							})
						] }) }), /* @__PURE__ */ w("tbody", { children: ne.map((e) => /* @__PURE__ */ T("tr", {
							className: "border-t border-line transition-colors duration-150 hover:bg-raised/40",
							children: [
								/* @__PURE__ */ T(F, { children: [/* @__PURE__ */ w("span", {
									className: "block truncate text-sm font-medium text-fg",
									children: e.display_name || e.username
								}), /* @__PURE__ */ w("span", {
									className: "block truncate font-mono text-[10px] text-fg-muted",
									children: e.username
								})] }),
								/* @__PURE__ */ w(F, { children: /* @__PURE__ */ w(I, {
									label: e.role,
									color: "#f59e0b"
								}) }),
								/* @__PURE__ */ w(F, { children: /* @__PURE__ */ w(I, {
									label: e.active ? "active" : "disabled",
									color: e.active ? "#10b981" : "#71717a"
								}) }),
								/* @__PURE__ */ w(F, { children: /* @__PURE__ */ w("span", {
									className: "whitespace-nowrap font-mono text-[11px] text-fg-secondary",
									children: x(e.created_at)
								}) }),
								/* @__PURE__ */ w(F, {
									className: "text-right",
									children: /* @__PURE__ */ w(a, {
										size: "sm",
										disabled: O === e.id,
										onClick: () => void j(e.id, e.active),
										children: e.active ? "Disable" : "Enable"
									})
								})
							]
						}, e.id)) })]
					})
				})
			]
		}), /* @__PURE__ */ T(u, {
			title: "API Keys",
			subtitle: "Machine credentials for integrations",
			children: [
				/* @__PURE__ */ T("form", {
					onSubmit: $,
					className: "grid grid-cols-1 gap-3 sm:grid-cols-3 lg:items-end",
					children: [
						/* @__PURE__ */ w(s, {
							label: "Name",
							htmlFor: "k-name",
							children: /* @__PURE__ */ w(c, {
								id: "k-name",
								value: N,
								onChange: (e) => B(e.target.value),
								placeholder: "anpr-worker"
							})
						}),
						/* @__PURE__ */ w(s, {
							label: "Role",
							htmlFor: "k-role",
							children: /* @__PURE__ */ w(f, {
								id: "k-role",
								value: V,
								onChange: (e) => H(e.target.value),
								children: M.map((e) => /* @__PURE__ */ w("option", {
									value: e,
									children: e[0].toUpperCase() + e.slice(1)
								}, e))
							})
						}),
						/* @__PURE__ */ w(a, {
							type: "submit",
							variant: "primary",
							disabled: U,
							children: U ? /* @__PURE__ */ T(C, { children: [/* @__PURE__ */ w(p, { size: 14 }), "Creating…"] }) : "Create key"
						})
					]
				}),
				G && /* @__PURE__ */ w("div", {
					className: "mt-3",
					children: /* @__PURE__ */ w(R, { children: G })
				}),
				q && /* @__PURE__ */ T("div", {
					className: "mt-3 rounded-md border border-accent/40 bg-accent/[0.07] p-3",
					children: [
						/* @__PURE__ */ T("div", {
							className: "flex items-center gap-2 font-mono text-[10px] font-semibold uppercase tracking-micro text-accent",
							children: [/* @__PURE__ */ w(L, {}), " New key — copy it now"]
						}),
						/* @__PURE__ */ T("p", {
							className: "mt-1 text-xs leading-relaxed text-fg-secondary",
							children: [
								"The key for ",
								/* @__PURE__ */ w("span", {
									className: "text-fg",
									children: q.name
								}),
								" (",
								q.role,
								") is shown only once and cannot be retrieved again."
							]
						}),
						/* @__PURE__ */ T("div", {
							className: "mt-2 flex items-center gap-2",
							children: [
								/* @__PURE__ */ w("code", {
									className: "min-w-0 flex-1 truncate rounded border border-line bg-canvas px-2 py-1.5 font-mono text-xs text-accent-soft",
									children: q.key
								}),
								/* @__PURE__ */ w(a, {
									size: "sm",
									onClick: () => void ee(),
									children: Y ? "Copied" : "Copy"
								}),
								/* @__PURE__ */ w(a, {
									size: "sm",
									variant: "ghost",
									onClick: () => J(null),
									children: "Dismiss"
								})
							]
						})
					]
				}),
				/* @__PURE__ */ w("div", {
					className: "mt-4 overflow-x-auto rounded-md border border-line",
					children: t.error && !t.data ? /* @__PURE__ */ w("div", {
						className: "p-4",
						children: /* @__PURE__ */ T(R, { children: ["Failed to load API keys: ", t.error] })
					}) : re.length === 0 ? /* @__PURE__ */ w("div", {
						className: "p-4",
						children: t.loading ? /* @__PURE__ */ w(z, { label: "API keys" }) : /* @__PURE__ */ w("p", {
							className: "font-mono text-xs text-fg-muted",
							children: "No API keys."
						})
					}) : /* @__PURE__ */ T("table", {
						className: "w-full border-collapse",
						children: [/* @__PURE__ */ w("thead", { children: /* @__PURE__ */ T("tr", { children: [
							/* @__PURE__ */ w(P, { children: "Name" }),
							/* @__PURE__ */ w(P, { children: "Prefix" }),
							/* @__PURE__ */ w(P, { children: "Role" }),
							/* @__PURE__ */ w(P, { children: "Last used" }),
							/* @__PURE__ */ w(P, {
								className: "text-right",
								children: "Action"
							})
						] }) }), /* @__PURE__ */ w("tbody", { children: re.map((e) => /* @__PURE__ */ T("tr", {
							className: "border-t border-line transition-colors duration-150 hover:bg-raised/40",
							children: [
								/* @__PURE__ */ w(F, { children: /* @__PURE__ */ w("span", {
									className: "text-sm font-medium text-fg",
									children: e.name
								}) }),
								/* @__PURE__ */ w(F, { children: /* @__PURE__ */ T("span", {
									className: "font-mono text-xs text-fg-secondary",
									children: [e.key_prefix, "…"]
								}) }),
								/* @__PURE__ */ w(F, { children: /* @__PURE__ */ w(I, {
									label: e.role,
									color: "#f59e0b"
								}) }),
								/* @__PURE__ */ w(F, { children: /* @__PURE__ */ w("span", {
									className: "whitespace-nowrap font-mono text-[11px] text-fg-secondary",
									children: e.last_used_at ? x(e.last_used_at) : "never"
								}) }),
								/* @__PURE__ */ w(F, {
									className: "text-right",
									children: /* @__PURE__ */ w(a, {
										size: "sm",
										variant: "danger",
										disabled: Z === e.id,
										onClick: () => void te(e.id, e.name),
										children: "Revoke"
									})
								})
							]
						}, e.id)) })]
					})
				})
			]
		})]
	});
}
function Q({ active: e, onClick: t, children: n }) {
	return /* @__PURE__ */ w("button", {
		type: "button",
		onClick: t,
		className: _("relative -mb-px whitespace-nowrap border-b-2 px-3.5 py-2.5 font-mono text-[11px] font-semibold uppercase tracking-micro transition-colors duration-150 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-accent focus-visible:ring-offset-2 focus-visible:ring-offset-canvas", e ? "border-accent text-fg" : "border-transparent text-fg-muted hover:text-fg-secondary"),
		children: n
	});
}
function $() {
	let [o, s] = r(null), [c, f] = r(!0), [m, h] = r(!1), [_, v] = r(null), [y, x] = r("live"), S = e(async () => {
		f(!0), v(null);
		try {
			s(await g.me()), h(!1);
		} catch (e) {
			e instanceof i && e.status === 401 ? (s(null), h(!0)) : v(e instanceof Error ? e.message : String(e));
		} finally {
			f(!1);
		}
	}, []);
	t(() => {
		S();
	}, [S]);
	let C = o?.role === "admin", E = o?.role === "admin" || o?.role === "manager" || o?.role === "guard", D = n(() => {
		let e = [
			{
				key: "live",
				label: "Live Entry"
			},
			{
				key: "passes",
				label: "Visitor Passes"
			},
			{
				key: "vehicles",
				label: "Vehicles"
			},
			{
				key: "watchlist",
				label: "Watchlist"
			},
			{
				key: "reports",
				label: "Reports"
			}
		];
		return C && e.push({
			key: "admin",
			label: "Admin"
		}), e;
	}, [C]);
	async function O() {
		try {
			await g.logout();
		} catch {}
		b(null), s(null), h(!0);
	}
	if (m) return /* @__PURE__ */ w(l, { onSuccess: (e) => {
		s(e), h(!1), v(null);
	} });
	if (c && !o) return /* @__PURE__ */ T("div", {
		className: "flex min-h-[60vh] items-center justify-center gap-3 text-fg-secondary",
		children: [/* @__PURE__ */ w(p, {}), /* @__PURE__ */ w("span", {
			className: "font-mono text-xs uppercase tracking-micro",
			children: "Authenticating…"
		})]
	});
	if (_ && !o) return /* @__PURE__ */ w("div", {
		className: "mx-auto max-w-md px-4 py-20",
		children: /* @__PURE__ */ T(u, {
			title: "Console unavailable",
			children: [/* @__PURE__ */ w(R, { children: _ }), /* @__PURE__ */ w("div", {
				className: "mt-3 flex justify-end",
				children: /* @__PURE__ */ w(a, {
					variant: "primary",
					onClick: () => void S(),
					children: "Retry"
				})
			})]
		})
	});
	if (!o) return null;
	let k = y === "admin" && !C ? "live" : y;
	return /* @__PURE__ */ T("div", {
		className: "mx-auto max-w-[1600px] px-4 py-6 sm:px-6",
		children: [/* @__PURE__ */ T("header", {
			className: "animate-rise",
			children: [/* @__PURE__ */ T("div", {
				className: "flex flex-wrap items-end justify-between gap-4",
				children: [/* @__PURE__ */ T("div", {
					className: "min-w-0",
					children: [/* @__PURE__ */ w(d, { children: "Operations · Entry" }), /* @__PURE__ */ w("h1", {
						className: "mt-1 font-display text-2xl font-extrabold tracking-tight text-fg",
						children: "Access Control"
					})]
				}), /* @__PURE__ */ T("div", {
					className: "flex items-center gap-3",
					children: [/* @__PURE__ */ T("div", {
						className: "flex flex-col items-end leading-none",
						children: [/* @__PURE__ */ w("span", {
							className: "font-mono text-[12px] font-semibold text-fg",
							children: o.name
						}), /* @__PURE__ */ T("span", {
							className: "mt-1 font-mono text-[9px] uppercase tracking-micro text-accent",
							children: [o.role, o.kind === "system" && /* @__PURE__ */ w("span", {
								className: "text-fg-muted",
								children: " · auth off"
							})]
						})]
					}), o.kind === "user" && /* @__PURE__ */ w(a, {
						size: "sm",
						onClick: () => void O(),
						children: "Sign out"
					})]
				})]
			}), /* @__PURE__ */ w("div", {
				className: "mt-5 flex flex-wrap gap-1 overflow-x-auto border-b border-line",
				children: D.map((e) => /* @__PURE__ */ w(Q, {
					active: k === e.key,
					onClick: () => x(e.key),
					children: e.label
				}, e.key))
			})]
		}), /* @__PURE__ */ T("div", {
			className: "mt-5",
			children: [
				k === "live" && /* @__PURE__ */ w(U, { canOperate: E }),
				k === "passes" && /* @__PURE__ */ w(W, {}),
				k === "vehicles" && /* @__PURE__ */ w(G, {}),
				k === "watchlist" && /* @__PURE__ */ w(K, {}),
				k === "reports" && /* @__PURE__ */ w(X, {}),
				k === "admin" && C && /* @__PURE__ */ w(Z, {})
			]
		})]
	});
}
//#endregion
//#region src/modules/entry/entry.tsx
var ee = $;
//#endregion
export { ee as default };
