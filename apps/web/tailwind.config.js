/** @type {import('tailwindcss').Config} */
// Heldar Core — "Operations console / SOC" design tokens.
// Industrial, utilitarian, dark, signal-driven. Always dark (no light theme).
export default {
  content: ["./index.html", "./src/**/*.{ts,tsx}"],
  darkMode: "class",
  theme: {
    extend: {
      colors: {
        // --- Surfaces (near-black canvas up to raised panels) ---
        canvas: "#09090b", // page background
        panel: "#121316", // primary panel surface
        panel2: "#16181c", // secondary / inset panel surface
        raised: "#1c1f24", // raised controls (buttons, inputs hover)
        line: "#26282e", // hairline borders (1px)
        hairline: "#26282e", // alias
        ink: "#09090b", // back-compat alias for canvas

        // --- Text ---
        fg: "#f4f4f5", // primary text
        "fg-secondary": "#a1a1aa", // secondary text
        "fg-muted": "#71717a", // muted / micro labels

        // --- Brand / primary accent (signal amber) ---
        accent: {
          DEFAULT: "#f59e0b",
          soft: "#fbbf24",
          deep: "#b45309",
          ink: "#1a1206", // text on top of amber fills
        },

        // --- Semantic status LEDs (camera state) ---
        rec: "#10b981", // recording / live (emerald)
        live: "#10b981",
        connecting: "#fbbf24", // amber
        offline: "#71717a", // zinc
        danger: "#ef4444", // error red
        disabled: "#3f3f46",
        unknown: "#52525b",
      },
      fontFamily: {
        display: ['"Archivo"', "ui-sans-serif", "system-ui", "sans-serif"],
        sans: ['"Hanken Grotesk"', "ui-sans-serif", "system-ui", "sans-serif"],
        mono: ['"JetBrains Mono"', "ui-monospace", "SFMono-Regular", "Menlo", "monospace"],
      },
      letterSpacing: {
        micro: "0.12em", // uppercase micro-labels
        wide2: "0.18em",
        wordmark: "0.3em", // wordmark tracking
      },
      borderRadius: {
        panel: "0.625rem",
      },
      boxShadow: {
        // Subtle inner highlight + drop for hairline panels.
        panel:
          "0 1px 0 0 rgba(255,255,255,0.025) inset, 0 1px 2px 0 rgba(0,0,0,0.5), 0 8px 24px -12px rgba(0,0,0,0.6)",
        raised:
          "0 1px 0 0 rgba(255,255,255,0.04) inset, 0 1px 1px 0 rgba(0,0,0,0.5)",
        // Amber focus / active glow.
        glow: "0 0 0 1px rgba(245,158,11,0.45), 0 0 18px -2px rgba(245,158,11,0.35)",
        "glow-rec": "0 0 8px 0 rgba(16,185,129,0.7)",
      },
      keyframes: {
        rise: {
          "0%": { opacity: "0", transform: "translateY(10px)" },
          "100%": { opacity: "1", transform: "translateY(0)" },
        },
        "led-ping": {
          "0%": { transform: "scale(1)", opacity: "0.55" },
          "70%": { opacity: "0" },
          "100%": { transform: "scale(2.6)", opacity: "0" },
        },
        "led-breathe": {
          "0%,100%": { opacity: "1" },
          "50%": { opacity: "0.5" },
        },
        "spin-slow": {
          to: { transform: "rotate(360deg)" },
        },
      },
      animation: {
        rise: "rise 0.5s cubic-bezier(0.22,1,0.36,1) both",
        "led-ping": "led-ping 1.8s cubic-bezier(0,0,0.2,1) infinite",
        "led-breathe": "led-breathe 2.2s ease-in-out infinite",
        "spin-slow": "spin-slow 0.9s linear infinite",
      },
    },
  },
  plugins: [],
};
