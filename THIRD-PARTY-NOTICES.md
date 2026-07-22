# Third-Party Notices

TraceLoupe (Apache-2.0) is built on open-source software under permissive
licenses (MIT, Apache-2.0, BSD, ISC, and public domain). This file records the
notable components; it is not an exhaustive list of transitive dependencies. A
complete machine-generated inventory can be produced with `cargo about` (Rust)
and `license-checker` (npm).

## Development reference (not distributed, not run by the app)

- **iLEAPP** — MIT — Copyright Alexis Brignoni and contributors —
  https://github.com/abrignoni/iLEAPP
  Used only during development to cross-check TraceLoupe's original native
  parsers against iLEAPP's output. No iLEAPP code is included in, linked into,
  bundled with, or executed by the app — so there is no distribution of iLEAPP
  or its Python dependencies. This acknowledgment is a courtesy, not a license
  obligation.

## Parser provenance policy

TraceLoupe's data parsers follow the parser-provenance rules in
`docs/architecture.md` §10:

- Native parsers written from reverse-engineered facts ("reference" provenance)
  carry no third-party notice and are not listed here.
- Any parser ported from iLEAPP or another project ("port" provenance) must add
  the upstream copyright line and license text to this file, and mark its source
  header `// provenance: port of <module> — see THIRD-PARTY-NOTICES.md`.

No parsers are currently ported — all are clean-room "reference" implementations,
so there are no port entries.

## Rust (application core)

- **Tauri** and plugins — MIT OR Apache-2.0 — https://tauri.app
- **SQLite** (bundled via `rusqlite`) — Public Domain — https://sqlite.org
- **rusqlite** — MIT
- **RustCrypto** (`aes`, `cbc`, `cipher`, `aes-kw`, `sha1`, `sha2`, `pbkdf2`,
  `crypto-common`, `zeroize`, …) — MIT OR Apache-2.0
- **serde**, **serde_json**, **plist**, **time**, **ureq**, **keyring**, **hex** — MIT OR Apache-2.0

## Web UI

- **React** — MIT — Copyright Meta Platforms, Inc. and affiliates
- **TanStack** Query / Router / Virtual — MIT
- **Radix UI** (shadcn/ui primitives) — MIT
- **Tailwind CSS** — MIT
- **lucide-react** — ISC
- **Vite** — MIT
- **class-variance-authority**, **clsx**, **tailwind-merge** — MIT

Full license texts are available in each component's source repository and
package metadata.
