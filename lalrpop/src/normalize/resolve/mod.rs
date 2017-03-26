//! Resolves identifiers to decide if they are macros, terminals, or
//! nonterminals. Rewrites the parse tree accordingly.

use super::{NormResult, NormError};

use grammar::parse_tree::*;
use intern::{InternedString};
use collections::{map, Map};

#[cfg(test)]
mod test;

pub fn resolve(mut grammar: Grammar) -> NormResult<Grammar> {
    try!(resolve_in_place(&mut grammar));
    Ok(grammar)
}

fn resolve_in_place(grammar: &mut Grammar) -> NormResult<()> {
    let globals = {
        let nonterminal_identifiers =
            grammar.items
                   .iter()
                   .filter_map(|item| item.as_nonterminal())
                   .map(|nt| (nt.span, nt.name.0, Def::Nonterminal(nt.args.len())));

        let terminal_identifiers =
            grammar.items
                   .iter()
                   .filter_map(|item| item.as_extern_token())
                   .flat_map(|extern_token| extern_token.enum_token.as_ref())
                   .flat_map(|enum_token| &enum_token.conversions)
                   .filter_map(|conversion| match conversion.from {
                       TerminalString::Literal(..) | TerminalString::Error => None,
                       TerminalString::Bare(id) => Some((conversion.span, id, Def::Terminal)),
                   });

        // Extract all the bare identifiers that appear in the RHS of a `match` declaration.
        // Example:
        //     match {
        //         r"(?)begin" => "BEGIN",
        //     } else {
        //         r"[a-zA-Z_][a-zA-Z0-9_]*" => ID,
        //     }
        // This would result in `vec![ID]`.
        let match_identifiers =
            grammar.items
                   .iter()
                   .filter_map(|item| item.as_match_token())
                   .flat_map(|match_token| &match_token.contents)
                   .flat_map(|match_contents| &match_contents.items)
                   .filter_map(|item| match *item {
                       MatchItem::Mapped(_, TerminalString::Bare(id), _) => Some((item.span(), id, Def::Terminal)),
                       _ => None
                   });

        let all_identifiers =
            nonterminal_identifiers.chain(terminal_identifiers).chain(match_identifiers);

        let mut identifiers = map();
        for (span, id, def) in all_identifiers {
            if let Some(old_def) = identifiers.insert(id, def) {
                let description = def.description();
                let old_description = old_def.description();
                if description == old_description {
                    return_err!(
                        span,
                        "two {}s declared with the name `{}`",
                        description, id);
                } else {
                    return_err!(
                        span,
                        "{} and {} both declared with the name `{}`",
                        description, old_description, id);
                }
            }
        }

        ScopeChain { previous: None, identifiers: identifiers }
    };

    let validator = Validator {
        globals: globals,
    };

    validator.validate(grammar)
}

struct Validator {
    globals: ScopeChain<'static>,
}

#[derive(Copy, Clone, Debug)]
enum Def {
    Terminal,
    Nonterminal(usize), // argument is the number of macro arguments
    MacroArg,
}

#[derive(Debug)]
struct ScopeChain<'scope> {
    previous: Option<&'scope ScopeChain<'scope>>,
    identifiers: Map<InternedString, Def>,
}

impl Def {
    fn description(&self) -> &'static str {
        match *self {
            Def::Terminal => "terminal",
            Def::Nonterminal(0) => "nonterminal",
            Def::Nonterminal(_) => "macro",
            Def::MacroArg => "macro argument",
        }
    }
}

impl Validator {
    fn validate(&self, grammar: &mut Grammar) -> NormResult<()> {
        for item in &mut grammar.items {
            match *item {
                GrammarItem::Use(..) => { }
                GrammarItem::MatchToken(..) => {}
                GrammarItem::InternToken(..) => {}
                GrammarItem::ExternToken(..) => {}
                GrammarItem::Nonterminal(ref mut data) => {
                    let identifiers = try!(self.validate_macro_args(data.span, &data.args));
                    let locals = ScopeChain {
                        previous: Some(&self.globals),
                        identifiers: identifiers,
                    };
                    for alternative in &mut data.alternatives {
                        try!(self.validate_alternative(&locals, alternative));
                    }
                }
            }
        }
        Ok(())
    }

    fn validate_macro_args(&self,
                           span: Span,
                           args: &[NonterminalString])
                           -> NormResult<Map<InternedString, Def>> {
        for (index, arg) in args.iter().enumerate() {
            if args[..index].contains(&arg) {
                return_err!(span, "multiple macro arguments declared with the name `{}`", arg);
            }
        }
        Ok(args.iter().map(|&nt| (nt.0, Def::MacroArg)).collect())
    }

