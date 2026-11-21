import { createContext, useContext, useEffect, useState, type ReactNode } from "react";

export type Theme = "dark" | "light";

export interface ChartTheme {
  fg: string;
  muted: string;
  border: string;
  tooltipBg: string;
  tooltipBorder: string;
  axis: string;
  cpu: string;
  mem: string;
  io: string;
  gpu: string;
  gpuMem: string;
  temp: string;
  power: string;
  heatmap: string[];
}

const DARK: ChartTheme = {
  fg: "#ececee",
  muted: "#85868d",
  border: "rgba(236,236,238,0.08)",
  tooltipBg: "#1b1c20",
  tooltipBorder: "rgba(236,236,238,0.14)",
  axis: "#85868d",
  cpu: "#7aa2f7",
  mem: "#7fc8a9",
  io: "#d9a45b",
  gpu: "#d7a1c4",
  gpuMem: "#96a9c6",
  temp: "#e0836f",
  power: "#d9a45b",
  heatmap: ["#243b5a", "#2f6f8f", "#3fa7a0", "#8fbf7f", "#e0b268", "#e56767"],
};

const LIGHT: ChartTheme = {
  fg: "#15171a",
  muted: "#6b7280",
  border: "rgba(21,23,26,0.10)",
  tooltipBg: "#ffffff",
  tooltipBorder: "rgba(21,23,26,0.14)",
  axis: "#6b7280",
  cpu: "#3567c7",
  mem: "#2f8f6b",
  io: "#a86f22",
  gpu: "#a4648f",
  gpuMem: "#607b9e",
  temp: "#c2553f",
  power: "#a86f22",
  heatmap: ["#dbe7f3", "#9cc4dd", "#54a6a0", "#a8c47a", "#e9c46a", "#e76f51"],
};

export function chartTheme(theme: Theme): ChartTheme {
  return theme === "light" ? LIGHT : DARK;
}

function initialTheme(): Theme {
  const current = document.documentElement.dataset.theme;
  if (current === "light" || current === "dark") return current;
  try {
    const saved = localStorage.getItem("ivitals-theme");
    if (saved === "light" || saved === "dark") return saved;
  } catch {
    // localStorage can be unavailable in privacy modes.
  }
  return window.matchMedia("(prefers-color-scheme: light)").matches ? "light" : "dark";
}

const ThemeContext = createContext<{ theme: Theme; toggle: () => void } | null>(null);

export function ThemeProvider({ children }: { children: ReactNode }) {
  const [theme, setTheme] = useState<Theme>(initialTheme);

  useEffect(() => {
    document.documentElement.dataset.theme = theme;
    try {
      localStorage.setItem("ivitals-theme", theme);
    } catch {
      // Ignore storage failures; the in-memory theme still applies.
    }
  }, [theme]);

  const value = {
    theme,
    toggle: () => setTheme((current) => (current === "dark" ? "light" : "dark")),
  };

  return <ThemeContext.Provider value={value}>{children}</ThemeContext.Provider>;
}

export function useTheme(): { theme: Theme; toggle: () => void } {
  const context = useContext(ThemeContext);
  if (!context) throw new Error("useTheme must be used inside ThemeProvider");
  return context;
}
