import { Component, type ReactNode } from "react";

import { Button } from "@/components/ui/button";

/**
 * Keeps a rendering bug in one part of the window from blanking the rest, so the header (and HALT)
 * stays usable while the machine runs.
 */
export class ErrorBoundary extends Component<{ children: ReactNode }, { error: Error | null }> {
  state = { error: null as Error | null };

  static getDerivedStateFromError(error: Error) {
    return { error };
  }

  render() {
    const { error } = this.state;
    if (!error) return this.props.children;
    return (
      <div className="flex h-full flex-col items-center justify-center gap-3 p-6 text-sm">
        <div className="font-medium text-red-400">This part of the window crashed.</div>
        <pre className="max-w-xl whitespace-pre-wrap text-xs text-muted-foreground">{error.message}</pre>
        <Button size="sm" variant="outline" onClick={() => this.setState({ error: null })}>
          Try again
        </Button>
      </div>
    );
  }
}
