export { cn } from "cn"

/** How the platform writes the shortcut modifier: "⌘" on macOS, "Ctrl+" elsewhere. */
export const MOD = typeof navigator !== "undefined" && /Mac|iPhone|iPad/.test(navigator.userAgent) ? "⌘" : "Ctrl+";
