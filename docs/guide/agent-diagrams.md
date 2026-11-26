# Publishing diagrams as task artifacts

Any agent harness that can run a shell (corral-pi, Claude Code, Codex, a plain terminal) can
publish a diagram the owner reads in the Corral app. There is no harness-specific protocol: the
diagram is an ordinary Markdown artifact recorded with `corral-next artifact add`, and the app
draws its Mermaid fences itself. No HTML, JavaScript or external renderer is involved.

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

The app reads a Markdown artifact from its `.md`/`.markdown` title or, for a title that is not a
file name with another extension, from its own structure: a first line that is a heading, or a
closed code fence or table within its first 64 KiB. A descriptively titled report therefore
renders as Markdown, not as its source, while `deploy.sh` or `notes.txt` stays text.

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
