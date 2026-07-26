# Security Policy

## Supported Versions

Security fixes are issued for the latest `3.0.x` release only. Older lines do
not receive backports — upgrade to the current patch release.

| Version | Supported |
|---------|-----------|
| 3.0.x   | Yes       |
| < 3.0   | No        |

This policy covers all three published artifacts, which share a version number:

- [`dataflow-rs`](https://crates.io/crates/dataflow-rs) (crates.io)
- [`@goplasmatic/dataflow-wasm`](https://www.npmjs.com/package/@goplasmatic/dataflow-wasm) (npm)
- [`@goplasmatic/dataflow-ui`](https://www.npmjs.com/package/@goplasmatic/dataflow-ui) (npm)

## Reporting a Vulnerability

**Please do not open a public issue for security reports.**

Use GitHub's private vulnerability reporting:
[Report a vulnerability](https://github.com/GoPlasmatic/dataflow-rs/security/advisories/new).
This opens a private thread visible only to the maintainers.

If you cannot use GitHub advisories, email **shankar@goplasmatic.io** with
`[SECURITY] dataflow-rs` in the subject line.

Please include:

- The affected version and artifact (crate, wasm, or UI)
- A description of the issue and its impact
- Steps to reproduce, ideally a minimal workflow JSON or code sample
- Any known mitigations

### What to expect

- **Acknowledgement** within 3 business days
- **Initial assessment** — whether we can reproduce it, and a severity call —
  within 10 business days
- **Fix and release**: we aim to ship a patch release within 30 days of a
  confirmed report, sooner for high-severity issues
- **Credit** in the release notes and advisory, unless you prefer otherwise

We will coordinate disclosure timing with you and will not publish details
before a fix is available.

## Scope

In scope:

- Memory-safety or panic-based denial of service in the engine, reachable from
  workflow JSON, message payloads, or the wasm bindings
- JSONLogic evaluation that escapes its intended sandbox (reading or writing
  outside the `data` / `metadata` / `temp_data` context)
- Unbounded resource consumption triggered by attacker-controlled input, such as
  parser input in `parse_json` / `parse_xml`
- Vulnerabilities in the published npm packages, including the bundled wasm

Out of scope:

- Vulnerabilities in dependencies with an existing RUSTSEC advisory — these are
  tracked by the `cargo-deny` job (see [`deny.toml`](deny.toml)). Open a normal
  issue if you spot one we have missed.
- Behaviour of custom `AsyncFunctionHandler` implementations you supply. The
  engine executes registered handlers by design; treat workflow definitions as
  trusted configuration.
- Resource exhaustion from workflows you authored yourself. Workflow JSON is
  configuration, not untrusted input; if you accept workflow definitions from
  untrusted parties, you must validate and sandbox them yourself.

## Trust Model

Dataflow-rs treats **workflow definitions as trusted configuration** and
**message payloads as untrusted data**. Conditions and mappings are JSONLogic
expressions with no filesystem, network, or process access, evaluated only
against a message's own context. Reports that depend on an attacker being able
to supply arbitrary workflow JSON fall outside this model, but we are still
interested in hearing about them — say so in your report and we will assess it.
