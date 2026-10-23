# Security Policy

## Supported Versions

We provide security updates for the latest `4.x` minor release. Older releases
do not receive backports—please upgrade to the latest release.

| Version              | Supported |
|----------------------|-----------|
| Latest 4.x minor     | Yes       |
| < 4.0                | No        |

This policy covers all four published packages, which share a version number:

- [`dataflow-rs`](https://crates.io/crates/dataflow-rs) (crates.io)
- [`dataflow-rs-derive`](https://crates.io/crates/dataflow-rs-derive) (crates.io)
- [`@goplasmatic/dataflow-wasm`](https://www.npmjs.com/package/@goplasmatic/dataflow-wasm) (npm)
- [`@goplasmatic/dataflow-ui`](https://www.npmjs.com/package/@goplasmatic/dataflow-ui) (npm)

## Reporting a Vulnerability

**Please do not open a public GitHub issue for security vulnerabilities.**

Instead, use GitHub's private vulnerability reporting:
[Report a vulnerability](https://github.com/GoPlasmatic/dataflow-rs/security/advisories/new).
This opens a private discussion visible only to the maintainers.

If you are unable to use GitHub advisories, email **shankar@goplasmatic.io**
with `[SECURITY] dataflow-rs` in the subject line.

Please include:

- The affected version and package (`dataflow-rs`, `dataflow-wasm`, or `dataflow-ui`)
- A clear description of the vulnerability and its potential impact
- Steps to reproduce, ideally with a minimal workflow JSON or code snippet
- Any known workarounds or mitigations

### Response Timeline

- **Acknowledgment:** within 3 business days
- **Initial assessment** (reproduction and severity rating): within 10 business days
- **Fix and release:** within 30 days of a confirmed report (sooner for critical/high-severity issues)
- **Credit** in the release notes and security advisory, unless you prefer to remain anonymous

We will coordinate disclosure timing with you and publish the advisory once a
fix is released.

## Scope

**In scope:**

- Memory safety issues or panics (denial of service) in the engine triggered by
  workflow JSON, message payloads, or the WebAssembly bindings
- JSONLogic evaluation escaping its sandbox (reading or writing outside the
  `data`, `metadata`, and `temp_data` context roots or the engine's `Secrets` store)
- Secret values leaking into serialized messages, traces, error records,
  observer events, or logs
- Unbounded memory or CPU consumption triggered by untrusted message payloads
  (for example, in `parse_json` or `parse_xml`)
- Vulnerabilities in the published npm packages, including the bundled WASM binary

**Out of scope:**

- Known vulnerabilities in third-party dependencies that already have a RUSTSEC
  advisory. These are tracked automatically by our `cargo-deny` CI job (see
  [`deny.toml`](deny.toml)). Feel free to open a regular issue if you notice one
  that needs attention.
- Behavior inside custom `Function` implementations provided by your application.
- Resource exhaustion caused by workflow definitions you authored yourself
  (unless you are using `ops_budget` / compilation limits and found a bypass).

## Trust Model

Dataflow-rs treats **workflow definitions as trusted configuration** and
**message payloads as untrusted input**. Conditions and mappings are pure
JSONLogic expressions with no filesystem, network, or process access; they can
only read the message context (`data`, `metadata`, `temp_data`) and the engine's
isolated `Secrets` store (`{"secret": "name"}`).

If you discover an issue that requires an attacker to supply untrusted workflow
JSON (for example, in a multi-tenant rule authoring environment), we still
welcome your report—just note that context in your submission so we can evaluate
it appropriately.
