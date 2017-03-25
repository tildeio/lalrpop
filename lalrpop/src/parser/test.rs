use parser;
use grammar::parse_tree::{GrammarItem, MatchItem, MatchSymbol};

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

    // We could probably make some nice system for testing this
    let first_item = parsed.items.first().expect("has item");
    match *first_item {
        GrammarItem::MatchToken(ref data) => {
            // match { ... }
            let contents0 = data.contents.get(0).unwrap();
            // r"(?i)begin" => "BEGIN"
            let item00 = contents0.items.get(0).unwrap();
            match *item00 {
                MatchItem::Mapped(ref sym, ref mapping, _) => {
                    match *sym {
                        MatchSymbol::Terminal(ref t) => {
                            assert_eq!(format!("{:?}", t), "r#\"(?i)begin\"#")
                        }
                        _ => panic!("expected MatchSymbol::Terminal, but was: {:?}", sym)
                    };
                    assert_eq!(format!("{}", mapping), "\"BEGIN\"")
                },
                _ => panic!("expected MatchItem::Mapped, but was: {:?}", item00)
            };
            // r"(?i)end" => "END",
            let item01 = contents0.items.get(1).unwrap();
            match *item01 {
                MatchItem::Mapped(ref sym, ref mapping, _) => {
                    match *sym {
                        MatchSymbol::Terminal(ref t) => {
                            assert_eq!(format!("{}", t), "r#\"(?i)end\"#")
                        }
                        _ => panic!("expected MatchSymbol::Terminal, but was: {:?}", sym)
                    };
                    assert_eq!(format!("{}", mapping), "\"END\"")
                },
                _ => panic!("expected MatchItem::Mapped, but was: {:?}", item00)
            };
            // else { ... }
            let contents1 = data.contents.get(1).unwrap();
            // r"[a-zA-Z_][a-zA-Z0-9_]*" => IDENTIFIER,
            let item10 = contents1.items.get(0).unwrap();
            match *item10 {
                MatchItem::Mapped(ref sym, ref mapping, _) => {
                    match *sym {
                        MatchSymbol::Terminal(ref t) => {
                            assert_eq!(format!("{:?}", t), "r#\"[a-zA-Z_][a-zA-Z0-9_]*\"#")
                        }
                        _ => panic!("expected MatchSymbol::Terminal, but was: {:?}", sym)
                    };
                    assert_eq!(format!("{}", mapping), "IDENTIFIER")
                },
                _ => panic!("expected MatchItem::Mapped, but was: {:?}", item10)
            };
            // else { ... }
            let contents2 = data.contents.get(2).unwrap();
            // "other",
            let item20 = contents2.items.get(0).unwrap();
            match *item20 {
                MatchItem::Unmapped(ref sym, _) => {
                    match *sym {
                        MatchSymbol::Terminal(ref t) => {
                            assert_eq!(format!("{:?}", t), "\"other\"")
                        }
                        _ => panic!("expected MatchSymbol::Terminal, but was: {:?}", sym)
                    };
                },
                _ => panic!("expected MatchItem::Unmapped, but was: {:?}", item20)
            };
            // _
            let item21 = contents2.items.get(1).unwrap();
            match *item21 {
                MatchItem::Unmapped(ref sym, _) => {
                    match *sym {
                        MatchSymbol::CatchAll() => (),
                        _ => panic!("expected MatchSymbol::CatchAll, but was: {:?}", sym)
                    };
                },
                _ => panic!("expected MatchItem::Unmapped, but was: {:?}", item20)
            };
        }
        _ => panic!("expected MatchToken, but was: {:?}", first_item)
    }
}
