const NOT_SOURCE: &str = r#"#[derive(Serialize)] struct Fake;"#;
mod wire {
    #[derive(::serde::Deserialize)]
    struct Address {
        zip: String,
    }

    #[derive(serde::Serialize)]
    struct Customer {
        address: Option< /* trivia */ Address >,
    }

    #[derive(Deserialize)]
    enum Lookup {
        Found(Customer),
        Missing,
    }
}
