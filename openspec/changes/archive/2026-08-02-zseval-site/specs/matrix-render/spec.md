## RENAMED Requirements

- FROM: `### Requirement: Two renderers, shared with run`
- TO: `### Requirement: Renderers over one model, shared across subcommands`

## MODIFIED Requirements

### Requirement: Renderers over one model, shared across subcommands
`matrix` SHALL provide a fixed-width terminal renderer, a markdown renderer (for records, no width limit), and an HTML renderer, all over the same `Matrix` model. The `run` subcommand SHALL reuse the fixed-width renderer function for its stderr table, and the `site` subcommand SHALL reuse the HTML one for its results section.

`matrix`'s own command-line surface is unchanged by the third renderer: `matrix` SHALL still emit fixed-width by default, markdown under `--markdown`, and JSON under `--json`, and SHALL NOT gain an HTML flag. The HTML renderer is reachable only through `site`.

Renderers live together in the same module rather than beside the subcommands that use them, because the cell, hole, footer-figure and row-mark formatting is shared and private. A renderer outside the module would have to restate that formatting, and three independent answers to "how is a hole written" drift apart on the first change.

#### Scenario: Markdown on request, fixed-width by default
- **WHEN** `matrix` is invoked with `--markdown`
- **THEN** it emits markdown; without it, it emits the fixed-width table

#### Scenario: `matrix` gains no HTML flag
- **WHEN** `matrix` is invoked with any combination of its own flags
- **THEN** it never emits HTML; the HTML renderer is reached only by `site`

#### Scenario: Every renderer reads the same model
- **WHEN** the same report is rendered as fixed-width, markdown, and HTML
- **THEN** all three report the same cells, the same holes, the same per-kind grouping, and the same footer figures, because none of them recomputes the model
