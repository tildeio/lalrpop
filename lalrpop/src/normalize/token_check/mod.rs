//! If an extern token is provided, then this pass validates that
//! terminal IDs have conversions. Otherwise, it generates a
//! tokenizer. This can only be done after macro expansion because
//! some macro arguments never make it into an actual production and
//! are only used in `if` conditions; we use string literals for
//! those, but they do not have to have a defined conversion.

use super::{NormResult, NormError};

use intern::{self, intern};
use lexer::re;
use lexer::dfa::{self, DFAConstructionError, Precedence};
use lexer::nfa::NFAConstructionError::*;
use grammar::consts::*;
use grammar::parse_tree::*;
use collections::Set;
use collections::{map, Map};

#[cfg(test)]
mod test;

pub fn validate(mut grammar: Grammar) -> NormResult<Grammar> {
    let (has_enum_token, all_literals, match_to_user_name_map) = {
        let opt_match_token = grammar.match_token();

        let (match_to_user_name_map, user_name_to_match_map, match_catch_all) = if let Some(mt) = opt_match_token {
            let mut match_to_user = map();
            let mut user_to_match = map();
            let mut catch_all = false;

            // FIXME: This should probably move _inside_ the Validator
            for (idx, mc) in mt.contents.iter().enumerate() {
                let precedence = &mt.contents.len() - idx;
                for item in &mc.items {
                    // TODO: Maybe move this into MatchItem methods
                    match *item {
                        MatchItem::Unmapped(sym, _) => {
                            let precedence_sym = sym.with_match_precedence(precedence);
                            match_to_user.insert(precedence_sym, sym);
                            user_to_match.insert(sym, precedence_sym);
                        },
                        MatchItem::Mapped(sym, mapping, _) => {
                            let precedence_sym = sym.with_match_precedence(precedence);
                            match_to_user.insert(precedence_sym, mapping);
                            user_to_match.insert(mapping, precedence_sym);
                        },
                        MatchItem::CatchAll(_) => { catch_all = true; }
                    };
                }
            }

            (Some(match_to_user), Some(user_to_match), Some(catch_all))
        } else {
            (None, None, None)
        };

        let opt_enum_token = grammar.enum_token();
        let conversions = opt_enum_token.map(|et| {
            et.conversions.iter()
                          .map(|conversion| conversion.from)
                          .collect()
        });

        let mut validator = Validator {
            grammar: &grammar,
            all_literals: map(),
            conversions: conversions,
            user_name_to_match_map: user_name_to_match_map,
            match_catch_all: match_catch_all
        };

        assert!(!opt_match_token.is_some() || !opt_enum_token.is_some(),
                    "expected to not have both match and extern");

        try!(validator.validate());

        (opt_enum_token.is_some(), validator.all_literals, match_to_user_name_map)
    };

    if !has_enum_token {
        try!(construct(&mut grammar, all_literals, match_to_user_name_map));
    }

    Ok(grammar)
}

///////////////////////////////////////////////////////////////////////////
// Validation phase -- this phase walks the grammar and visits all
// terminals. If using an external set of tokens, it checks that all
// terminals have a defined conversion to some pattern. Otherwise,
// it collects all terminals into the `all_literals` set for later use.

struct Validator<'grammar> {
    grammar: &'grammar Grammar,
    all_literals: Map<TerminalLiteral, Span>,
    conversions: Option<Set<TerminalString>>,
    user_name_to_match_map: Option<Map<TerminalString, TerminalString>>,
    match_catch_all: Option<bool>,
}

impl<'grammar> Validator<'grammar> {
    fn validate(&mut self) -> NormResult<()> {
        for item in &self.grammar.items {
            match *item {
                GrammarItem::Use(..) => { }
                GrammarItem::MatchToken(..) => { }
                GrammarItem::ExternToken(_) => { }
                GrammarItem::InternToken(_) => { }
                GrammarItem::Nonterminal(ref data) => {
                    for alternative in &data.alternatives {
                        try!(self.validate_alternative(alternative));
                    }
                }
            }
        }
        Ok(())
    }

