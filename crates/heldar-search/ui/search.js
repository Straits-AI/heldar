import { useCallback as e, useEffect as t, useMemo as n, useState as r } from "react";
import { ApiError as i, Button as a, EmptyState as o, Field as s, Input as c, Login as l, Panel as u, SectionLabel as d, Select as f, Spinner as p, Stat as m, StatusPill as h, api as g, cx as _, formatClock as v, localInputToIso as y, setAuthToken as b, usePoll as x } from "@heldar/shell";
import { Fragment as S, jsx as C, jsxs as w } from "react/jsx-runtime";
//#region src/modules/search/page.tsx
var T = {
	matched: "recording",
	exception: "connecting",
	blocked: "error",
	unmatched: "offline"
}, E = {
	matched: "#10b981",
	exception: "#fbbf24",
	blocked: "#ef4444",
	unmatched: "#52525b"
}, D = {
	entry: "#38bdf8",
	zone: "#a78bfa",
	breach: "#ef4444"
}, O = {
	llm: "#f59e0b",
	rules: "#38bdf8",
	structured: "#a78bfa"
}, k = {
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
}, A = {
	inference: 0,
	aggregate: 1,
	event: 2,
	track: 3,
	observation: 4
}, j = [
	"white cars entering after 6pm last week",
	"unauthorized vehicles today",
	"red zone breaches yesterday"
];
function M({ label: e, color: t }) {
	return /* @__PURE__ */ C("span", {
		className: "inline-flex shrink-0 items-center rounded border px-1.5 py-0.5 font-mono text-[9px] font-semibold uppercase tracking-micro leading-none",
		style: {
			color: t,
			borderColor: `${t}55`,
			backgroundColor: `${t}1a`
		},
		children: e
	});
}
function N({ className: e }) {
	return /* @__PURE__ */ w("svg", {
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
			/* @__PURE__ */ C("path", { d: "M8 1.5l6.5 11.5H1.5z" }),
			/* @__PURE__ */ C("path", { d: "M8 6.5v3.5" }),
			/* @__PURE__ */ C("path", { d: "M8 11.6v.4" })
		]
	});
}
function P({ children: e }) {
	return /* @__PURE__ */ w("div", {
		role: "alert",
		className: "flex items-start gap-2 rounded-md border border-danger/40 bg-danger/10 px-3 py-2 font-mono text-xs text-red-300",
		children: [/* @__PURE__ */ C(N, { className: "mt-0.5 shrink-0" }), /* @__PURE__ */ C("span", {
			className: "break-words",
			children: e
		})]
	});
}
function F(e, t) {
	if (!e) return null;
	let n = e[t];
	return typeof n == "string" ? n.trim() ? n : null : typeof n == "number" || typeof n == "boolean" ? String(n) : null;
}
function I({ path: e, alt: t }) {
	let [n, i] = r(!1);
	return n ? null : /* @__PURE__ */ C("img", {
		src: e,
		alt: t,
		loading: "lazy",
		onError: () => i(!0),
		className: "h-16 w-24 shrink-0 rounded-md border border-line bg-black object-cover"
	});
}
function L({ children: e }) {
	return /* @__PURE__ */ w("div", {
		className: "flex items-start gap-3 rounded-panel border border-line bg-panel px-4 py-3",
		children: [/* @__PURE__ */ w("svg", {
			viewBox: "0 0 20 20",
			className: "mt-0.5 h-4 w-4 shrink-0 text-accent",
			fill: "none",
			stroke: "currentColor",
			strokeWidth: "1.6",
			strokeLinecap: "round",
			strokeLinejoin: "round",
			"aria-hidden": "true",
			children: [
				/* @__PURE__ */ C("circle", {
					cx: "10",
					cy: "10",
					r: "7.5"
				}),
				/* @__PURE__ */ C("path", { d: "M10 9v4" }),
				/* @__PURE__ */ C("path", { d: "M10 6.6v.4" })
			]
		}), /* @__PURE__ */ C("p", {
			className: "font-mono text-[11px] leading-relaxed text-fg-secondary",
			children: e
		})]
	});
}
function R(e) {
	return `${String(e).padStart(2, "0")}:00`;
}
function z({ label: e, value: t }) {
	return /* @__PURE__ */ w("span", {
		className: "inline-flex items-center gap-1.5 rounded-md border border-line bg-canvas px-2 py-1 leading-none",
		children: [/* @__PURE__ */ C("span", {
			className: "font-mono text-[9px] uppercase tracking-micro text-fg-muted",
			children: e
		}), /* @__PURE__ */ C("span", {
			className: "font-mono text-[11px] font-semibold text-fg",
			children: t
		})]
	});
}
function B(e, t) {
	let n = [];
	return e.from && n.push({
		label: "From",
		value: v(e.from)
	}), e.to && n.push({
		label: "To",
		value: v(e.to)
	}), e.hour_min != null && n.push({
		label: "After",
		value: `${R(e.hour_min)} UTC`
	}), e.hour_max != null && n.push({
		label: "Before",
		value: `${R(e.hour_max)} UTC`
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
function V({ plan: e, planner: t, nameFor: r, dryRun: i }) {
	let a = n(() => B(e, r), [e, r]), o = O[t] ?? "#71717a";
	return /* @__PURE__ */ w(u, {
		title: "Interpreted as",
		subtitle: "The structured plan your question was turned into — the only inference in the answer",
		actions: /* @__PURE__ */ w("div", {
			className: "flex items-center gap-2",
			children: [i && /* @__PURE__ */ C(M, {
				label: "Dry run",
				color: "#fbbf24"
			}), /* @__PURE__ */ C(M, {
				label: `Planner · ${t}`,
				color: o
			})]
		}),
		children: [a.length === 0 ? /* @__PURE__ */ w("p", {
			className: "font-mono text-[11px] leading-relaxed text-fg-secondary",
			children: [
				"No filters were extracted — this defaults to",
				" ",
				/* @__PURE__ */ C("span", {
					className: "text-fg",
					children: "all sources"
				}),
				" over the last ~7 days. Add detail (color, time, camera, authorization) to narrow it."
			]
		}) : /* @__PURE__ */ C("div", {
			className: "flex flex-wrap gap-2",
			children: a.map((e) => /* @__PURE__ */ C(z, {
				label: e.label,
				value: e.value
			}, e.label))
		}), /* @__PURE__ */ w("p", {
			className: "mt-3 flex items-start gap-1.5 border-t border-line pt-3 font-mono text-[10px] leading-relaxed text-fg-muted",
			children: [/* @__PURE__ */ C(N, { className: "mt-0.5 shrink-0 text-fg-muted/80" }), /* @__PURE__ */ w("span", { children: [
				"Verify this reflects your intent — the planner only decides",
				" ",
				/* @__PURE__ */ C("span", {
					className: "text-fg-secondary",
					children: "how to query"
				}),
				". The results are exactly what this plan selected, nothing more."
			] })]
		})]
	});
}
function H(e, t) {
	let n = e[t];
	return typeof n == "string" && n.trim() ? n : null;
}
function U({ proof: e }) {
	let t = n(() => {
		let t = [...e.claim_levels ?? []];
		return t.sort((e, t) => (A[String(e.level ?? "")] ?? 99) - (A[String(t.level ?? "")] ?? 99)), t;
	}, [e.claim_levels]);
	return /* @__PURE__ */ w(u, {
		title: "Proof",
		subtitle: "Why this answer can be trusted — facts at the bottom, interpretation at the top",
		children: [
			/* @__PURE__ */ w("p", {
				className: "mb-4 rounded-md border border-accent/30 bg-accent/[0.06] px-3 py-2 text-xs leading-relaxed text-fg-secondary",
				children: [
					/* @__PURE__ */ C("span", {
						className: "font-semibold text-fg",
						children: "The answers are facts; the interpretation is the only inference."
					}),
					" ",
					"Each rung below states a claim, its confidence, and the caveat that bounds it."
				]
			}),
			/* @__PURE__ */ w("ol", {
				className: "relative space-y-3 pl-5",
				children: [/* @__PURE__ */ C("span", {
					className: "absolute left-[5px] top-2 bottom-2 w-px bg-line",
					"aria-hidden": "true"
				}), t.map((e, t) => {
					let n = H(e, "level") ?? "—", r = k[n] ?? {
						color: "#71717a",
						blurb: ""
					}, i = H(e, "statement"), a = H(e, "confidence"), o = H(e, "caveat"), s = H(e, "basis"), c = H(e, "provenance");
					return /* @__PURE__ */ w("li", {
						className: "relative",
						children: [/* @__PURE__ */ C("span", {
							className: "absolute -left-5 top-1.5 h-2.5 w-2.5 rounded-full border-2 border-canvas",
							style: { backgroundColor: r.color },
							"aria-hidden": "true"
						}), /* @__PURE__ */ w("div", {
							className: "rounded-md border border-line bg-panel2/40 p-3",
							style: {
								borderLeftColor: r.color,
								borderLeftWidth: 3
							},
							children: [
								/* @__PURE__ */ w("div", {
									className: "flex flex-wrap items-center gap-2",
									children: [
										/* @__PURE__ */ C(M, {
											label: n,
											color: r.color
										}),
										r.blurb && /* @__PURE__ */ C("span", {
											className: "font-mono text-[10px] text-fg-muted",
											children: r.blurb
										}),
										a && /* @__PURE__ */ w("span", {
											className: "ml-auto whitespace-nowrap font-mono text-[10px] text-fg-secondary",
											children: ["confidence:\xA0", /* @__PURE__ */ C("span", {
												className: "text-fg",
												children: a
											})]
										})
									]
								}),
								i && /* @__PURE__ */ C("p", {
									className: "mt-2 text-xs leading-relaxed text-fg-secondary",
									children: i
								}),
								s && /* @__PURE__ */ w("p", {
									className: "mt-1.5 font-mono text-[10px] leading-relaxed text-fg-muted",
									children: ["basis: ", s]
								}),
								c && /* @__PURE__ */ w("p", {
									className: "mt-1.5 font-mono text-[10px] leading-relaxed text-fg-muted",
									children: ["provenance: ", c]
								}),
								o && /* @__PURE__ */ w("p", {
									className: "mt-2 flex items-start gap-1.5 rounded border border-connecting/30 bg-connecting/[0.06] px-2 py-1.5 font-mono text-[10px] leading-relaxed text-connecting",
									children: [/* @__PURE__ */ C(N, { className: "mt-0.5 shrink-0" }), /* @__PURE__ */ C("span", { children: o })]
								})
							]
						})]
					}, `${n}-${t}`);
				})]
			}),
			e.note && /* @__PURE__ */ C("p", {
				className: "mt-4 border-t border-line pt-3 font-mono text-[10px] leading-relaxed text-fg-muted",
				children: e.note
			})
		]
	});
}
function W({ hit: e, nameFor: t }) {
	let n = D[e.source] ?? "#71717a", r = (e.auth_status ? E[e.auth_status] : void 0) ?? n, i = F(e.subject, "color"), a = F(e.subject, "vehicle_type"), o = F(e.subject, "label"), s = F(e.subject, "subject_type") ?? F(e.subject, "type"), c = F(e.subject, "severity");
	return /* @__PURE__ */ w("div", {
		className: "flex gap-3 rounded-md border border-line bg-panel2/40 p-3 transition-colors duration-150 hover:border-[#34373e]",
		style: {
			borderLeftColor: r,
			borderLeftWidth: 3
		},
		children: [e.evidence_path && /* @__PURE__ */ C(I, {
			path: e.evidence_path,
			alt: `${e.source} ${e.plate ?? e.id}`
		}), /* @__PURE__ */ w("div", {
			className: "min-w-0 flex-1",
			children: [
				/* @__PURE__ */ w("div", {
					className: "flex flex-wrap items-center gap-2",
					children: [
						/* @__PURE__ */ C(M, {
							label: e.source,
							color: n
						}),
						/* @__PURE__ */ C("span", {
							className: "font-mono text-[10px] uppercase tracking-micro text-fg-muted",
							children: e.kind
						}),
						e.claim_level && /* @__PURE__ */ C(M, {
							label: e.claim_level,
							color: "#52525b"
						}),
						/* @__PURE__ */ C("span", {
							className: "ml-auto whitespace-nowrap font-mono text-[10px] text-fg-muted",
							children: v(e.timestamp)
						})
					]
				}),
				/* @__PURE__ */ w("div", {
					className: "mt-2 flex flex-wrap items-center gap-2",
					children: [e.plate ? /* @__PURE__ */ C("span", {
						className: "font-mono text-base font-semibold tracking-wide text-fg",
						children: e.plate
					}) : /* @__PURE__ */ C("span", {
						className: "font-mono text-sm text-fg-secondary",
						children: o ?? s ?? "—"
					}), e.auth_status && /* @__PURE__ */ C(h, {
						state: T[e.auth_status] ?? "unknown",
						label: e.auth_status
					})]
				}),
				/* @__PURE__ */ w("div", {
					className: "mt-1.5 flex flex-wrap gap-x-3 gap-y-0.5 font-mono text-[10px] text-fg-secondary",
					children: [
						/* @__PURE__ */ w("span", {
							className: "text-fg-muted",
							children: ["camera:\xA0", /* @__PURE__ */ C("span", {
								className: "text-fg-secondary",
								children: t(e.camera_id)
							})]
						}),
						e.zone && /* @__PURE__ */ w("span", {
							className: "text-fg-muted",
							children: [
								"zone:\xA0",
								/* @__PURE__ */ C("span", {
									className: "text-fg-secondary",
									children: e.zone
								}),
								e.zone_kind ? /* @__PURE__ */ w("span", {
									className: "text-fg-muted",
									children: [
										" (",
										e.zone_kind,
										")"
									]
								}) : null
							]
						}),
						s && e.plate && /* @__PURE__ */ w("span", {
							className: "text-fg-muted",
							children: ["subject:\xA0", /* @__PURE__ */ C("span", {
								className: "text-fg-secondary",
								children: s
							})]
						}),
						a && /* @__PURE__ */ w("span", {
							className: "text-fg-muted",
							children: ["type:\xA0", /* @__PURE__ */ C("span", {
								className: "text-fg-secondary",
								children: a
							})]
						}),
						i && /* @__PURE__ */ w("span", {
							className: "text-fg-muted",
							children: ["color:\xA0", /* @__PURE__ */ C("span", {
								className: "text-fg-secondary",
								children: i
							})]
						}),
						c && /* @__PURE__ */ w("span", {
							className: "text-fg-muted",
							children: ["severity:\xA0", /* @__PURE__ */ C("span", {
								className: "text-fg-secondary",
								children: c
							})]
						})
					]
				})
			]
		})]
	});
}
function G({ result: e, nameFor: t }) {
	let r = n(() => {
		let t = 0, n = 0, r = 0;
		for (let i of e.hits) i.source === "entry" ? t += 1 : i.source === "zone" ? n += 1 : i.source === "breach" && (r += 1);
		return {
			entry: t,
			zone: n,
			breach: r
		};
	}, [e.hits]);
	return /* @__PURE__ */ w(S, { children: [/* @__PURE__ */ w("div", {
		className: "grid grid-cols-2 gap-px overflow-hidden rounded-panel border border-line bg-line sm:grid-cols-4",
		children: [
			/* @__PURE__ */ C("div", {
				className: "bg-panel px-4 py-3",
				children: /* @__PURE__ */ C(m, {
					label: "Matches",
					value: e.count
				})
			}),
			/* @__PURE__ */ C("div", {
				className: "bg-panel px-4 py-3",
				children: /* @__PURE__ */ C(m, {
					label: "Entry",
					value: r.entry
				})
			}),
			/* @__PURE__ */ C("div", {
				className: "bg-panel px-4 py-3",
				children: /* @__PURE__ */ C(m, {
					label: "Zone",
					value: r.zone
				})
			}),
			/* @__PURE__ */ C("div", {
				className: "bg-panel px-4 py-3",
				children: /* @__PURE__ */ C(m, {
					label: "Breach",
					value: r.breach,
					tone: r.breach > 0 ? "bad" : "default"
				})
			})
		]
	}), /* @__PURE__ */ C(u, {
		title: "Results",
		subtitle: "Stored events matching the executed plan — newest first",
		actions: /* @__PURE__ */ C("span", {
			className: "font-mono text-[11px] tabular-nums text-fg-muted",
			children: e.count
		}),
		children: e.hits.length === 0 ? /* @__PURE__ */ C(o, {
			title: "No matching events",
			hint: "The plan ran cleanly but no stored events matched. Loosen the filters above, widen the time window, or check the interpreted plan."
		}) : /* @__PURE__ */ C("div", {
			className: "space-y-2.5",
			children: e.hits.map((e) => /* @__PURE__ */ C(W, {
				hit: e,
				nameFor: t
			}, `${e.source}-${e.id}`))
		})
	})] });
}
function K({ busy: e, onRun: t }) {
	let [n, i] = r(""), [o, l] = r(""), [u, d] = r(""), [m, h] = r(""), [g, _] = r("");
	function v(e) {
		e.preventDefault();
		let r = {};
		n && (r.sources = [n]), o && (r.auth_status = [o]), u.trim() && (r.color = u.trim());
		let i = y(m);
		i && (r.from = i);
		let a = y(g);
		a && (r.to = a), t(r);
	}
	return /* @__PURE__ */ w("form", {
		onSubmit: v,
		className: "space-y-4",
		children: [/* @__PURE__ */ w("div", {
			className: "grid grid-cols-1 gap-3 sm:grid-cols-2 lg:grid-cols-3",
			children: [
				/* @__PURE__ */ C(s, {
					label: "Source",
					htmlFor: "sf-source",
					children: /* @__PURE__ */ w(f, {
						id: "sf-source",
						value: n,
						onChange: (e) => i(e.target.value),
						children: [
							/* @__PURE__ */ C("option", {
								value: "",
								children: "Any source"
							}),
							/* @__PURE__ */ C("option", {
								value: "entry",
								children: "Entry"
							}),
							/* @__PURE__ */ C("option", {
								value: "zone",
								children: "Zone"
							}),
							/* @__PURE__ */ C("option", {
								value: "breach",
								children: "Breach"
							})
						]
					})
				}),
				/* @__PURE__ */ C(s, {
					label: "Authorization",
					htmlFor: "sf-auth",
					children: /* @__PURE__ */ w(f, {
						id: "sf-auth",
						value: o,
						onChange: (e) => l(e.target.value),
						children: [
							/* @__PURE__ */ C("option", {
								value: "",
								children: "Any status"
							}),
							/* @__PURE__ */ C("option", {
								value: "matched",
								children: "Matched"
							}),
							/* @__PURE__ */ C("option", {
								value: "exception",
								children: "Exception"
							}),
							/* @__PURE__ */ C("option", {
								value: "unmatched",
								children: "Unmatched"
							}),
							/* @__PURE__ */ C("option", {
								value: "blocked",
								children: "Blocked"
							})
						]
					})
				}),
				/* @__PURE__ */ C(s, {
					label: "Color",
					htmlFor: "sf-color",
					children: /* @__PURE__ */ C(c, {
						id: "sf-color",
						value: u,
						onChange: (e) => d(e.target.value),
						placeholder: "white",
						autoComplete: "off"
					})
				}),
				/* @__PURE__ */ C(s, {
					label: "From",
					htmlFor: "sf-from",
					children: /* @__PURE__ */ C(c, {
						id: "sf-from",
						type: "datetime-local",
						step: 1,
						value: m,
						onChange: (e) => h(e.target.value)
					})
				}),
				/* @__PURE__ */ C(s, {
					label: "To",
					htmlFor: "sf-to",
					children: /* @__PURE__ */ C(c, {
						id: "sf-to",
						type: "datetime-local",
						step: 1,
						value: g,
						onChange: (e) => _(e.target.value)
					})
				})
			]
		}), /* @__PURE__ */ C("div", {
			className: "flex justify-end",
			children: /* @__PURE__ */ C(a, {
				type: "submit",
				variant: "primary",
				disabled: e,
				children: e ? /* @__PURE__ */ w(S, { children: [/* @__PURE__ */ C(p, { size: 14 }), "Running…"] }) : "Run structured query"
			})
		})]
	});
}
function q({ nameFor: t }) {
	let [n, l] = r(""), [d, f] = r(null), [m, h] = r(null), [v, y] = r(null), [b, x] = r(null), [T, E] = r(!1), [D, O] = r(!1), k = e(async (e) => {
		let t = e.trim();
		if (t) {
			l(t), y("nl"), x(null), h(null);
			try {
				f(await g.searchNl(t)), E(!0);
			} catch (e) {
				x(e instanceof i ? e.message : String(e)), f(null), E(!0);
			} finally {
				y(null);
			}
		}
	}, []);
	async function A() {
		let e = n.trim();
		if (e) {
			y("plan"), x(null), f(null);
			try {
				h(await g.searchPlan(e)), E(!0);
			} catch (e) {
				x(e instanceof i ? e.message : String(e)), h(null), E(!0);
			} finally {
				y(null);
			}
		}
	}
	let M = e(async (e) => {
		y("structured"), x(null), h(null);
		try {
			f(await g.searchEvents(e)), E(!0);
		} catch (e) {
			x(e instanceof i ? e.message : String(e)), f(null), E(!0);
		} finally {
			y(null);
		}
	}, []);
	function N(e) {
		e.preventDefault(), k(n);
	}
	let F = m?.plan ?? d?.plan ?? null, I = m?.planner ?? d?.planner ?? "rules";
	return /* @__PURE__ */ w("div", {
		className: "stagger space-y-4",
		children: [
			/* @__PURE__ */ w(L, { children: [
				/* @__PURE__ */ C("span", {
					className: "text-fg",
					children: "Ask in plain language; the answer is the data."
				}),
				" A planner (transparent rules, or an optional LLM) translates your question into a structured query — that interpretation is the only inference. The plan then runs deterministically over the kernel's stored events, so",
				" ",
				/* @__PURE__ */ C("span", {
					className: "text-fg",
					children: "the answers are facts, the interpretation is the only inference"
				}),
				". Every search is logged; plate-targeted queries are audited."
			] }),
			/* @__PURE__ */ w(u, {
				title: "Ask",
				subtitle: "Natural-language search over entry, zone & breach events",
				children: [
					/* @__PURE__ */ w("form", {
						onSubmit: N,
						className: "flex flex-col gap-3 sm:flex-row sm:items-end",
						children: [/* @__PURE__ */ C("div", {
							className: "min-w-0 flex-1",
							children: /* @__PURE__ */ C(s, {
								label: "Query",
								htmlFor: "nl-query",
								children: /* @__PURE__ */ C(c, {
									id: "nl-query",
									value: n,
									onChange: (e) => l(e.target.value),
									placeholder: "white cars entering after 6pm last week",
									autoComplete: "off"
								})
							})
						}), /* @__PURE__ */ w("div", {
							className: "flex shrink-0 items-center gap-2",
							children: [/* @__PURE__ */ C(a, {
								type: "submit",
								variant: "primary",
								disabled: v !== null || !n.trim(),
								children: v === "nl" ? /* @__PURE__ */ w(S, { children: [/* @__PURE__ */ C(p, { size: 14 }), "Searching…"] }) : "Search"
							}), /* @__PURE__ */ C(a, {
								type: "button",
								onClick: () => void A(),
								disabled: v !== null || !n.trim(),
								children: v === "plan" ? /* @__PURE__ */ w(S, { children: [/* @__PURE__ */ C(p, { size: 14 }), "Planning…"] }) : "Plan only (dry-run)"
							})]
						})]
					}),
					/* @__PURE__ */ w("div", {
						className: "mt-3 flex flex-wrap items-center gap-2",
						children: [/* @__PURE__ */ C("span", {
							className: "font-mono text-[9px] uppercase tracking-micro text-fg-muted",
							children: "Try"
						}), j.map((e) => /* @__PURE__ */ C("button", {
							type: "button",
							disabled: v !== null,
							onClick: () => void k(e),
							className: "rounded-full border border-line bg-canvas px-2.5 py-1 font-mono text-[10px] text-fg-secondary transition-colors duration-150 hover:border-accent/50 hover:text-fg disabled:cursor-not-allowed disabled:opacity-50",
							children: e
						}, e))]
					}),
					/* @__PURE__ */ w("p", {
						className: "mt-3 flex items-center gap-1.5 font-mono text-[10px] uppercase tracking-micro text-fg-muted",
						children: [/* @__PURE__ */ w("svg", {
							viewBox: "0 0 16 16",
							width: "12",
							height: "12",
							fill: "none",
							stroke: "currentColor",
							strokeWidth: "1.5",
							"aria-hidden": "true",
							children: [/* @__PURE__ */ C("rect", {
								x: "3",
								y: "7",
								width: "10",
								height: "7",
								rx: "1.5"
							}), /* @__PURE__ */ C("path", {
								d: "M5.5 7V5a2.5 2.5 0 0 1 5 0v2",
								strokeLinecap: "round"
							})]
						}), "Searches are logged · plate-targeted queries are audited."]
					}),
					b && /* @__PURE__ */ C("div", {
						className: "mt-3",
						children: /* @__PURE__ */ C(P, { children: b })
					}),
					/* @__PURE__ */ w("div", {
						className: "mt-4 border-t border-line pt-3",
						children: [/* @__PURE__ */ w("button", {
							type: "button",
							onClick: () => O((e) => !e),
							className: "flex items-center gap-1.5 font-mono text-[10px] font-semibold uppercase tracking-micro text-fg-secondary transition-colors duration-150 hover:text-fg",
							children: [/* @__PURE__ */ C("svg", {
								viewBox: "0 0 16 16",
								width: "12",
								height: "12",
								fill: "none",
								stroke: "currentColor",
								strokeWidth: "1.6",
								strokeLinecap: "round",
								strokeLinejoin: "round",
								"aria-hidden": "true",
								className: _("transition-transform duration-150", D && "rotate-90"),
								children: /* @__PURE__ */ C("path", { d: "M6 4l4 4-4 4" })
							}), "Structured filters"]
						}), D && /* @__PURE__ */ C("div", {
							className: "mt-3",
							children: /* @__PURE__ */ C(K, {
								busy: v === "structured",
								onRun: (e) => void M(e)
							})
						})]
					})
				]
			}),
			F && /* @__PURE__ */ C(V, {
				plan: F,
				planner: I,
				nameFor: t,
				dryRun: m != null
			}),
			m && /* @__PURE__ */ w(u, {
				title: "Dry run — not executed",
				subtitle: "The plan above was generated but no query was run",
				children: [/* @__PURE__ */ C("p", {
					className: "text-xs leading-relaxed text-fg-secondary",
					children: "Nothing was read from the fact tables. Review the interpreted plan, then execute it exactly as shown."
				}), /* @__PURE__ */ w("div", {
					className: "mt-3 flex items-center gap-2",
					children: [/* @__PURE__ */ C(a, {
						variant: "primary",
						disabled: v !== null,
						onClick: () => void M(m.plan),
						children: v === "structured" ? /* @__PURE__ */ w(S, { children: [/* @__PURE__ */ C(p, { size: 14 }), "Running…"] }) : "Run this plan"
					}), /* @__PURE__ */ C(a, {
						disabled: v !== null,
						onClick: () => void k(n),
						children: "Re-run as search"
					})]
				})]
			}),
			d && /* @__PURE__ */ w(S, { children: [/* @__PURE__ */ C(G, {
				result: d,
				nameFor: t
			}), /* @__PURE__ */ C(U, { proof: d.proof })] }),
			!T && !F && /* @__PURE__ */ C(o, {
				title: "Ask a question to begin",
				hint: "Search in plain language across entry, zone and breach events. The interpreted plan and a proof ladder are shown with every result, so you can always see how the question was read and why the answer holds."
			})
		]
	});
}
function J() {
	let [o, s] = r(null), [c, f] = r(!0), [m, h] = r(!1), [_, v] = r(null), y = e(async () => {
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
		y();
	}, [y]);
	let S = x(() => g.listCameras(), 0), T = S.data ?? [], E = n(() => {
		let e = /* @__PURE__ */ new Map();
		for (let t of T) e.set(t.id, t.name);
		return e;
	}, [S.data]), D = e((e) => e ? E.get(e) ?? e : "—", [E]);
	async function O() {
		try {
			await g.logout();
		} catch {}
		b(null), s(null), h(!0);
	}
	return m ? /* @__PURE__ */ C(l, { onSuccess: (e) => {
		s(e), h(!1), v(null);
	} }) : c && !o ? /* @__PURE__ */ w("div", {
		className: "flex min-h-[60vh] items-center justify-center gap-3 text-fg-secondary",
		children: [/* @__PURE__ */ C(p, {}), /* @__PURE__ */ C("span", {
			className: "font-mono text-xs uppercase tracking-micro",
			children: "Authenticating…"
		})]
	}) : _ && !o ? /* @__PURE__ */ C("div", {
		className: "mx-auto max-w-md px-4 py-20",
		children: /* @__PURE__ */ w(u, {
			title: "Console unavailable",
			children: [/* @__PURE__ */ C(P, { children: _ }), /* @__PURE__ */ C("div", {
				className: "mt-3 flex justify-end",
				children: /* @__PURE__ */ C(a, {
					variant: "primary",
					onClick: () => void y(),
					children: "Retry"
				})
			})]
		})
	}) : o ? /* @__PURE__ */ w("div", {
		className: "mx-auto max-w-[1600px] px-4 py-6 sm:px-6",
		children: [/* @__PURE__ */ C("header", {
			className: "animate-rise",
			children: /* @__PURE__ */ w("div", {
				className: "flex flex-wrap items-end justify-between gap-4",
				children: [/* @__PURE__ */ w("div", {
					className: "min-w-0",
					children: [/* @__PURE__ */ C(d, { children: "Intelligence · Search" }), /* @__PURE__ */ C("h1", {
						className: "mt-1 font-display text-2xl font-extrabold tracking-tight text-fg",
						children: "Semantic Search"
					})]
				}), /* @__PURE__ */ w("div", {
					className: "flex items-center gap-3",
					children: [/* @__PURE__ */ w("div", {
						className: "flex flex-col items-end leading-none",
						children: [/* @__PURE__ */ C("span", {
							className: "font-mono text-[12px] font-semibold text-fg",
							children: o.name
						}), /* @__PURE__ */ w("span", {
							className: "mt-1 font-mono text-[9px] uppercase tracking-micro text-accent",
							children: [o.role, o.kind === "system" && /* @__PURE__ */ C("span", {
								className: "text-fg-muted",
								children: " · auth off"
							})]
						})]
					}), o.kind === "user" && /* @__PURE__ */ C(a, {
						size: "sm",
						onClick: () => void O(),
						children: "Sign out"
					})]
				})]
			})
		}), /* @__PURE__ */ C("div", {
			className: "mt-5",
			children: /* @__PURE__ */ C(q, { nameFor: D })
		})]
	}) : null;
}
//#endregion
//#region src/modules/search/entry.tsx
var Y = J;
//#endregion
export { Y as default };
