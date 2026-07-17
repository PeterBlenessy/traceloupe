import { createContext, useContext, useState } from "react";

/**
 * Per-view cache key for custom-scheme media URLs, defeating a WKWebView quirk.
 *
 * Custom-scheme media (`traceloupe-media://`, `traceloupe-attachment://`) is
 * served by a WKWebView URI-scheme handler. When an `<img>` is torn down as a
 * view unmounts (navigating Messages → Photos), WebKit cancels its scheme task
 * and caches that URL as *failed* in the content process. Re-mounting an element
 * with the **same** URL then serves the cached failure instead of re-invoking
 * the handler, so the image renders broken until a full reload or a switch to
 * different URLs. Appending a key that changes per view mount (`&k=<n>`) makes
 * the remounted view request fresh URLs, so the handler always runs.
 *
 * The key is deliberately scoped to the *view*, not the individual image
 * component: it must change when the whole view remounts (the case that
 * triggers the bug) but stay stable while that view is alive, so scrolling a
 * virtualized list — which mounts/unmounts rows constantly — keeps requesting
 * identical URLs and lets WebKit's own cache serve repeats instead of
 * re-invoking the handler (and re-opening the cache DB) for every row that
 * scrolls back into view. Wrap a view's subtree in <MediaCacheKeyBoundary>; the
 * image components below it read the shared key via useMediaCacheKey().
 */
let counter = 0;

const MediaCacheKeyContext = createContext<number | null>(null);

/** Establishes a fresh cache key for its subtree, stable for this mount. */
export function MediaCacheKeyBoundary({ children }: { children: React.ReactNode }) {
  const key = useState(() => (counter += 1))[0];
  return (
    <MediaCacheKeyContext.Provider value={key}>{children}</MediaCacheKeyContext.Provider>
  );
}

/** The current view's cache key. Falls back to a per-mount key when rendered
 *  outside a boundary (so a stray consumer still dodges the cache quirk). */
export function useMediaCacheKey(): number {
  const ctx = useContext(MediaCacheKeyContext);
  const fallback = useState(() => (counter += 1))[0];
  return ctx ?? fallback;
}
