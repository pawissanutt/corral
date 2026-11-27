# Publishing diagrams as task artifacts

Any agent harness that can run a shell (corral-pi, Claude Code, Codex, a plain terminal) can
publish a diagram the owner reads in the Corral app. There is no harness-specific protocol: the
diagram is an ordinary Markdown artifact recorded with `corral-next artifact add`, and the app
draws its Mermaid fences itself. No HTML, JavaScript or external renderer is involved.

For the separate typed workflow format, see [workflow diagram recipes](workflow-diagram-recipes.md).
The app can inspect evidence for typed recipes; the offline validator checks structure only.
The portable skill at `packages/corral-pi/assets/skills/corral-diagrams/SKILL.md` is bundled
with all skills-enabled Corral Pi roles and can be switched off in **Settings → Configuration → Skills**.
External harnesses can install it as a skill directory.

## Workflow

1. Write one Markdown file. Put each diagram in a fenced block tagged `mermaid`, and keep the
   prose, tables and code you would write anyway around it:

   ````markdown
   # Worker submits → Gate → Review → Land

   ```mermaid
   flowchart LR
     W[Worker submits] --> G{Gate}
     G -->|pass| R[Review] --> L[Land]
     G -->|fail| W
   ```

   ```mermaid
   sequenceDiagram
     Worker->>corrald: submit
     corrald-->>Worker: gate receipt
   ```

   ```mermaid
   erDiagram
     TASK ||--o{ RUN : starts
     RUN ||--o{ ARTIFACT : records
   ```

   ```rust
   fn main() {}
   ```
   ````

2. Attach it to the task (or a project's library) with the existing command, choosing the kind
   that says what it is (`report` for a deliverable, `reference` for background, `evidence` for
   proof):

   ```sh
   corral-next artifact add T-12 .evidence/pipeline.md --kind report --title "Worker submits → Gate → Review → Land"
   ```

   `--title` is optional and may be any human title; it is not how the app decides what the file
   is. `--json` prints the recorded artifact for scripts.

3. Cite the artifact by its reference, `T-12:3` (the task and the artifact's number, as
   `corral-next artifact list T-12` shows it), in your summary, a comment or a later artifact.
   A pressed citation, the task page's artifact list and Activity → Artifacts all open the same
   saved render.

To revise a diagram, attach the edited file again with `--replaces T-12:3`; the earlier artifact
stays readable and points to its successor.

## What the app draws

The app never decides the format from the title (or the media type corrald derives from it).
When `artifact add` attaches a file, the catalog records its format from the file's path: a
`.md`/`.markdown` file is Markdown under any `--title`, so an ordinary note of a heading and
prose renders. Otherwise the artifact's recorded kind decides first (a `diff`, `gate` or `log`
artifact keeps that view), then the content: within its first 64 KiB, a closed code fence, a
table's delimiter row, or a leading heading followed by a list item, link, quote or
`**strong**` text makes it Markdown. Those bytes render the same under `Build pipeline`,
`Build pipeline v1.0rc1` or `pipeline.txt`. Without the file hint a lone leading `# line` is not
enough, so a script's comments stay text. A `mermaid` fence is always a closed fence, so a
diagram report always qualifies; attaching it from a `.md` file is still the surest path.

On desktop and web each `mermaid` fence is drawn on the app's diagram canvas:

| Mermaid kind | Drawn as |
|---|---|
| `flowchart` / `graph` | boxes and one-way arrows, arrow words as labels |
| `stateDiagram-v2` | states and transitions, `[*]` as Start/End |
| `sequenceDiagram` | participants and numbered messages |
| `erDiagram` | entities (with their attributes' names) and relationships labelled with both ends' cardinality |

Other fences keep their language tag and are syntax-highlighted. A Mermaid kind or statement the
canvas cannot represent faithfully (for example `subgraph`, `alt`/`loop` blocks, undirected
links, Gantt charts) shows a short reason and the fence's source instead of a misleading picture,
and the terminal and phone always show the source. Download saves the artifact's exact bytes in
every case.

Diagrams are bounded: at most 16 KiB of source, 80 boxes and 160 arrows per fence. Split a
larger picture into several fences.
