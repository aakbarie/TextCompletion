# Scriblet Phase 2: Native Work Surface

## Product thesis

Scriblet evolves from a system-wide text expander into a keyboard-first, local-first work surface for humans and language models. The durable artifact is open, machine-readable text. Word, PDF, HTML and presentation formats are exports, not the source of truth.

The existing text-expansion engine remains a first-class capability. Phase 2 adds a native note editor without breaking global expansion.

## Design principles

1. **Writing first.** Opening Scriblet should allow immediate writing. AI is not a destination or mandatory sidebar.
2. **Open canonical format.** Markdown is the initial durable format. YAML front matter carries typed metadata. No proprietary document blob is required to recover the work.
3. **Keyboard is the command surface.** Preserve WordStar/WordPerfect speed with discoverable command chords and a command palette.
4. **Completion, not prompting.** The first AI interaction is inline next-sentence completion. The human remains in the writing loop and explicitly accepts generated text.
5. **Local first.** Notes and history work without a server. Model providers are adapters; a local model must be supported.
6. **Structure without form burden.** Humans write prose. Scriblet can infer candidate structure, but machine inference never silently rewrites canonical user text.
7. **Collaboration is architectural.** The document model must leave room for real-time multi-user editing, comments, suggestions, provenance and agents. Do not bake collaboration into Markdown itself.
8. **Human and machine provenance.** AI proposals, human acceptance and later edits must be distinguishable in the event model.

## Phase 2A: ship the native writer

The first implementation should be deliberately small:

- Notes screen alongside the existing Snippet Library.
- Create, open, rename, save and delete `.md` notes.
- Autosave with crash-safe atomic writes.
- Markdown source remains canonical on disk.
- YAML front matter supported but optional.
- Recent notes and full-text local search.
- Keyboard-first navigation and command palette.
- Inline completion provider interface with a deterministic mock provider for tests.
- Ghost-text completion at the caret.
- `Tab` accepts the proposed completion; `Esc` rejects it.
- Completion is never written to disk until accepted.
- Local event log records note edits and completion proposal/accept/reject events without recording hidden prompt material.

### Initial keyboard grammar

| Input | Action |
|---|---|
| `Cmd/Ctrl+N` | New note |
| `Cmd/Ctrl+P` | Open command palette / quick open |
| `Cmd/Ctrl+K` | Begin semantic command chord |
| `Tab` | Accept visible completion |
| `Esc` | Reject visible completion / close transient UI |
| `Cmd/Ctrl+K`, `C` | Request completion |
| `Cmd/Ctrl+K`, `R` | Rewrite selected text (provider capability; later in 2A) |
| `Cmd/Ctrl+K`, `S` | Summarize selection (provider capability; later in 2A) |

Commands must be discoverable. `Cmd/Ctrl+K` opens a small palette near the caret showing available semantic operations and their keys.

## Native note format

A note is valid Markdown with optional YAML front matter:

```markdown
---
type: note
schema: scriblet/note/v1
created: 2026-09-13T12:00:00-07:00
updated: 2026-09-13T12:10:00-07:00
---

# Title

Start writing.
```

Phase 2A must not require front matter for an existing Markdown file. Scriblet should open ordinary `.md` files without conversion.

## Internal architecture

Keep durable content separate from operational state:

```text
workspace/
  notes/
    example.md
  attachments/
  .scriblet/
    workspace.json
    events.sqlite
    index.sqlite
```

- `notes/*.md`: user-owned canonical content.
- `workspace.json`: non-content workspace preferences and schema version.
- `events.sqlite`: append-only semantic event/provenance log.
- `index.sqlite`: disposable search/index state; rebuildable from Markdown.

No collaboration metadata, embeddings or model traces belong in the Markdown unless the user explicitly writes them there.

### Rust module boundaries

Add modules without coupling them to the existing expansion runtime:

```text
src/workspace.rs       workspace discovery, paths, atomic note IO
src/note.rs            note identity, Markdown/front-matter model
src/note_index.rs      local search index
src/completion.rs      provider trait, request/candidate types, cancellation
src/provenance.rs      append-only semantic event store
src/commands.rs        keyboard/semantic command registry
```

The existing `runtime.rs` remains responsible only for system-wide expansion. Native editor completion must not route through the global keyboard injection path.

## Completion contract

Completion is a cancellable proposal, not a document mutation.

```text
CompletionRequest
  note_id
  text_before_cursor
  text_after_cursor
  document_type
  optional_context

CompletionCandidate
  id
  text
  provider
  model
  created_at
```

Rules:

- Typing after a proposal cancels it.
- Moving the caret cancels it.
- Switching notes cancels it.
- `Tab` accepts only the currently visible candidate.
- `Esc` rejects it.
- Providers cannot directly mutate the note.
- Provider/network work never blocks the editor thread.
- Passwords, secrets and unrelated application keystrokes are never completion context.

## Provider strategy

Start with interfaces, not a vendor dependency:

```text
CompletionProvider
  complete(request) -> candidate
  cancel(request_id)
  capabilities()
```

Initial implementations:

1. `MockCompletionProvider` for deterministic tests.
2. Local HTTP provider for Ollama-compatible endpoints.
3. Additional providers later behind explicit configuration.

A fast small model can serve phrase/sentence completion while larger models handle explicit semantic commands. Routing belongs above the provider adapter so the editor does not care which model is used.

## Provenance model

The event store records meaningful operations rather than every keystroke:

- `note.created`
- `note.renamed`
- `note.saved`
- `completion.proposed`
- `completion.accepted`
- `completion.rejected`
- `command.executed`

For generated text, record provider/model and a hash of the accepted content. Do not store raw hidden prompts by default.

## Phase 2B: structured work objects

After the writer is stable:

- typed blocks (`decision`, `task`, `evidence`, `claim`, `citation`)
- semantic transformations on selection/current block
- backlinks and local knowledge graph
- attachments and citations
- export adapters for DOCX, PDF and HTML
- document-type schemas and templates
- semantic diff of typed objects

Markdown remains readable when Scriblet-specific blocks are present.

## Phase 2C: collaboration

Collaboration is a separate state layer, not a replacement file format:

- CRDT-backed live text state
- presence and cursors
- comments and suggestions
- named human and agent identities
- semantic change events
- review/accept/reject workflow
- offline edits and reconciliation
- self-hostable sync service

The CRDT state materializes to canonical Markdown. A workspace can remain entirely local and never enable collaboration.

## Phase 2D: agents at the cursor

Agents participate as attributable collaborators rather than chat windows. Examples include evidence review, policy review, meeting action extraction and structured-work maintenance. Agent changes are proposals by default and use the same review/provenance model as human suggestions.

## Non-goals for Phase 2A

Do not build rich Office-format fidelity, a Notion clone, a graph visualization, cloud accounts, CRDT synchronization, autonomous agents, or a general chat sidebar yet. Those would obscure the core product test: **is a keyboard-first Markdown writer with near-zero-friction sentence completion materially better to write in?**

## Acceptance criteria for the first Phase 2 build

A clean install on Windows, Intel macOS and Apple Silicon macOS can create/open a Markdown note, survive restart without data loss, search notes, invoke a deterministic completion, display it as non-persistent ghost text, accept with Tab or reject with Esc, and continue to run existing system-wide snippet expansion without regression.

The next release after the v0.5.x stabilization line should begin the Phase 2 development line rather than mixing these architectural changes into the hotfix series.