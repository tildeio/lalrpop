use parser;
use normalize::resolve::resolve;
use normalize::NormResult;
use lexer::dfa::interpret;
use grammar::parse_tree::{Grammar,GrammarItem};
use test_util;

fn parse_grammar(grammar: &str) -> NormResult<Grammar> {
    let parsed_grammar = parser::parse_grammar(&grammar).expect(format!("Unable to parse; grammar={}", grammar).as_str());
    let parsed_grammar = resolve(parsed_grammar).expect(format!("Unable to resolve; grammar={}", grammar).as_str());
    super::validate(parsed_grammar)
}

fn check_err(expected_err: &str,
             grammar: &str,
             span: &str) {
    let err = parse_grammar(grammar).unwrap_err();
    test_util::check_norm_err(expected_err, span, err);
}

fn check_intern_token(grammar: &str,
                      expected_tokens: Vec<(&'static str, &'static str)>)
{
    let parsed_grammar = parse_grammar(&grammar).unwrap();
    let intern_token = parsed_grammar.intern_token().unwrap();
    for (input, expected_literal) in expected_tokens {
        let actual_literal =
            interpret::interpret(&intern_token.dfa, input)
            .map(|(index, text)| {
                let literal = intern_token.literals[index.index()];
                (literal, text)
            });
        let actual_literal = format!("{:?}", actual_literal);
        if expected_literal != actual_literal {
            panic!("input `{}` matched `{}` but we expected `{}`",
                   input,
                   actual_literal,
                   expected_literal);
        }
    }
}

#[test]
fn unknown_terminal() {
    check_err(
        r#"terminal `"\+"` does not have a pattern defined for it"#,
        r#"grammar; extern { enum Term { } } X = X "+";"#,
        r#"                                        ~~~ "#);
}

#[test]
fn unknown_id_terminal() {
    check_err(
        r#"terminal `"foo"` does not have a pattern defined for it"#,
        r#"grammar; extern { enum Term { } } X = X "foo";"#,
        r#"                                        ~~~~~ "#);
}

#[test]
fn tick_input_lifetime_already_declared() {
    check_err(
        r#".*the `'input` lifetime is implicit and cannot be declared"#,
        r#"grammar<'input>; X = X "foo";"#,
        r#"~~~~~~~                      "#);
}

#[test]
fn input_parameter_already_declared() {
    check_err(
        r#".*the `input` parameter is implicit and cannot be declared"#,
        r#"grammar(input:u32); X = X "foo";"#,
        r#"~~~~~~~                         "#);
}

#[test]
fn invalid_regular_expression_unterminated_group() {
    check_err(
        r#"Unclosed parenthesis"#,
        r#"grammar; X = X r"(123";"#,
        r#"               ~~~~~~~ "#);
}

#[test]
fn quoted_literals() {
    check_intern_token(
        r#"grammar; X = X "+" "-" "foo" "(" ")";"#,
        vec![("+", r#"Some(("+", "+"))"#),
             ("-", r#"Some(("-", "-"))"#),
             ("(", r#"Some(("(", "("))"#),
             (")", r#"Some((")", ")"))"#),
             ("foo", r#"Some(("foo", "foo"))"#),
             ("<", r#"None"#)]);
}

#[test]
fn regex_literals() {
    check_intern_token(
        r#"grammar; X = X r"[a-z]+" r"[0-9]+";"#,
        vec![
            ("a", r##"Some((r#"[a-z]+"#, "a"))"##),
            ("def", r##"Some((r#"[a-z]+"#, "def"))"##),
            ("1", r##"Some((r#"[0-9]+"#, "1"))"##),
            ("9123456", r##"Some((r#"[0-9]+"#, "9123456"))"##),
                ]);
}

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
        let parsed = parse_grammar(block).expect(format!("Invalid grammar; grammar={}", block).as_str());
        let first_item = parsed.items.first().expect("has item");
        match *first_item {
            GrammarItem::MatchToken() => (), // OK
            _ => panic!("expected MatchToken, but was {:?}", first_item)
        }
    }
}

#[test]
fn invalid_match_block() {
    let grammar = r#"grammar; match { _, "abc" }"#;
    match parser::parse_grammar(&grammar) {
        Ok(_)   => panic!("expected to fail parsing; grammar={}", grammar),
        Err(_)  => () // Ok
    }
}
