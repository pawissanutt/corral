use super::*;

fn labels(elements: &[Element]) -> Vec<&str> {
    elements.iter().map(|e| e.label.as_str()).collect()
}

/// A flowchart's shapes, chains, `&` groups and the arrows' two ways of carrying words must all
/// land on the boxes and arrows they name, or the drawing misleads.
#[test]
fn a_flowchart_reads_its_shapes_chains_and_arrow_words() {
    let diagram = parse(
        "flowchart LR\n  %% a comment\n  A[Brief] --> B{Gate passes?}\n  B -->|yes| C((Land)) & D[(Store)]\n  B -- no --> A; C -.-> E>Notify]\n  classDef hot fill:#f00\n",
    )
    .unwrap();
    assert_eq!(diagram.kind, RecipeLayout::Graph);
    assert_eq!(
        labels(&diagram.nodes),
        ["Brief", "Gate passes?", "Land", "Store", "Notify"]
    );
    let edges: Vec<_> = diagram
        .edges
        .iter()
        .map(|e| (e.ends.unwrap(), e.label.as_str()))
        .collect();
    assert_eq!(
        edges,
        [
            ((0, 1), ""),
            ((1, 2), "yes"),
            ((1, 3), "yes"),
            ((1, 0), "no"),
            ((2, 4), ""),
        ]
    );
}

/// Statements may follow the header on its line, and a node's style class is not its name.
#[test]
fn header_statements_and_style_classes_are_read() {
    let diagram = parse("flowchart TD; A:::hot --> B:::hot-path\n  B:::hot==>C\n").unwrap();
    assert_eq!(labels(&diagram.nodes), ["A", "B", "C"]);
    assert_eq!(diagram.edges.len(), 2);
}

/// Mermaid would refuse these, or draw them other than as one-way arrows: the app must show
/// the source rather than a picture that says something else.
#[test]
fn invalid_headers_and_links_other_than_one_way_arrows_fall_back() {
    for source in [
        "flowchart NOT_A_DIRECTION\nA --> B\n",
        "flowchart LR extra\nA --> B\n",
        "graph TD\n  direction SIDEWAYS\n  A --> B\n",
        "stateDiagram-v2 LR\n  [*] --> A\n",
        "flowchart LR\n  A --- B\n",
        "flowchart LR\n  A -.- B\n",
        "flowchart LR\n  A <--> B\n",
        "flowchart LR\n  A --o B\n",
        "flowchart LR\n  A --x B\n",
        "flowchart LR\n  A ~~~ B\n",
        "flowchart LR\n  A -- words --- B\n",
        "sequenceDiagram\n  A->B: no head\n",
        "sequenceDiagram\n  A-xB: lost\n",
        "sequenceDiagram\n  A<<->>B: both\n",
        "sequenceDiagram\nalt authorized\nA->>B: Grant\nelse denied\nA->>B: Reject\nend\n",
        "sequenceDiagram\nalt authorized\nA->>B: Grant\n",
        "sequenceDiagram\nloop every page\nA->>B: read\nend\n",
        "flowchart TD\nA[Box]] --> B\n",
        "flowchart TD\nA[(db]] --> B\n",
        "flowchart TD\nsubgraph S\nA --> B\nend\n",
        "stateDiagram-v2\nstate Busy {\n[*] --> Work\n}\n",
        "stateDiagram-v2\nstate pick <<choice>>\n[*] --> pick\n",
    ] {
        assert!(parse(source).is_err(), "{source:?} is drawn");
    }
    let drawn = parse("flowchart LR\n  A -- yes --> B -. maybe .-> C == sure ==> D\n").unwrap();
    let words: Vec<_> = drawn.edges.iter().map(|e| e.label.as_str()).collect();
    assert_eq!(words, ["yes", "maybe", "sure"]);
    let shaped =
        parse("flowchart LR\n  A[Run (fast)] --> B[(db)] --> C((x)) --> D[/in\\]\n").unwrap();
    assert_eq!(labels(&shaped.nodes), ["Run (fast)", "db", "x", "in"]);
}

