# GPT-5.6-terra (medium) implementation goal

Use this as the complete task prompt in a fresh Codex session configured with model `gpt-5.6-terra` and reasoning effort `medium`.

---

Create a proper Codex goal before doing implementation work. Use exactly this objective, without adding a token budget:

> Fully implement Fileporter v1 in this workspace according to `FILEPORTER_SPEC.md`. Completion means a production-quality Tauri 2 application for Windows and macOS with a polished React/TypeScript UI; per-user background/tray lifecycle; direct encrypted LAN discovery, pairing, fan-out transfer, integrity verification, durable resume, safe cross-platform filesystem semantics, configurable receive directory, native Copy/Cut/Move and reveal actions, persistence, diagnostics, tests, CI, packaging configuration, and documentation. Run the application on the current Windows machine and verify the relevant automated and local smoke tests. Continue until every acceptance criterion is implemented or an external credential/hardware prerequisite is explicitly isolated and documented.

Then follow this execution contract:

1. Read `FILEPORTER_SPEC.md` completely and treat it as the source of truth. Inspect all existing workspace files and applicable instructions before editing.
2. Create and maintain a concrete implementation plan matching the milestones in section 23. Keep exactly one step in progress. The plan is tracking, not a substitute for implementation.
3. Work autonomously through the entire goal. Make safe, reversible assumptions that preserve the specification. Ask the user only when a missing choice would materially change the product or when external authorization/credentials/hardware are truly required.
4. Use current stable, mutually compatible Tauri 2, Rust, and frontend dependencies. Commit lockfiles. Check official primary documentation when platform/plugin behavior is version-sensitive.
5. Build vertical slices and test continuously. Do not stop after scaffolding, a design, or a partially mocked UI. Do not leave required behavior behind TODOs or simulated network/filesystem code.
6. Keep all networking, trust, transfer, hashing, filesystem, native clipboard, and durable-state logic in Rust. Keep the webview a narrow typed presentation layer with minimal Tauri capabilities.
7. Preserve no-overwrite, path containment, authenticated trust, bounded memory, and resumability invariants even if they take longer than cosmetic work.
8. Provide a test-only isolated two/three-instance loopback harness so peer transfer, fan-out, failure injection, and resume can be exercised on one development machine.
9. Run formatting, lint, type checking, frontend tests, Rust tests, Clippy with warnings denied, security checks, and a Tauri build. Fix failures caused by the work.
10. Launch Fileporter on the current Windows machine in development or packaged mode and perform a local smoke test. Leave a clear command for relaunch. Do not claim macOS manual verification from Windows; require macOS CI/build evidence and clearly list any physical-Mac manual checks still needing hardware.
11. Maintain `README.md` as an operator/developer handoff and add short ADRs for meaningful departures from the specification.
12. Before completion, audit every acceptance criterion in section 24 and record evidence in a concise `VERIFICATION.md`. If a criterion cannot be met solely because signing credentials or a physical second platform are unavailable, keep the implementation complete, document the exact external step, and do not fabricate a result.
13. Mark the Codex goal complete only after the objective is genuinely achieved and no in-scope implementation or verification work remains. When marking it complete, report the goal tool's final token usage.

Expected handoff:

- Fileporter is implemented, not just designed.
- The Windows app is running or has just been successfully smoke-tested on the current machine.
- Automated checks and build results are summarized with exact commands.
- macOS CI/build status and remaining credential/hardware-only checks are explicit.
- Important files and the exact local launch command are linked in the final response.

---

Model confirmation: official OpenAI documentation lists `medium` as a supported and default reasoning effort for `gpt-5.6-terra`; preserve the requested model and effort.
