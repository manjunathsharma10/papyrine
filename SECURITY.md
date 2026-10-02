# Security policy

## Reporting a vulnerability

Please report suspected vulnerabilities privately through GitHub:
**Security tab -> Report a vulnerability** on
<https://github.com/manjunathsharma10/papyrine/security/advisories/new>.
Do not open a public issue for a security problem.

Include the Papyrine version and platform, a description of the impact, and, if you can, a
minimal file that reproduces it. Please send a PDF only as an attachment to the private report,
and do not include documents that contain other people's personal data.

What to expect: an acknowledgement within 7 days, an assessment within 14 days, and a fix or a
mitigation plan for confirmed issues. We credit reporters in the release notes unless you ask us not
to. Papyrine is pre-1.0, so only the latest release receives fixes.

## Threat model (summary)

The main threat is a **malicious PDF** that exploits a memory-safety bug in qpdf, PDFium or a codec
(C/C++) to read or write the user's files or to exfiltrate data. The design assumes such bugs will
exist and limits what they can reach (ARCHITECTURE section 8).

| Control | What it does |
|---|---|
| Least privilege | The engine, the renderer and component helpers run as sandboxed child processes. Only the host process has file, dialog and network access. |
| macOS | Seatbelt profile applied at child start: deny by default, no network, no writes outside a per-child temp dir. |
| Linux | `landlock` file-system allowlist, `seccomp-bpf` syscall allowlist (no `socket`, `connect`, `execve`), `PR_SET_NO_NEW_PRIVS`. |
| Windows | Restricted token, low integrity level and a Job object (no child processes, memory cap, UI restrictions). |
| File access | Brokered. Children receive already-open read-only handles or bytes. Writes go through the host's atomic replace of bytes the engine produced. |
| PDF JavaScript | Off by default. Available only through the optional Form Scripts component in a sandboxed helper with CPU and memory limits and no network or file-system API. |
| Actions | Launch, URI, GoToR and embedded-file-open actions are handed to the host, which always prompts and shows the target. |
| Secrets | Passwords and keys live in `zeroize`d buffers, are redacted in logs, and are stored in the OS keychain only if the user opts in. |
| Components | Optional components are verified by signature before install and by hash before each launch, and run only as sandboxed children. They are never loaded into the host as libraries. |
| Network | No network access by default. The update check and component downloads are opt-in and user-initiated. |

### Supply chain

- `cargo-deny` (licenses, bans, advisories, sources), `cargo-audit` and `pnpm audit` run in CI
  (advisories nightly). Lockfiles are committed. Vendored native sources are pinned by SHA-256
  (`third_party/native.toml`).
- The bundle-inspection gate (`tools/inspect-bundle`) fails a release artifact that links an
  unlisted library or statically includes GnuTLS/OpenSSL symbols. qpdf is built with its native
  crypto provider only (the CMake configuration is checked in CI).
- Fuzzing: `cargo-fuzz` targets for the content-stream parser, qpdf loading, the serializer, codecs
  and font loaders run for 10 minutes per pull request and for one hour per target nightly.

### Out of scope

- Attacks that need a malicious local user with the same privileges as the app.
- Bugs in the operating system's web view that do not involve Papyrine's own code.
- Content that is dangerous only when the user explicitly confirms an action prompt.
