---
name: h11r-implementation
description: Implement requested changes in the h11r Rust core, PyO3 binding, Python package, tests, packaging, CI, or project documentation. Use when creating, modifying, fixing, refactoring, replacing, or migrating h11r behavior or repository files. Do not use for read-only audits or factual research.
---

# h11r Implementation

Deliver the smallest complete change that satisfies the request and preserves
the repository boundaries in `AGENTS.md`.

## Workflow

1. Read `AGENTS.md` and every affected file before editing. Inspect callers,
   tests, public exports, type stubs, and configuration that share the changed
   contract.
2. Establish the requested outcome and explicit non-goals. Do not add adjacent
   features, compatibility layers, reports, or tooling without a current need.
3. Trace the owning layer before choosing the fix. Keep protocol state and
   framing in the Rust core, transport/runtime ownership outside the core, and
   Python-facing vocabulary natural for Python.
4. Use `h11r-knowledge` when a decision depends on protocol requirements,
   established tooling behavior, packaging rules, or mature implementation
   practice. Separate source facts from project choices.
5. Implement the smallest coherent design. Update all affected layers required
   for correctness, including Python exports, stubs, and tests when their
   contract changes.
6. Add the smallest check that would fail for a changed behavior. Match evidence
   to the claim: focused tests for behavior, interoperability for real peers,
   fuzzing for hostile input/state exploration, and benchmarks for performance.
7. Run focused checks first, then the broader gate required by `AGENTS.md`.
   Read fresh output before claiming success.
8. Review the final changes with `h11r-audit`. Address blocking findings
   through this workflow and rerun the affected checks.

## Implementation Boundaries

- Preserve the HTTP/1.1-only product boundary. Upgrade and handoff behavior may
  be implemented only as HTTP/1.1 connection semantics, not as another protocol.
- Keep the Rust protocol core independent of PyO3 and network I/O.
- Prefer existing language features and mature dependencies over local
  machinery.
- Replace incorrect pre-release APIs directly. Do not retain aliases, shims, or
  duplicate implementations unless compatibility is explicitly required.
- Do not invent protocol behavior, resource limits, safety claims, or
  performance claims without evidence.
- Do not add tests that only restate constants, types, or implementation
  spelling without protecting observable behavior.

Stop and report the conflict instead of patching around missing authority,
unclear ownership, or an environment that cannot verify the requested result.