#[test]
fn state_and_sequence_diagrams_read_their_ends_and_messages() {
    let states = parse(
        "stateDiagram-v2\n  [*] --> Queued\n  state \"Under review\" as Review\n  Queued --> Review : submit\n  note right of Review\n    waits\n  end note\n  Review --> [*]\n",
    )
    .unwrap();
    assert_eq!(
        labels(&states.nodes),
        ["Start", "Queued", "Under review", "End"]
    );
    assert_eq!(states.edges[1].label, "submit");

    let sequence = parse(
        "sequenceDiagram\n  participant A as App\n  actor O\n  A->>+corrald: read page\n  Note over A: bounded\n  corrald-->>-A: bytes\n",
    )
    .unwrap();
    assert_eq!(sequence.kind, RecipeLayout::OrderedLanes);
    assert_eq!(labels(&sequence.nodes), ["App", "O", "corrald"]);
    assert_eq!(sequence.edges[1].ends, Some((2, 0)));
}

/// What the app cannot read must come back as an error, so the view shows the source instead of
/// a wrong or empty picture.
#[test]
fn unreadable_diagrams_are_errors_not_pictures() {
    assert!(parse("pie title Pets\n  \"Dogs\" : 3\n").is_err());
    assert!(parse("flowchart TD\n  A[open --> B\n").is_err());
    assert!(parse("flowchart TD\n").is_err());
    assert!(parse("sequenceDiagram\n  A says hi\n").is_err());
    assert!(parse("sequenceDiagram\n  participa\u{e9}\u{e9}\n").is_err());
    let many: String = (0..=MAX_NODES).map(|n| format!("N{n}\n")).collect();
    assert!(parse(&format!("graph TD\n{many}")).is_err());
    assert!(parse(&"x".repeat(MAX_SOURCE + 1)).is_err());
}

/// An ER diagram draws on the same canvas: entities with their attributes' names, and each
/// relationship from the entity written first, its words carrying both ends' cardinality so the
/// arrow does not drop what the crow's feet say. What it cannot read falls back to the source.
#[test]
fn an_er_diagram_reads_entities_attributes_and_cardinality() {
    let diagram = parse(
        "erDiagram\n  CUSTOMER ||--o{ ORDER : places\n  CUSTOMER {\n    string name PK\n    string email \"contact\"\n  }\n  ORDER ||--|{ LINE-ITEM : contains\n  p[\"Delivery address\"] }o..|| CUSTOMER : \"ships to\"\n  WAREHOUSE\n",
    )
    .unwrap();
    assert_eq!(diagram.kind, RecipeLayout::Graph);
    assert_eq!(
        labels(&diagram.nodes),
        [
            "CUSTOMER: name, email",
            "ORDER",
            "LINE-ITEM",
            "Delivery address",
            "WAREHOUSE"
        ]
    );
    let edges: Vec<_> = diagram
        .edges
        .iter()
        .map(|e| (e.ends.unwrap(), e.label.as_str()))
        .collect();
    assert_eq!(
        edges,
        [
            ((0, 1), "places (exactly one to zero or more)"),
            ((1, 2), "contains (exactly one to one or more)"),
            ((3, 0), "ships to (zero or more to exactly one)"),
        ]
    );
    assert_eq!(diagram.edges[2].detail.as_deref(), Some("non-identifying"));
    // A second block adds to the first, and a later alias keeps the attributes.
    let merged = parse(
        "erDiagram\n  c {\n    string a PK, FK\n  }\n  c {\n    int b\n  }\n  c[\"Customer\"]\n",
    )
    .unwrap();
    assert_eq!(labels(&merged.nodes), ["Customer: a, b"]);
    for source in [
        "erDiagram\n  CUSTOMER ||--o{ ORDER\n",
        "erDiagram\n  CUSTOMER places ORDER\n",
        "erDiagram\n  CUSTOMER {\n    string name\n",
        "erDiagram\n  CUSTOMER only one to zero or more ORDER : places\n",
        "erDiagram\n  CUSTOMER {\n    string name }\n  ORDER {\n    int id\n  }\n",
        "erDiagram\n  CUSTOMER {\n    string a string b\n  }\n",
    ] {
        assert!(parse(source).is_err(), "{source:?} is drawn");
    }
}
