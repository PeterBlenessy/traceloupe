# Third-Party Notices

TraceLoupe (Apache-2.0) is built on open-source software under permissive
licenses (MIT, Apache-2.0, BSD, ISC, and public domain). This file records the
notable components; it is not an exhaustive list of transitive dependencies. A
complete machine-generated inventory can be produced with `cargo about` (Rust)
and `license-checker` (npm).

## Parsing engine (run as a separate subprocess — not linked into TraceLoupe)

- **iLEAPP** — MIT — Copyright Alexis Brignoni and contributors —
  https://github.com/abrignoni/iLEAPP
  TraceLoupe downloads and runs a pinned iLEAPP build as a headless subprocess.
  iLEAPP bundles Python libraries under permissive licenses, including
  **pandas** (BSD-3-Clause), **NumPy** (BSD-3-Clause), and **Pillow** (MIT-CMU / HPND).

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