    fn validate_alternative(&mut self, alternative: &Alternative) -> NormResult<()> {
        assert!(alternative.condition.is_none()); // macro expansion should have removed these
        try!(self.validate_expr(&alternative.expr));
        Ok(())
    }

    fn validate_expr(&mut self, expr: &ExprSymbol) -> NormResult<()> {
        for symbol in &expr.symbols {
            try!(self.validate_symbol(symbol));
        }
        Ok(())
    }

    fn validate_symbol(&mut self, symbol: &Symbol) -> NormResult<()> {
        match symbol.kind {
            SymbolKind::Expr(ref expr) => {
                try!(self.validate_expr(expr));
            }
            SymbolKind::Terminal(term) => {
                try!(self.validate_terminal(symbol.span, term));
            }
            SymbolKind::Nonterminal(_) => {
            }
            SymbolKind::Repeat(ref repeat) => {
                try!(self.validate_symbol(&repeat.symbol));
            }
            SymbolKind::Choose(ref sym) | SymbolKind::Name(_, ref sym) => {
                try!(self.validate_symbol(sym));
            }
            SymbolKind::Lookahead | SymbolKind::Lookbehind | SymbolKind::Error => {
            }
            SymbolKind::AmbiguousId(id) => {
                panic!("ambiguous id `{}` encountered after name resolution", id)
            }
            SymbolKind::Macro(..) => {
                panic!("macro not removed: {:?}", symbol);
            }
        }

        Ok(())
    }

    fn validate_terminal(&mut self, span: Span, term: TerminalString) -> NormResult<()> {
        match self.conversions {
            // If there is an extern token definition, validate that
            // this terminal has a defined conversion.
            Some(ref c) => {
                if !c.contains(&term) {
                    return_err!(span, "terminal `{}` does not have a pattern defined for it",
                                term);
                }
            }

            // If there is no extern token definition, then collect
            // the terminal literals ("class", r"[a-z]+") into a set.
            None => match term {
                // FIMXE: Should not allow undefined literals if no CatchAll
                TerminalString::Bare(c) => match self.user_name_to_match_map {
                    Some(ref m) => {
                        if let Some(v) = m.get(&term) {
                            // FIXME: I don't think this span here is correct
                            let vl = v.as_literal().expect("must map to a literal");
                            self.all_literals.entry(vl).or_insert(span);
                        } else {
                            return_err!(span, "terminal `{}` does not have a match mapping defined for it",
                                        term);
                        }
                    }

                    None => {
                        // Bare identifiers like `x` can never be resolved
                        // as terminals unless there is a conversion or mapping
                        // defined for them that indicates they are a
                        // terminal; otherwise it's just an unresolved
                        // identifier.
                        panic!("bare literal `{}` without extern token definition", c);
                    }
                },

                TerminalString::Literal(l) => match self.user_name_to_match_map {
                    Some(ref m) => {
                        if let Some(v) = m.get(&term) {
                            // FIXME: I don't think this span here is correct
                            let vl = v.as_literal().expect("must map to a literal");
                            self.all_literals.entry(vl).or_insert(span);
                        } else {
                            // Unwrap should be safe as we shouldn't have match_catch_all without user_name_to_match_map
                            if self.match_catch_all.unwrap() {
                                // FIXME: I don't think this span here is correct
                                self.all_literals.entry(l).or_insert(span);
                            } else {
                                return_err!(span, "terminal `{}` does not have a match mapping defined for it",
                                            term);
                            }

                        }
                    }
                    None => { self.all_literals.entry(l).or_insert(span); }
                },

                // Error is a builtin terminal that always exists
                TerminalString::Error => (),
            }
        }

        Ok(())
    }
}

///////////////////////////////////////////////////////////////////////////
// Construction phase -- if we are constructing a tokenizer, this
// phase builds up an internal token DFA.

