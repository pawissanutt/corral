const NOT_SOURCE: &str = r#"#[derive(Serialize)] struct Fake;"#;
pub mod child;
mod wire {
    #[derive(serde::Serialize)]
    struct Customer {
        address: Option< /* trivia */ crate::child::Address >,
        missing: crate::orphan::Address,
    }

    #[derive(Deserialize)]
    enum Lookup {
        Found(Customer),
        Missing,
    }
}
mod nested { mod src { mod lib; } }
