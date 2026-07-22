import { useCallback as e, useEffect as t, useMemo as n, useRef as r, useState as i } from "react";
import { useNavigate as a } from "react-router-dom";
import { ApiError as o, Button as s, EmptyState as c, Field as l, Input as u, Login as d, Panel as f, SectionLabel as p, Select as m, Spinner as h, Stat as g, StatusPill as _, api as v, cx as y, formatClock as b, localInputToIso as x, setAuthToken as S, usePoll as C } from "@heldar/shell";
import { Fragment as w, jsx as T, jsxs as E } from "react/jsx-runtime";
//#region src/modules/search/page.tsx
var D = {
	matched: "recording",
	exception: "connecting",
	blocked: "error",
	unmatched: "offline"
}, O = {
	matched: "#10b981",
	exception: "#fbbf24",
	blocked: "#ef4444",
	unmatched: "#52525b"
}, k = {
	entry: "#38bdf8",
	zone: "#a78bfa",
	breach: "#ef4444"
}, A = {
	llm: "#f59e0b",
	rules: "#38bdf8",
	structured: "#a78bfa"
}, j = {
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
}, M = {
	inference: 0,
	aggregate: 1,
	event: 2,
	track: 3,
	observation: 4
}, N = [
	"white cars entering after 6pm last week",
	"unauthorized vehicles today",
	"red zone breaches yesterday"
];
function P({ label: e, color: t }) {
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
function F({ className: e }) {
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
function I({ children: e }) {
	return /* @__PURE__ */ E("div", {
		role: "alert",
		className: "flex items-start gap-2 rounded-md border border-danger/40 bg-danger/10 px-3 py-2 font-mono text-xs text-red-300",
		children: [/* @__PURE__ */ T(F, { className: "mt-0.5 shrink-0" }), /* @__PURE__ */ T("span", {
			className: "break-words",
			children: e
		})]
	});
}
function L(e, t) {
	if (!e) return null;
	let n = e[t];
	return typeof n == "string" ? n.trim() ? n : null : typeof n == "number" || typeof n == "boolean" ? String(n) : null;
}
function R({ path: e, alt: t }) {
	let [n, r] = i(!1);
	return n ? null : /* @__PURE__ */ T("img", {
		src: e,
		alt: t,
		loading: "lazy",
		onError: () => r(!0),
		className: "h-16 w-24 shrink-0 rounded-md border border-line bg-black object-cover"
	});
}
function z({ children: e }) {
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
function B(e) {
	return `${String(e).padStart(2, "0")}:00`;
}
function V({ label: e, value: t }) {
	return /* @__PURE__ */ E("span", {
		className: "inline-flex items-center gap-1.5 rounded-md border border-line bg-canvas px-2 py-1 leading-none",
		children: [/* @__PURE__ */ T("span", {
			className: "font-mono text-[9px] uppercase tracking-micro text-fg-muted",
			children: e
		}), /* @__PURE__ */ T("span", {
			className: "font-mono text-[11px] font-semibold text-fg",
			children: t
		})]
	});
}
function H(e, t) {
	let n = [];
	return e.from && n.push({
		label: "From",
		value: b(e.from)
	}), e.to && n.push({
		label: "To",
		value: b(e.to)
	}), e.hour_min != null && n.push({
		label: "After",
		value: `${B(e.hour_min)} UTC`
	}), e.hour_max != null && n.push({
		label: "Before",
		value: `${B(e.hour_max)} UTC`
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
function U({ plan: e, planner: t, nameFor: r, dryRun: i }) {
	let a = n(() => H(e, r), [e, r]), o = A[t] ?? "#71717a";
	return /* @__PURE__ */ E(f, {
		title: "Interpreted as",
		subtitle: "The structured plan your question was turned into — the only inference in the answer",
		actions: /* @__PURE__ */ E("div", {
			className: "flex items-center gap-2",
			children: [i && /* @__PURE__ */ T(P, {
				label: "Dry run",
				color: "#fbbf24"
			}), /* @__PURE__ */ T(P, {
				label: `Planner · ${t}`,
				color: o
			})]
		}),
		children: [a.length === 0 ? /* @__PURE__ */ E("p", {
			className: "font-mono text-[11px] leading-relaxed text-fg-secondary",
			children: [
				"No filters were extracted — this defaults to",
				" ",
				/* @__PURE__ */ T("span", {
					className: "text-fg",
					children: "all sources"
				}),
				" over the last ~7 days. Add detail (color, time, camera, authorization) to narrow it."
			]
		}) : /* @__PURE__ */ T("div", {
			className: "flex flex-wrap gap-2",
			children: a.map((e) => /* @__PURE__ */ T(V, {
				label: e.label,
				value: e.value
			}, e.label))
		}), /* @__PURE__ */ E("p", {
			className: "mt-3 flex items-start gap-1.5 border-t border-line pt-3 font-mono text-[10px] leading-relaxed text-fg-muted",
			children: [/* @__PURE__ */ T(F, { className: "mt-0.5 shrink-0 text-fg-muted/80" }), /* @__PURE__ */ E("span", { children: [
				"Verify this reflects your intent — the planner only decides",
				" ",
				/* @__PURE__ */ T("span", {
					className: "text-fg-secondary",
					children: "how to query"
				}),
				". The results are exactly what this plan selected, nothing more."
			] })]
		})]
	});
}
function W(e, t) {
	let n = e[t];
	return typeof n == "string" && n.trim() ? n : null;
}
function G({ proof: e }) {
	let t = n(() => {
		let t = [...e.claim_levels ?? []];
		return t.sort((e, t) => (M[String(e.level ?? "")] ?? 99) - (M[String(t.level ?? "")] ?? 99)), t;
	}, [e.claim_levels]);
	return /* @__PURE__ */ E(f, {
		title: "Proof",
		subtitle: "Why this answer can be trusted — facts at the bottom, interpretation at the top",
		children: [
			/* @__PURE__ */ E("p", {
				className: "mb-4 rounded-md border border-accent/30 bg-accent/[0.06] px-3 py-2 text-xs leading-relaxed text-fg-secondary",
				children: [
					/* @__PURE__ */ T("span", {
						className: "font-semibold text-fg",
						children: "The answers are facts; the interpretation is the only inference."
					}),
					" ",
					"Each rung below states a claim, its confidence, and the caveat that bounds it."
				]
			}),
			/* @__PURE__ */ E("ol", {
				className: "relative space-y-3 pl-5",
				children: [/* @__PURE__ */ T("span", {
					className: "absolute left-[5px] top-2 bottom-2 w-px bg-line",
					"aria-hidden": "true"
				}), t.map((e, t) => {
					let n = W(e, "level") ?? "—", r = j[n] ?? {
						color: "#71717a",
						blurb: ""
					}, i = W(e, "statement"), a = W(e, "confidence"), o = W(e, "caveat"), s = W(e, "basis"), c = W(e, "provenance");
					return /* @__PURE__ */ E("li", {
						className: "relative",
						children: [/* @__PURE__ */ T("span", {
							className: "absolute -left-5 top-1.5 h-2.5 w-2.5 rounded-full border-2 border-canvas",
							style: { backgroundColor: r.color },
							"aria-hidden": "true"
						}), /* @__PURE__ */ E("div", {
							className: "rounded-md border border-line bg-panel2/40 p-3",
							style: {
								borderLeftColor: r.color,
								borderLeftWidth: 3
							},
							children: [
								/* @__PURE__ */ E("div", {
									className: "flex flex-wrap items-center gap-2",
									children: [
										/* @__PURE__ */ T(P, {
											label: n,
											color: r.color
										}),
										r.blurb && /* @__PURE__ */ T("span", {
											className: "font-mono text-[10px] text-fg-muted",
											children: r.blurb
										}),
										a && /* @__PURE__ */ E("span", {
											className: "ml-auto whitespace-nowrap font-mono text-[10px] text-fg-secondary",
											children: ["confidence:\xA0", /* @__PURE__ */ T("span", {
												className: "text-fg",
												children: a
											})]
										})
									]
								}),
								i && /* @__PURE__ */ T("p", {
									className: "mt-2 text-xs leading-relaxed text-fg-secondary",
									children: i
								}),
								s && /* @__PURE__ */ E("p", {
									className: "mt-1.5 font-mono text-[10px] leading-relaxed text-fg-muted",
									children: ["basis: ", s]
								}),
								c && /* @__PURE__ */ E("p", {
									className: "mt-1.5 font-mono text-[10px] leading-relaxed text-fg-muted",
									children: ["provenance: ", c]
								}),
								o && /* @__PURE__ */ E("p", {
									className: "mt-2 flex items-start gap-1.5 rounded border border-connecting/30 bg-connecting/[0.06] px-2 py-1.5 font-mono text-[10px] leading-relaxed text-connecting",
									children: [/* @__PURE__ */ T(F, { className: "mt-0.5 shrink-0" }), /* @__PURE__ */ T("span", { children: o })]
								})
							]
						})]
					}, `${n}-${t}`);
				})]
			}),
			e.note && /* @__PURE__ */ T("p", {
				className: "mt-4 border-t border-line pt-3 font-mono text-[10px] leading-relaxed text-fg-muted",
				children: e.note
			})
		]
	});
}
function K({ hit: e, nameFor: t }) {
	let n = k[e.source] ?? "#71717a", r = (e.auth_status ? O[e.auth_status] : void 0) ?? n, i = L(e.subject, "color"), a = L(e.subject, "vehicle_type"), o = L(e.subject, "label"), s = L(e.subject, "subject_type") ?? L(e.subject, "type"), c = L(e.subject, "severity");
	return /* @__PURE__ */ E("div", {
		className: "flex gap-3 rounded-md border border-line bg-panel2/40 p-3 transition-colors duration-150 hover:border-[#34373e]",
		style: {
			borderLeftColor: r,
			borderLeftWidth: 3
		},
		children: [e.evidence_path && /* @__PURE__ */ T(R, {
			path: e.evidence_path,
			alt: `${e.source} ${e.plate ?? e.id}`
		}), /* @__PURE__ */ E("div", {
			className: "min-w-0 flex-1",
			children: [
				/* @__PURE__ */ E("div", {
					className: "flex flex-wrap items-center gap-2",
					children: [
						/* @__PURE__ */ T(P, {
							label: e.source,
							color: n
						}),
						/* @__PURE__ */ T("span", {
							className: "font-mono text-[10px] uppercase tracking-micro text-fg-muted",
							children: e.kind
						}),
						e.claim_level && /* @__PURE__ */ T(P, {
							label: e.claim_level,
							color: "#52525b"
						}),
						/* @__PURE__ */ T("span", {
							className: "ml-auto whitespace-nowrap font-mono text-[10px] text-fg-muted",
							children: b(e.timestamp)
						})
					]
				}),
				/* @__PURE__ */ E("div", {
					className: "mt-2 flex flex-wrap items-center gap-2",
					children: [e.plate ? /* @__PURE__ */ T("span", {
						className: "font-mono text-base font-semibold tracking-wide text-fg",
						children: e.plate
					}) : /* @__PURE__ */ T("span", {
						className: "font-mono text-sm text-fg-secondary",
						children: o ?? s ?? "—"
					}), e.auth_status && /* @__PURE__ */ T(_, {
						state: D[e.auth_status] ?? "unknown",
						label: e.auth_status
					})]
				}),
				/* @__PURE__ */ E("div", {
					className: "mt-1.5 flex flex-wrap gap-x-3 gap-y-0.5 font-mono text-[10px] text-fg-secondary",
					children: [
						/* @__PURE__ */ E("span", {
							className: "text-fg-muted",
							children: ["camera:\xA0", /* @__PURE__ */ T("span", {
								className: "text-fg-secondary",
								children: t(e.camera_id)
							})]
						}),
						e.zone && /* @__PURE__ */ E("span", {
							className: "text-fg-muted",
							children: [
								"zone:\xA0",
								/* @__PURE__ */ T("span", {
									className: "text-fg-secondary",
									children: e.zone
								}),
								e.zone_kind ? /* @__PURE__ */ E("span", {
									className: "text-fg-muted",
									children: [
										" (",
										e.zone_kind,
										")"
									]
								}) : null
							]
						}),
						s && e.plate && /* @__PURE__ */ E("span", {
							className: "text-fg-muted",
							children: ["subject:\xA0", /* @__PURE__ */ T("span", {
								className: "text-fg-secondary",
								children: s
							})]
						}),
						a && /* @__PURE__ */ E("span", {
							className: "text-fg-muted",
							children: ["type:\xA0", /* @__PURE__ */ T("span", {
								className: "text-fg-secondary",
								children: a
							})]
						}),
						i && /* @__PURE__ */ E("span", {
							className: "text-fg-muted",
							children: ["color:\xA0", /* @__PURE__ */ T("span", {
								className: "text-fg-secondary",
								children: i
							})]
						}),
						c && /* @__PURE__ */ E("span", {
							className: "text-fg-muted",
							children: ["severity:\xA0", /* @__PURE__ */ T("span", {
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
function q({ result: e, nameFor: t }) {
	let r = n(() => {
		let t = 0, n = 0, r = 0;
		for (let i of e.hits) i.source === "entry" ? t += 1 : i.source === "zone" ? n += 1 : i.source === "breach" && (r += 1);
		return {
			entry: t,
			zone: n,
			breach: r
		};
	}, [e.hits]);
	return /* @__PURE__ */ E(w, { children: [/* @__PURE__ */ E("div", {
		className: "grid grid-cols-2 gap-px overflow-hidden rounded-panel border border-line bg-line sm:grid-cols-4",
		children: [
			/* @__PURE__ */ T("div", {
				className: "bg-panel px-4 py-3",
				children: /* @__PURE__ */ T(g, {
					label: "Matches",
					value: e.count
				})
			}),
			/* @__PURE__ */ T("div", {
				className: "bg-panel px-4 py-3",
				children: /* @__PURE__ */ T(g, {
					label: "Entry",
					value: r.entry
				})
			}),
			/* @__PURE__ */ T("div", {
				className: "bg-panel px-4 py-3",
				children: /* @__PURE__ */ T(g, {
					label: "Zone",
					value: r.zone
				})
			}),
			/* @__PURE__ */ T("div", {
				className: "bg-panel px-4 py-3",
				children: /* @__PURE__ */ T(g, {
					label: "Breach",
					value: r.breach,
					tone: r.breach > 0 ? "bad" : "default"
				})
			})
		]
	}), /* @__PURE__ */ T(f, {
		title: "Results",
		subtitle: "Stored events matching the executed plan — newest first",
		actions: /* @__PURE__ */ T("span", {
			className: "font-mono text-[11px] tabular-nums text-fg-muted",
			children: e.count
		}),
		children: e.hits.length === 0 ? /* @__PURE__ */ T(c, {
			title: "No matching events",
			hint: "The plan ran cleanly but no stored events matched. Loosen the filters above, widen the time window, or check the interpreted plan."
		}) : /* @__PURE__ */ T("div", {
			className: "space-y-2.5",
			children: e.hits.map((e) => /* @__PURE__ */ T(K, {
				hit: e,
				nameFor: t
			}, `${e.source}-${e.id}`))
		})
	})] });
}
function J({ busy: e, onRun: t }) {
	let [n, r] = i(""), [a, o] = i(""), [c, d] = i(""), [f, p] = i(""), [g, _] = i("");
	function v(e) {
		e.preventDefault();
		let r = {};
		n && (r.sources = [n]), a && (r.auth_status = [a]), c.trim() && (r.color = c.trim());
		let i = x(f);
		i && (r.from = i);
		let o = x(g);
		o && (r.to = o), t(r);
	}
	return /* @__PURE__ */ E("form", {
		onSubmit: v,
		className: "space-y-4",
		children: [/* @__PURE__ */ E("div", {
			className: "grid grid-cols-1 gap-3 sm:grid-cols-2 lg:grid-cols-3",
			children: [
				/* @__PURE__ */ T(l, {
					label: "Source",
					htmlFor: "sf-source",
					children: /* @__PURE__ */ E(m, {
						id: "sf-source",
						value: n,
						onChange: (e) => r(e.target.value),
						children: [
							/* @__PURE__ */ T("option", {
								value: "",
								children: "Any source"
							}),
							/* @__PURE__ */ T("option", {
								value: "entry",
								children: "Entry"
							}),
							/* @__PURE__ */ T("option", {
								value: "zone",
								children: "Zone"
							}),
							/* @__PURE__ */ T("option", {
								value: "breach",
								children: "Breach"
							})
						]
					})
				}),
				/* @__PURE__ */ T(l, {
					label: "Authorization",
					htmlFor: "sf-auth",
					children: /* @__PURE__ */ E(m, {
						id: "sf-auth",
						value: a,
						onChange: (e) => o(e.target.value),
						children: [
							/* @__PURE__ */ T("option", {
								value: "",
								children: "Any status"
							}),
							/* @__PURE__ */ T("option", {
								value: "matched",
								children: "Matched"
							}),
							/* @__PURE__ */ T("option", {
								value: "exception",
								children: "Exception"
							}),
							/* @__PURE__ */ T("option", {
								value: "unmatched",
								children: "Unmatched"
							}),
							/* @__PURE__ */ T("option", {
								value: "blocked",
								children: "Blocked"
							})
						]
					})
				}),
				/* @__PURE__ */ T(l, {
					label: "Color",
					htmlFor: "sf-color",
					children: /* @__PURE__ */ T(u, {
						id: "sf-color",
						value: c,
						onChange: (e) => d(e.target.value),
						placeholder: "white",
						autoComplete: "off"
					})
				}),
				/* @__PURE__ */ T(l, {
					label: "From",
					htmlFor: "sf-from",
					children: /* @__PURE__ */ T(u, {
						id: "sf-from",
						type: "datetime-local",
						step: 1,
						value: f,
						onChange: (e) => p(e.target.value)
					})
				}),
				/* @__PURE__ */ T(l, {
					label: "To",
					htmlFor: "sf-to",
					children: /* @__PURE__ */ T(u, {
						id: "sf-to",
						type: "datetime-local",
						step: 1,
						value: g,
						onChange: (e) => _(e.target.value)
					})
				})
			]
		}), /* @__PURE__ */ T("div", {
			className: "flex justify-end",
			children: /* @__PURE__ */ T(s, {
				type: "submit",
				variant: "primary",
				disabled: e,
				children: e ? /* @__PURE__ */ E(w, { children: [/* @__PURE__ */ T(h, { size: 14 }), "Running…"] }) : "Run structured query"
			})
		})]
	});
}
var ee = [
	12,
	24,
	48
];
function Y({ path: e, alt: t }) {
	let [n, r] = i(!1);
	return n ? null : /* @__PURE__ */ T("a", {
		href: e,
		target: "_blank",
		rel: "noreferrer",
		className: "group shrink-0 self-start",
		title: "Open crop",
		children: /* @__PURE__ */ T("img", {
			src: e,
			alt: t,
			loading: "lazy",
			onError: () => r(!0),
			className: "h-28 w-40 rounded-md border border-line bg-black object-cover transition-colors duration-150 group-hover:border-accent"
		})
	});
}
function te({ hit: e, rank: t, nameFor: n, onPlayback: r }) {
	return /* @__PURE__ */ E("div", {
		className: "flex gap-3 rounded-md border border-line bg-panel2/40 p-3 transition-colors duration-150 hover:border-[#34373e]",
		children: [e.evidence_path && /* @__PURE__ */ T(Y, {
			path: e.evidence_path,
			alt: e.label ?? "match"
		}), /* @__PURE__ */ E("div", {
			className: "flex min-w-0 flex-1 flex-col",
			children: [
				/* @__PURE__ */ E("div", {
					className: "flex flex-wrap items-center gap-2",
					children: [
						/* @__PURE__ */ E("span", {
							className: "font-mono text-[10px] tabular-nums text-fg-muted",
							children: ["#", t]
						}),
						/* @__PURE__ */ T(P, {
							label: `${(e.score * 100).toFixed(0)}%`,
							color: "#a78bfa"
						}),
						/* @__PURE__ */ T("span", {
							className: "ml-auto whitespace-nowrap font-mono text-[10px] text-fg-muted",
							children: b(e.timestamp)
						})
					]
				}),
				/* @__PURE__ */ T("div", {
					className: "mt-2 truncate font-mono text-sm text-fg",
					children: e.label ?? "—"
				}),
				/* @__PURE__ */ E("div", {
					className: "mt-1 flex flex-wrap gap-x-3 gap-y-0.5 font-mono text-[10px] text-fg-muted",
					children: [
						/* @__PURE__ */ E("span", { children: ["camera:\xA0", /* @__PURE__ */ T("span", {
							className: "text-fg-secondary",
							children: n(e.camera_id)
						})] }),
						e.track_id && /* @__PURE__ */ E("span", { children: ["track:\xA0", /* @__PURE__ */ E("span", {
							className: "text-fg-secondary",
							children: ["#", e.track_id]
						})] }),
						e.detection?.confidence != null && /* @__PURE__ */ E("span", { children: ["det\xA0conf:\xA0", /* @__PURE__ */ T("span", {
							className: "text-fg-secondary",
							children: e.detection.confidence.toFixed(2)
						})] })
					]
				}),
				/* @__PURE__ */ T("div", {
					className: "mt-auto pt-2",
					children: /* @__PURE__ */ T(s, {
						size: "sm",
						onClick: () => r(e),
						children: "Playback"
					})
				})
			]
		})]
	});
}
function X({ nameFor: e, cameras: t }) {
	let n = a(), d = r(null), [p, g] = i(""), [_, y] = i(null), [b, S] = i(null), [C, D] = i(null), [O, k] = i(""), [A, j] = i(""), [M, N] = i(""), [P, L] = i(""), [R, B] = i(24), [V, H] = i(null), [U, W] = i(null), [G, K] = i(null), [q, J] = i(!1), Y = p.trim().length > 0;
	function X(e) {
		if (!e || Y) return;
		if (!e.type.startsWith("image/")) {
			W(`"${e.name}" is not an image — drop a JPEG, PNG, WebP, or GIF.`);
			return;
		}
		if (e.size === 0) {
			W(`"${e.name}" is empty.`);
			return;
		}
		if (e.size > 7340032) {
			W(`"${e.name}" is ${(e.size / (1024 * 1024)).toFixed(1)} MB — the limit is 7 MB. Resize or crop it first.`);
			return;
		}
		let t = new FileReader();
		t.onload = () => {
			let n = typeof t.result == "string" ? t.result : "", r = n.indexOf(",");
			if (r < 0) {
				W("Could not read that file as an image.");
				return;
			}
			y(n.slice(r + 1)), D(n), S(e.name), W(null);
		}, t.onerror = () => W("Could not read that file as an image."), t.readAsDataURL(e);
	}
	function Z() {
		y(null), S(null), D(null), d.current && (d.current.value = "");
	}
	async function Q(e) {
		e.preventDefault();
		let t = p.trim();
		if (!t && !_) return;
		H("semantic"), W(null);
		let n = t ? { text: t } : { image_b64: _ }, r = x(O);
		r && (n.from = r);
		let i = x(A);
		i && (n.to = i), M && (n.cameras = [M]);
		let a = P.trim();
		a && (n.label = a), n.k = R;
		try {
			K(await v.searchSemantic(n)), J(!0);
		} catch (e) {
			e instanceof o && e.status === 503 ? W("Embedding worker offline — semantic search needs a running AI worker with the CLIP extra installed.") : W(e instanceof o ? e.message : String(e)), K(null), J(!0);
		} finally {
			H(null);
		}
	}
	function $(e) {
		let t = new Date(e.timestamp).getTime(), r = (/* @__PURE__ */ new Date(t - 6e4)).toISOString(), i = new Date(t + 6e4).toISOString();
		n(`/playback?camera=${encodeURIComponent(e.camera_id)}&from=${encodeURIComponent(r)}&to=${encodeURIComponent(i)}`);
	}
	return /* @__PURE__ */ E("div", {
		className: "stagger space-y-4",
		children: [
			/* @__PURE__ */ E(z, { children: [
				/* @__PURE__ */ T("span", {
					className: "text-fg",
					children: "Similarity-ranked, not verified."
				}),
				" These results are CLIP embedding matches — detection crops ranked by how visually close they are to your text or example image. A score is a relative rank,",
				" ",
				/* @__PURE__ */ T("span", {
					className: "text-fg",
					children: "not a probability and not a stored fact"
				}),
				" — confirm anything that matters in Playback before acting on it."
			] }),
			/* @__PURE__ */ E(f, {
				title: "Find",
				subtitle: "Describe what you're looking for, or match an example image",
				children: [/* @__PURE__ */ E("form", {
					onSubmit: Q,
					className: "space-y-4",
					children: [
						/* @__PURE__ */ E("div", {
							className: "grid grid-cols-1 gap-3 lg:grid-cols-2",
							children: [/* @__PURE__ */ T(l, {
								label: "Describe it",
								htmlFor: "sem-text",
								children: /* @__PURE__ */ T(u, {
									id: "sem-text",
									value: p,
									onChange: (e) => g(e.target.value),
									placeholder: "red pickup truck",
									autoComplete: "off",
									disabled: _ != null
								})
							}), /* @__PURE__ */ E(l, {
								label: "Or match an image",
								htmlFor: "sem-image",
								children: [/* @__PURE__ */ T("input", {
									ref: d,
									id: "sem-image",
									type: "file",
									accept: "image/*",
									className: "hidden",
									disabled: V !== null || Y,
									onChange: (e) => {
										X(e.target.files?.[0]), e.target.value = "";
									}
								}), _ ? /* @__PURE__ */ E("div", {
									className: "flex items-center gap-2 rounded-md border border-line bg-panel2 px-2 py-1.5",
									children: [
										C && /* @__PURE__ */ T("img", {
											src: C,
											alt: b ?? "query image",
											className: "h-9 w-14 shrink-0 rounded border border-line bg-black object-cover"
										}),
										/* @__PURE__ */ T("span", {
											className: "min-w-0 flex-1 truncate font-mono text-[11px] text-fg-secondary",
											children: b
										}),
										/* @__PURE__ */ T(s, {
											size: "sm",
											onClick: Z,
											disabled: V !== null,
											children: "Clear"
										})
									]
								}) : /* @__PURE__ */ E("button", {
									type: "button",
									onClick: () => d.current?.click(),
									disabled: V !== null || Y,
									onDragOver: (e) => e.preventDefault(),
									onDrop: (e) => {
										e.preventDefault(), Y || X(e.dataTransfer.files?.[0]);
									},
									className: "flex w-full items-center justify-center gap-2 rounded-md border border-dashed border-line bg-panel2 px-3 py-2 font-mono text-[11px] text-fg-muted transition-colors duration-150 hover:border-accent/50 hover:text-fg-secondary disabled:cursor-not-allowed disabled:opacity-50",
									children: [/* @__PURE__ */ E("svg", {
										viewBox: "0 0 16 16",
										width: "12",
										height: "12",
										fill: "none",
										stroke: "currentColor",
										strokeWidth: "1.5",
										strokeLinecap: "round",
										strokeLinejoin: "round",
										"aria-hidden": "true",
										children: [
											/* @__PURE__ */ T("path", { d: "M8 10.5V3" }),
											/* @__PURE__ */ T("path", { d: "M5 5.5l3-3 3 3" }),
											/* @__PURE__ */ T("path", { d: "M2.5 10.5v2a1 1 0 0 0 1 1h9a1 1 0 0 0 1-1v-2" })
										]
									}), Y ? "Text query set — clear it to use an image" : "Drop an image or click to browse"]
								})]
							})]
						}),
						/* @__PURE__ */ E("div", {
							className: "grid grid-cols-2 gap-3 sm:grid-cols-4",
							children: [
								/* @__PURE__ */ T(l, {
									label: "From",
									htmlFor: "sem-from",
									children: /* @__PURE__ */ T(u, {
										id: "sem-from",
										type: "datetime-local",
										step: 1,
										value: O,
										onChange: (e) => k(e.target.value)
									})
								}),
								/* @__PURE__ */ T(l, {
									label: "To",
									htmlFor: "sem-to",
									children: /* @__PURE__ */ T(u, {
										id: "sem-to",
										type: "datetime-local",
										step: 1,
										value: A,
										onChange: (e) => j(e.target.value)
									})
								}),
								/* @__PURE__ */ T(l, {
									label: "Camera",
									htmlFor: "sem-camera",
									children: /* @__PURE__ */ E(m, {
										id: "sem-camera",
										value: M,
										onChange: (e) => N(e.target.value),
										children: [/* @__PURE__ */ T("option", {
											value: "",
											children: "All cameras"
										}), t.map((e) => /* @__PURE__ */ T("option", {
											value: e.id,
											children: e.name
										}, e.id))]
									})
								}),
								/* @__PURE__ */ T(l, {
									label: "Label (optional)",
									htmlFor: "sem-label",
									children: /* @__PURE__ */ T(u, {
										id: "sem-label",
										value: P,
										onChange: (e) => L(e.target.value),
										placeholder: "car",
										autoComplete: "off"
									})
								}),
								/* @__PURE__ */ T(l, {
									label: "Results",
									htmlFor: "sem-k",
									children: /* @__PURE__ */ T(m, {
										id: "sem-k",
										value: R,
										onChange: (e) => B(Number(e.target.value)),
										children: ee.map((e) => /* @__PURE__ */ E("option", {
											value: e,
											children: ["Top ", e]
										}, e))
									})
								})
							]
						}),
						/* @__PURE__ */ T("div", {
							className: "flex justify-end",
							children: /* @__PURE__ */ T(s, {
								type: "submit",
								variant: "primary",
								disabled: V !== null || !Y && !_,
								children: V === "semantic" ? /* @__PURE__ */ E(w, { children: [/* @__PURE__ */ T(h, { size: 14 }), "Searching…"] }) : "Search by similarity"
							})
						})
					]
				}), U && /* @__PURE__ */ T("div", {
					className: "mt-3",
					children: /* @__PURE__ */ T(I, { children: U })
				})]
			}),
			G && /* @__PURE__ */ E(f, {
				title: "Ranked matches",
				subtitle: `Crops ranked by similarity to "${G.query}"${G.model ? ` · ${G.model}` : ""}`,
				actions: /* @__PURE__ */ T("span", {
					className: "font-mono text-[11px] tabular-nums text-fg-muted",
					children: G.count
				}),
				children: [G.truncated && /* @__PURE__ */ E("p", {
					className: "mb-3 flex items-start gap-1.5 rounded border border-connecting/30 bg-connecting/[0.06] px-2 py-1.5 font-mono text-[10px] leading-relaxed text-connecting",
					children: [/* @__PURE__ */ T(F, { className: "mt-0.5 shrink-0" }), /* @__PURE__ */ T("span", { children: "The candidate scan hit its cap before covering the whole window — narrow the time range or camera filter for a complete ranking." })]
				}), G.hits.length === 0 ? /* @__PURE__ */ T(c, {
					title: "No similar crops found",
					hint: "Nothing embedded in this window resembles the query. Widen the time range, check the camera filter, or confirm an embedding AI task is running for these cameras."
				}) : /* @__PURE__ */ T("div", {
					className: "grid grid-cols-1 gap-3 md:grid-cols-2 xl:grid-cols-3",
					children: G.hits.map((t, n) => /* @__PURE__ */ T(te, {
						hit: t,
						rank: n + 1,
						nameFor: e,
						onPlayback: $
					}, t.id))
				})]
			}),
			!q && /* @__PURE__ */ T(c, {
				title: "Search by what it looks like",
				hint: "Type a description ('red pickup truck') or drop an example image. Crops from the embedding index are ranked by visual similarity — a recall tool for finding footage, not a source of verified facts."
			})
		]
	});
}
function Z({ nameFor: t }) {
	let [n, r] = i(""), [a, d] = i(null), [p, m] = i(null), [g, _] = i(null), [b, x] = i(null), [S, C] = i(!1), [D, O] = i(!1), k = e(async (e) => {
		let t = e.trim();
		if (t) {
			r(t), _("nl"), x(null), m(null);
			try {
				d(await v.searchNl(t)), C(!0);
			} catch (e) {
				x(e instanceof o ? e.message : String(e)), d(null), C(!0);
			} finally {
				_(null);
			}
		}
	}, []);
	async function A() {
		let e = n.trim();
		if (e) {
			_("plan"), x(null), d(null);
			try {
				m(await v.searchPlan(e)), C(!0);
			} catch (e) {
				x(e instanceof o ? e.message : String(e)), m(null), C(!0);
			} finally {
				_(null);
			}
		}
	}
	let j = e(async (e) => {
		_("structured"), x(null), m(null);
		try {
			d(await v.searchEvents(e)), C(!0);
		} catch (e) {
			x(e instanceof o ? e.message : String(e)), d(null), C(!0);
		} finally {
			_(null);
		}
	}, []);
	function M(e) {
		e.preventDefault(), k(n);
	}
	let P = p?.plan ?? a?.plan ?? null, F = p?.planner ?? a?.planner ?? "rules";
	return /* @__PURE__ */ E("div", {
		className: "stagger space-y-4",
		children: [
			/* @__PURE__ */ E(z, { children: [
				/* @__PURE__ */ T("span", {
					className: "text-fg",
					children: "Ask in plain language; the answer is the data."
				}),
				" A planner (transparent rules, or an optional LLM) translates your question into a structured query — that interpretation is the only inference. The plan then runs deterministically over the kernel's stored events, so",
				" ",
				/* @__PURE__ */ T("span", {
					className: "text-fg",
					children: "the answers are facts, the interpretation is the only inference"
				}),
				". Every search is logged; plate-targeted queries are audited."
			] }),
			/* @__PURE__ */ E(f, {
				title: "Ask",
				subtitle: "Natural-language search over entry, zone & breach events",
				children: [
					/* @__PURE__ */ E("form", {
						onSubmit: M,
						className: "flex flex-col gap-3 sm:flex-row sm:items-end",
						children: [/* @__PURE__ */ T("div", {
							className: "min-w-0 flex-1",
							children: /* @__PURE__ */ T(l, {
								label: "Query",
								htmlFor: "nl-query",
								children: /* @__PURE__ */ T(u, {
									id: "nl-query",
									value: n,
									onChange: (e) => r(e.target.value),
									placeholder: "white cars entering after 6pm last week",
									autoComplete: "off"
								})
							})
						}), /* @__PURE__ */ E("div", {
							className: "flex shrink-0 items-center gap-2",
							children: [/* @__PURE__ */ T(s, {
								type: "submit",
								variant: "primary",
								disabled: g !== null || !n.trim(),
								children: g === "nl" ? /* @__PURE__ */ E(w, { children: [/* @__PURE__ */ T(h, { size: 14 }), "Searching…"] }) : "Search"
							}), /* @__PURE__ */ T(s, {
								type: "button",
								onClick: () => void A(),
								disabled: g !== null || !n.trim(),
								children: g === "plan" ? /* @__PURE__ */ E(w, { children: [/* @__PURE__ */ T(h, { size: 14 }), "Planning…"] }) : "Plan only (dry-run)"
							})]
						})]
					}),
					/* @__PURE__ */ E("div", {
						className: "mt-3 flex flex-wrap items-center gap-2",
						children: [/* @__PURE__ */ T("span", {
							className: "font-mono text-[9px] uppercase tracking-micro text-fg-muted",
							children: "Try"
						}), N.map((e) => /* @__PURE__ */ T("button", {
							type: "button",
							disabled: g !== null,
							onClick: () => void k(e),
							className: "rounded-full border border-line bg-canvas px-2.5 py-1 font-mono text-[10px] text-fg-secondary transition-colors duration-150 hover:border-accent/50 hover:text-fg disabled:cursor-not-allowed disabled:opacity-50",
							children: e
						}, e))]
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
						}), "Searches are logged · plate-targeted queries are audited."]
					}),
					b && /* @__PURE__ */ T("div", {
						className: "mt-3",
						children: /* @__PURE__ */ T(I, { children: b })
					}),
					/* @__PURE__ */ E("div", {
						className: "mt-4 border-t border-line pt-3",
						children: [/* @__PURE__ */ E("button", {
							type: "button",
							onClick: () => O((e) => !e),
							className: "flex items-center gap-1.5 font-mono text-[10px] font-semibold uppercase tracking-micro text-fg-secondary transition-colors duration-150 hover:text-fg",
							children: [/* @__PURE__ */ T("svg", {
								viewBox: "0 0 16 16",
								width: "12",
								height: "12",
								fill: "none",
								stroke: "currentColor",
								strokeWidth: "1.6",
								strokeLinecap: "round",
								strokeLinejoin: "round",
								"aria-hidden": "true",
								className: y("transition-transform duration-150", D && "rotate-90"),
								children: /* @__PURE__ */ T("path", { d: "M6 4l4 4-4 4" })
							}), "Structured filters"]
						}), D && /* @__PURE__ */ T("div", {
							className: "mt-3",
							children: /* @__PURE__ */ T(J, {
								busy: g === "structured",
								onRun: (e) => void j(e)
							})
						})]
					})
				]
			}),
			P && /* @__PURE__ */ T(U, {
				plan: P,
				planner: F,
				nameFor: t,
				dryRun: p != null
			}),
			p && /* @__PURE__ */ E(f, {
				title: "Dry run — not executed",
				subtitle: "The plan above was generated but no query was run",
				children: [/* @__PURE__ */ T("p", {
					className: "text-xs leading-relaxed text-fg-secondary",
					children: "Nothing was read from the fact tables. Review the interpreted plan, then execute it exactly as shown."
				}), /* @__PURE__ */ E("div", {
					className: "mt-3 flex items-center gap-2",
					children: [/* @__PURE__ */ T(s, {
						variant: "primary",
						disabled: g !== null,
						onClick: () => void j(p.plan),
						children: g === "structured" ? /* @__PURE__ */ E(w, { children: [/* @__PURE__ */ T(h, { size: 14 }), "Running…"] }) : "Run this plan"
					}), /* @__PURE__ */ T(s, {
						disabled: g !== null,
						onClick: () => void k(n),
						children: "Re-run as search"
					})]
				})]
			}),
			a && /* @__PURE__ */ E(w, { children: [/* @__PURE__ */ T(q, {
				result: a,
				nameFor: t
			}), /* @__PURE__ */ T(G, { proof: a.proof })] }),
			!S && !P && /* @__PURE__ */ T(c, {
				title: "Ask a question to begin",
				hint: "Search in plain language across entry, zone and breach events. The interpreted plan and a proof ladder are shown with every result, so you can always see how the question was read and why the answer holds."
			})
		]
	});
}
function Q({ active: e, onClick: t, children: n }) {
	return /* @__PURE__ */ T("button", {
		type: "button",
		onClick: t,
		className: y("relative -mb-px whitespace-nowrap border-b-2 px-3.5 py-2.5 font-mono text-[11px] font-semibold uppercase tracking-micro transition-colors duration-150 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-accent focus-visible:ring-offset-2 focus-visible:ring-offset-canvas", e ? "border-accent text-fg" : "border-transparent text-fg-muted hover:text-fg-secondary"),
		children: n
	});
}
function $() {
	let [r, a] = i(null), [c, l] = i(!0), [u, m] = i(!1), [g, _] = i(null), [y, b] = i("query"), x = e(async () => {
		l(!0), _(null);
		try {
			a(await v.me()), m(!1);
		} catch (e) {
			e instanceof o && e.status === 401 ? (a(null), m(!0)) : _(e instanceof Error ? e.message : String(e));
		} finally {
			l(!1);
		}
	}, []);
	t(() => {
		x();
	}, [x]);
	let w = C(() => v.listCameras(), 0), D = w.data ?? [], O = n(() => {
		let e = /* @__PURE__ */ new Map();
		for (let t of D) e.set(t.id, t.name);
		return e;
	}, [w.data]), k = e((e) => e ? O.get(e) ?? e : "—", [O]);
	async function A() {
		try {
			await v.logout();
		} catch {}
		S(null), a(null), m(!0);
	}
	return u ? /* @__PURE__ */ T(d, { onSuccess: (e) => {
		a(e), m(!1), _(null);
	} }) : c && !r ? /* @__PURE__ */ E("div", {
		className: "flex min-h-[60vh] items-center justify-center gap-3 text-fg-secondary",
		children: [/* @__PURE__ */ T(h, {}), /* @__PURE__ */ T("span", {
			className: "font-mono text-xs uppercase tracking-micro",
			children: "Authenticating…"
		})]
	}) : g && !r ? /* @__PURE__ */ T("div", {
		className: "mx-auto max-w-md px-4 py-20",
		children: /* @__PURE__ */ E(f, {
			title: "Console unavailable",
			children: [/* @__PURE__ */ T(I, { children: g }), /* @__PURE__ */ T("div", {
				className: "mt-3 flex justify-end",
				children: /* @__PURE__ */ T(s, {
					variant: "primary",
					onClick: () => void x(),
					children: "Retry"
				})
			})]
		})
	}) : r ? /* @__PURE__ */ E("div", {
		className: "mx-auto max-w-[1600px] px-4 py-6 sm:px-6",
		children: [/* @__PURE__ */ E("header", {
			className: "animate-rise",
			children: [/* @__PURE__ */ E("div", {
				className: "flex flex-wrap items-end justify-between gap-4",
				children: [/* @__PURE__ */ E("div", {
					className: "min-w-0",
					children: [/* @__PURE__ */ T(p, { children: "Intelligence · Search" }), /* @__PURE__ */ T("h1", {
						className: "mt-1 font-display text-2xl font-extrabold tracking-tight text-fg",
						children: "Semantic Search"
					})]
				}), /* @__PURE__ */ E("div", {
					className: "flex items-center gap-3",
					children: [/* @__PURE__ */ E("div", {
						className: "flex flex-col items-end leading-none",
						children: [/* @__PURE__ */ T("span", {
							className: "font-mono text-[12px] font-semibold text-fg",
							children: r.name
						}), /* @__PURE__ */ E("span", {
							className: "mt-1 font-mono text-[9px] uppercase tracking-micro text-accent",
							children: [r.role, r.kind === "system" && /* @__PURE__ */ T("span", {
								className: "text-fg-muted",
								children: " · auth off"
							})]
						})]
					}), r.kind === "user" && /* @__PURE__ */ T(s, {
						size: "sm",
						onClick: () => void A(),
						children: "Sign out"
					})]
				})]
			}), /* @__PURE__ */ T("div", {
				className: "mt-5 flex flex-wrap gap-1 overflow-x-auto border-b border-line",
				children: [{
					key: "query",
					label: "Query"
				}, {
					key: "semantic",
					label: "Semantic"
				}].map((e) => /* @__PURE__ */ T(Q, {
					active: y === e.key,
					onClick: () => b(e.key),
					children: e.label
				}, e.key))
			})]
		}), /* @__PURE__ */ T("div", {
			className: "mt-5",
			children: y === "query" ? /* @__PURE__ */ T(Z, { nameFor: k }) : /* @__PURE__ */ T(X, {
				nameFor: k,
				cameras: D
			})
		})]
	}) : null;
}
//#endregion
//#region src/modules/search/entry.tsx
var ne = $;
//#endregion
export { ne as default };
