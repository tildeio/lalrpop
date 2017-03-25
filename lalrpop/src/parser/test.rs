use parser;
use grammar::parse_tree::GrammarItem;

#[test]
fn match_block() {
    let blocks = vec![
        r#"grammar; match { _ }"#, // Minimal
        r#"grammar; match { _ } else { _ }"#, // Doesn't really make sense, but should be allowed
        r#"grammar; match { "abc" }"#, // Single token
        r#"grammar; match { "abc" => "QUOTED" }"#, // Single token with quoted alias
        r#"grammar; match { "abc" => UNQUOTED }"#, // Single token with unquoted alias
        r#"grammar; match { r"(?i)begin" => BEGIN }"#, // Regex
        r#"grammar; match { "abc", "def" => "DEF", _ } else { "foo" => BAR, r"(?i)begin" => BEGIN, _ }"#, // Complex
        r#"grammar; match { "abc" } else { "def" } else { _ }"#, // Multi-chain
    ];

    for block in blocks {
        let parsed = parser::parse_grammar(&block).expect(format!("Invalid grammar; grammar={}", block).as_str());
        let first_item = parsed.items.first().expect("has item");
        match *first_item {
            GrammarItem::MatchToken(_) => (), // OK
            _ => panic!("expected MatchToken, but was {:?}", first_item)
        }
    }
}

#[test]
fn match_complex() {
    let parsed = parser::parse_grammar(r#"
        grammar;
        match {
            r"(?i)begin" => "BEGIN",
            r"(?i)end" => "END",
        } else {
            r"[a-zA-Z_][a-zA-Z0-9_]*" => IDENTIFIER,
        } else {
            "other",
            _
        }
"#).unwrap();
}
