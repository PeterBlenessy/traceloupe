#!/usr/bin/env swift
//
// What the platform says text should measure — AppKit's metrics and text styles,
// and what a WKWebView (our actual renderer) resolves the `-apple-system-*` font
// keywords to.
//
// Why this exists: our type ramp is a WEB ramp (12/14/16/18/20/24 px) while
// macOS's is 10/11/12/13/15/17/22/26. They do not line up — our body text sits a
// step above the system's. This prints both sides so the mapping is a
// measurement rather than a guess.
//
// It also answers whether macOS's per-app Text Size setting (System Settings →
// Accessibility → Display → Text Size; stored as `FontSizeCategory` in
// com.apple.universalaccess) reaches these APIs. Run it, change the setting, run
// it again: if the numbers move, adopting the text styles gets us system
// legibility for free. If they don't, they are just constants we already have.
//
//   swift scripts/font-probe.swift
//
import AppKit
import WebKit

print("current Text Size category:")
let ua = UserDefaults(suiteName: "com.apple.universalaccess")
let cat = (ua?.dictionary(forKey: "FontSizeCategory")?["global"] as? String) ?? "(unset)"
print("  global = \(cat)\n")

print("AppKit metrics:")
print("  systemFontSize      \(NSFont.systemFontSize)")
print("  smallSystemFontSize \(NSFont.smallSystemFontSize)")
print("  labelFontSize       \(NSFont.labelFontSize)\n")

print("AppKit text styles:")
let styles: [(String, NSFont.TextStyle)] = [
    ("largeTitle", .largeTitle), ("title1", .title1), ("title2", .title2),
    ("title3", .title3), ("headline", .headline), ("subheadline", .subheadline),
    ("body", .body), ("callout", .callout), ("footnote", .footnote),
    ("caption1", .caption1), ("caption2", .caption2),
]
for (name, style) in styles {
    let f = NSFont.preferredFont(forTextStyle: style)
    print("  \(name.padding(toLength: 12, withPad: " ", startingAt: 0)) \(String(format: "%5.1f", f.pointSize))pt  \(f.fontName)")
}

// The same question asked of the renderer we actually ship, since that is what
// would consume these — CSS `font: -apple-system-body` needs no native bridge.
final class Probe: NSObject, WKNavigationDelegate {
    let done = DispatchSemaphore(value: 0)
    func webView(_ w: WKWebView, didFinish _: WKNavigation!) {
        w.evaluateJavaScript("""
        ['body','headline','subheadline','callout','caption1','caption2'].map(k => {
          const el = document.createElement('div');
          el.style.font = '-apple-system-' + k;
          document.body.appendChild(el);
          const cs = getComputedStyle(el);
          return '  ' + k.padEnd(12) + cs.fontSize.padStart(6) + '  weight ' + cs.fontWeight;
        }).join('\\n')
        """) { r, e in
            print("\nWKWebView `font: -apple-system-*`:")
            print(r as? String ?? "  error: \(String(describing: e))")
            self.done.signal()
        }
    }
}
let probe = Probe()
let wv = WKWebView(frame: .init(x: 0, y: 0, width: 300, height: 300))
wv.navigationDelegate = probe
wv.loadHTMLString("<html><body></body></html>", baseURL: nil)
while probe.done.wait(timeout: .now()) == .timedOut {
    RunLoop.current.run(mode: .default, before: Date().addingTimeInterval(0.05))
}
