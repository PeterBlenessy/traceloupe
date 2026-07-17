import { useState } from "react";

/**
 * A monotonically increasing key, one value per component mount.
 *
 * Custom-scheme media (`traceloupe-media://`, `traceloupe-attachment://`) is
 * served by a WKWebView URI-scheme handler. When an `<img>` is removed from the
 * DOM — e.g. navigating away from a view — WebKit cancels its in-flight scheme
 * task and caches that URL as *failed* in the content process. Re-mounting an
 * element with the **same** URL then serves the cached failure instead of
 * re-invoking the handler, so the image renders broken until a full page reload
 * (clears the cache) or a switch to different URLs (a different conversation).
 *
 * Appending this per-mount key (`&k=<n>`) to media URLs makes every mount
 * request a distinct URL, so a remount always re-invokes the handler. The key is
 * stable for the component instance's lifetime (a plain re-render keeps it, so no
 * reload flicker) and changes only when the component mounts afresh. Rendered
 * thumbnails are cached server-side on disk, so the extra requests are cheap.
 */
let counter = 0;

export function useMediaCacheKey(): number {
  return useState(() => ++counter)[0];
}
