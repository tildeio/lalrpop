use parser;
use normalize::resolve::resolve;
use lexer::dfa::interpret;
use grammar::parse_tree::{Grammar,GrammarItem};
use test_util;

fn check_err(expected_err: &str,
             grammar: &str,
             span: &str) {
    let parsed_grammar = parser::parse_grammar(&grammar).unwrap();
    let parsed_grammar = resolve(parsed_grammar).unwrap();
    let err = super::validate(parsed_grammar).unwrap_err();
    test_util::check_norm_err(expected_err, span, err);
}

fn parse_grammar(grammar: &str) -> Grammar {
    let parsed_grammar = parser::parse_grammar(&grammar).unwrap();
    let parsed_grammar = resolve(parsed_grammar).unwrap();
    super::validate(parsed_grammar).unwrap()
}

fn check_intern_token(grammar: &str,
                      expected_tokens: Vec<(&'static str, &'static str)>)
{
    let parsed_grammar = parse_grammar(&grammar);
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
    let parsed_grammar = parse_grammar(r#"grammar; match { _ }"#);
    let first_item = parsed_grammar.items.first().expect("has item");
    match *first_item {
        GrammarItem::MatchToken() => (), // OK
        _ => panic!("expected MatchToken, but was {:?}", first_item)
    }
}
