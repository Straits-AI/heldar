/** @type {import('tailwindcss').Config} */
export default {
  content: ["./index.html", "./src/**/*.{ts,tsx}"],
  darkMode: "class",
  theme: {
    extend: {
      colors: {
        // Dark operator / NVR control-plane palette.
        ink: "#0a0e15", // page background (near-black, faint blue)
        panel: "#10151e", // card / panel surface
        panel2: "#161d29", // raised surface (inputs, buttons)
        line: "#222b39", // hairline borders
        accent: "#38bdf8", // primary action / focus (sky)
      },
      fontFamily: {
        mono: ["ui-monospace", "SFMono-Regular", "Menlo", "Consolas", "monospace"],
      },
    },
  },
  plugins: [],
};