pub fn construct(grammar: &mut Grammar, literals_map: Map<TerminalLiteral, Span>, match_to_user_name_map: Option<Map<TerminalString, TerminalString>>) -> NormResult<()> {
    let mut literals: Vec<TerminalLiteral> =
        literals_map.keys()
                    .cloned()
                    .collect();

    // Sort literals by order of increasing precedence.
    literals.sort_by_key(|literal| literal.precedence());

    // Build up two vectors, one of parsed regular expressions and
    // one of precedences, that are parallel with `literals`.
    let mut regexs = Vec::with_capacity(literals.len());
    let mut precedences = Vec::with_capacity(literals.len());
    try!(intern::read(|interner| {
        for &literal in &literals {
            precedences.push(Precedence(literal.precedence()));
            match literal {
                TerminalLiteral::Quoted(s, _) => {
                    regexs.push(re::parse_literal(interner.data(s)));
                }
                TerminalLiteral::Regex(s, _) => {
                    match re::parse_regex(interner.data(s)) {
                        Ok(regex) => regexs.push(regex),
                        Err(error) => {
                            let literal_span = literals_map[&literal];
                            // FIXME -- take offset into account for
                            // span; this requires knowing how many #
                            // the user used, which we do not track
                            return_err!(
                                literal_span,
                                "invalid regular expression: {}",
                                error);
                        }
                    }
                }
            }
        }
        Ok(())
    }));

    let dfa = match dfa::build_dfa(&regexs, &precedences) {
        Ok(dfa) => dfa,
        Err(DFAConstructionError::NFAConstructionError { index, error }) => {
            let feature = match error {
                NamedCaptures => r#"named captures (`(?P<foo>...)`)"#,
                NonGreedy => r#""non-greedy" repetitions (`*?` or `+?`)"#,
                WordBoundary => r#"word boundaries (`\b` or `\B`)"#,
                LineBoundary => r#"line boundaries (`^` or `$`)"#,
                TextBoundary => r#"text boundaries (`^` or `$`)"#,
            };
            let literal = literals[index.index()];
            let span = literals_map[&literal];
            return_err!(
                span,
                "{} are not supported in regular expressions",
                feature)
        }
        Err(DFAConstructionError::Ambiguity { match0, match1 }) => {
            let literal0 = literals[match0.index()];
            let literal1 = literals[match1.index()];
            let span0 = literals_map[&literal0];
            let _span1 = literals_map[&literal1];
            // FIXME(#88) -- it'd be nice to give an example here
            return_err!(
                span0,
                "ambiguity detected between the terminal `{}` and the terminal `{}`",
                literal0, literal1);
        }
    };

    grammar.items.push(GrammarItem::InternToken(InternToken {
        literals: literals,
        match_to_user_name_map: match_to_user_name_map,
        dfa: dfa
    }));

    // we need to inject a `'input` lifetime and `input: &'input str` parameter as well:

    let input_lifetime = intern(INPUT_LIFETIME);
    for parameter in &grammar.type_parameters {
        match *parameter {
            TypeParameter::Lifetime(i) if i == input_lifetime => {
                return_err!(
                    grammar.span,
                    "since there is no external token enum specified, \
                     the `'input` lifetime is implicit and cannot be declared");
            }
            _ => { }
        }
    }

    let input_parameter = intern(INPUT_PARAMETER);
    for parameter in &grammar.parameters {
        if parameter.name == input_parameter {
            return_err!(
                grammar.span,
                "since there is no external token enum specified, \
                 the `input` parameter is implicit and cannot be declared");
        }
    }

    grammar.type_parameters.insert(0, TypeParameter::Lifetime(input_lifetime));

    let parameter = Parameter {
        name: input_parameter,
        ty: TypeRef::Ref {
            lifetime: Some(input_lifetime),
            mutable: false,
            referent: Box::new(TypeRef::Id(intern("str")))
        }
    };
    grammar.parameters.push(parameter);

    Ok(())
}


