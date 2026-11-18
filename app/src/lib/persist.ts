// Settings remembered between runs, in the webview's local storage. Reads fall back to the
// default if nothing is stored, or what's stored doesn't parse.

const PREFIX = "hj.";

export function load<T>(key: string, fallback: T): T {
  try {
    const text = localStorage.getItem(PREFIX + key);
    return text === null ? fallback : (JSON.parse(text) as T);
  } catch {
    return fallback;
  }
}

export function save(key: string, value: unknown) {
  try {
    localStorage.setItem(PREFIX + key, JSON.stringify(value));
  } catch {
    // Storage full or unavailable: it just won't be remembered.
  }
}
