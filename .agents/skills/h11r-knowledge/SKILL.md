---
name: h11r-knowledge
description: Retrieve, verify, and, when explicitly requested, maintain source-based protocol-engineering knowledge in docs/knowledge. Use for factual questions about HTTP/1.1, testing, architecture, Python/Rust boundaries, tooling, dependencies, licenses, or established practice, including stale or conflicting claims. Keep project decisions and code-specific audit conclusions out of the collection.
---

# h11r Knowledge

Use `docs/knowledge/` as a factual reference layer. It supplies evidence and
source maps; it does not decide what h11r should implement.

## Retrieve And Verify

1. Resolve the repository root and read `docs/knowledge/README.md` for evidence
   labels and freshness semantics.
2. Search document metadata before loading topic files:

   ```console
   rg -n '^(title|description|topics|checked_at):' docs/knowledge --glob '*.md'
   ```

3. Read only documents matching the protocol term, tool, claim, or evidence
   class. Follow direct citations when exact current wording or provenance
   matters.
4. Keep these evidence classes distinct:
   - normative source: an applicable RFC or BCP;
   - authoritative registry: current IANA data;
   - official guidance: documentation from the tool or language owner;
   - observed practice: behavior in a named, pinned implementation;
   - methodological synthesis: a conclusion across sources.
5. Verify live primary sources when the answer depends on mutable protocol
   status, registries, tool behavior, package metadata, or current practice.
6. Report unavailable, superseded, materially changed, or conflicting sources.
   Do not silently resolve uncertainty or rewrite the collection during a
   read-only task.

## Maintain The Collection

Modify `docs/knowledge/` only when knowledge maintenance is explicitly
requested.

1. Update the existing owner document; add a topic only for a distinct reusable
   domain.
2. Keep documents factual and independent of h11r code, audits, plans,
   benchmark results, release decisions, and temporary recommendations.
3. Use `title`, `description`, `topics`, and `checked_at` frontmatter on topic
   documents. Change `checked_at` only after reviewing every external claim in
   that document.
4. Cite primary sources beside the supported claims. Pin implementation
   observations to a version or commit.
5. Replace stale claims instead of appending a contradictory account. Do not
   copy remote source archives without an offline or reproducibility need.
6. Update `docs/knowledge/README.md` only when the document set or domain layout
   changes.
7. Verify frontmatter, relative links, absence of personal paths, and the
   separation between source facts, synthesis, and project choices.

Do not create reports, decision logs, generated indexes, or monitoring
machinery for ordinary knowledge work.
