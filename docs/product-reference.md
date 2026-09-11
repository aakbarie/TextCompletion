# Product references

Scriblet is intentionally a small native binary first. These products are reference points, not implementation templates.

## Breevy / aBreevy8

Useful reference for the core desktop expectation:
- global abbreviation expansion
- multiline snippets
- variables such as date/time and clipboard
- keyboard actions and cursor placement
- low-friction desktop utility behavior

## PhraseExpress

Useful reference for the eventual enterprise model:
- personal and shared phrase libraries
- Microsoft SQL Server-backed shared content
- offline/local execution
- permissions and centrally managed libraries
- version history and usage/audit capabilities

Scriblet should retain a local SQLite execution store even after SQL Server is introduced. SQL Server should distribute governed/shared content; expansion should not depend on network round trips.

## Key2Scribe

Useful reference for professional workflow ergonomics:
- variable prompts and validation
- audit-ready expansion history
- team libraries and role-based access
- explicit handling/warnings for potentially sensitive patterns
- healthcare-oriented consistency use cases

These are later-stage capabilities. They should shape the schema now but should not inflate v1.

## Scope boundary

The desktop app stays deliberately narrow:
1. Windows and macOS native binary
2. fast global text expansion
3. snippet CRUD with categories and favorites
4. multiline + Unicode text and template variables
5. SQLite persistence with a read-only enterprise cache
6. enable/disable and pause/resume
7. clean modern UI
8. import/export
9. start at login and tray behavior

No AI, cloud account system, analytics platform, or server dependency in the typing path.