    fn validate_alternative(&self,
                            scope: &ScopeChain,
                            alternative: &mut Alternative)
                            -> NormResult<()> {
        if let Some(ref condition) = alternative.condition {
            let def = try!(self.validate_id(scope, condition.span, condition.lhs.0));
            match def {
                Def::MacroArg => { /* OK */ }
                _ => {
                    return_err!(condition.span,
                                "only macro arguments can be used in conditions, \
                                 not {}s like `{}`",
                                def.description(), condition.lhs);
                }
            }
        }

        try!(self.validate_expr(scope, &mut alternative.expr));

        Ok(())
    }

    fn validate_expr(&self,
                     scope: &ScopeChain,
                     expr: &mut ExprSymbol)
                     -> NormResult<()> {
        for symbol in &mut expr.symbols {
            try!(self.validate_symbol(scope, symbol));
        }

        Ok(())
    }

    fn validate_symbol(&self,
                       scope: &ScopeChain,
                       symbol: &mut Symbol)
                       -> NormResult<()> {
        match symbol.kind {
            SymbolKind::Expr(ref mut expr) => {
                try!(self.validate_expr(scope, expr));
            }
            SymbolKind::AmbiguousId(name) => {
                try!(self.rewrite_ambiguous_id(scope, name, symbol));
            }
            SymbolKind::Terminal(_) => {
                /* see postvalidate! */
            }
            SymbolKind::Nonterminal(id) => {
                // in normal operation, the parser never produces Nonterminal(_) entries,
                // but during testing we do produce nonterminal entries
                let def = try!(self.validate_id(scope, symbol.span, id.0));
                match def {
                    Def::Nonterminal(0) |
                    Def::MacroArg => {
                        // OK
                    }
                    Def::Terminal |
                    Def::Nonterminal(_) => {
                        return_err!(symbol.span, "`{}` is a {}, not a nonterminal",
                                    def.description(), id);
                    }
                }
            }
            SymbolKind::Macro(ref mut msym) => {
                debug_assert!(msym.args.len() > 0);
                let def = try!(self.validate_id(scope, symbol.span, msym.name.0));
                match def {
                    Def::Nonterminal(0) |
                    Def::Terminal |
                    Def::MacroArg => {
                        return_err!(symbol.span, "`{}` is a {}, not a macro",
                                    def.description(), msym.name)
                    }
                    Def::Nonterminal(arity) => {
                        if arity != msym.args.len() {
                            return_err!(symbol.span,
                                        "wrong number of arguments to `{}`: \
                                         expected {}, found {}",
                                        msym.name, arity, msym.args.len());
                        }
                    }
                }

                for arg in &mut msym.args {
                    try!(self.validate_symbol(scope, arg));
                }
            }
            SymbolKind::Repeat(ref mut repeat) => {
                try!(self.validate_symbol(scope, &mut repeat.symbol));
            }
            SymbolKind::Choose(ref mut sym) | SymbolKind::Name(_, ref mut sym) => {
                try!(self.validate_symbol(scope, sym));
            }
            SymbolKind::Lookahead | SymbolKind::Lookbehind | SymbolKind::Error => {
            }
        }

        Ok(())
    }

    fn rewrite_ambiguous_id(&self,
                            scope: &ScopeChain,
                            id: InternedString,
                            symbol: &mut Symbol)
                            -> NormResult<()> {
        symbol.kind = match try!(self.validate_id(scope, symbol.span, id)) {
            Def::MacroArg |
            Def::Nonterminal(0) => SymbolKind::Nonterminal(NonterminalString(id)),
            Def::Terminal => SymbolKind::Terminal(TerminalString::Bare(id)),
            Def::Nonterminal(_) => return_err!(symbol.span, "`{}` is a macro", id),
        };
        Ok(())
    }

    fn validate_id(&self,
                   scope: &ScopeChain,
                   span: Span,
                   id: InternedString)
                   -> NormResult<Def> {
        match scope.def(id) {
            Some(def) => Ok(def),
            None => return_err!(span, "no definition found for `{}`", id)
        }
    }
}

impl<'scope> ScopeChain<'scope> {
    fn def(&self, id: InternedString) -> Option<Def> {
        self.identifiers.get(&id)
                        .cloned()
                        .or_else(|| self.previous.and_then(|s| s.def(id)))
    }
}
