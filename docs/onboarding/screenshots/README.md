# Onboarding screenshots

The first-use guide and technical-friend checklist reference
several annotated illustrations of the Obsidian plugin's
user-facing surfaces. The MVP ships **SVG-mode mockups** here
rather than PNG screenshots so the repo stays editor-
diffable and the illustrations don't drift silently when the
plugin layout changes.

Each illustration shows:

| File | What it depicts |
|---|---|
| [`daily-summary-panel.svg`](daily-summary-panel.svg) | The right-sidebar panel with proposals, questions, drift flags. Accept/Reject buttons annotated. |
| [`projection-navigation.svg`](projection-navigation.svg) | Obsidian's file explorer showing `contacts/by-name/S/` with `Sara_Chen.md` rendered. The vault root is `~/.ffs/` (substrate-is-vault per ADR-022), so these paths appear directly under the vault. |
| [`entity-search.svg`](entity-search.svg) | The entity-search modal with a 200ms-debounced query in flight. |
| [`federation-bridges.svg`](federation-bridges.svg) | The federation panel listing two active bridges. |

When the plugin's UI is finalized, these mockups will be
replaced with annotated PNG screenshots captured from a real
running plugin. Until then the mockups serve the same purpose:
they show the user-visible surface every onboarding doc
references.
