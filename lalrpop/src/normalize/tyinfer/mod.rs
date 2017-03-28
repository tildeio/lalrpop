use super::{NormResult, NormError};
use super::norm_util::{self, AlternativeAction, Symbols};

use std::collections::{HashMap};
use grammar::consts::{ERROR, INPUT_LIFETIME, LOCATION};
use grammar::parse_tree::{ActionKind, Alternative,
                          Grammar,
                          NonterminalData, NonterminalString,
                          Path,
                          Span,
                          SymbolKind,
                          TerminalString, TypeRef};
use grammar::repr::{NominalTypeRepr, Types, TypeRepr};
use intern::intern;

#[cfg(test)]
mod test;

pub fn infer_types(grammar: &Grammar) -> NormResult<Types> {
    let inferencer = try!(TypeInferencer::new(&grammar));
    inferencer.infer_types()
}

struct TypeInferencer<'grammar> {
    stack: Vec<NonterminalString>,
    nonterminals: HashMap<NonterminalString, NT<'grammar>>,
    types: Types,
}

#[derive(Copy, Clone)]
struct NT<'grammar> {
    span: Span,
    type_decl: &'grammar Option<TypeRef>,
    alternatives: &'grammar Vec<Alternative>,
}

impl<'grammar> TypeInferencer<'grammar> {
    fn new(grammar: &'grammar Grammar) -> NormResult<TypeInferencer<'grammar>> {
        let types = TypeInferencer::make_types(grammar);

        let nonterminals =
            grammar.items
                   .iter()
                   .filter_map(|item| item.as_nonterminal())
                   .map(|data| {
                       assert!(!data.is_macro_def()); // normalized away by now
                       (data.name, NT::new(data))
                   })
                   .collect();

        Ok(TypeInferencer { stack: vec![],
                            nonterminals: nonterminals,
                            types: types })
    }

    fn make_types(grammar: &Grammar) -> Types {
        let opt_extern_token = grammar.extern_token();

        // Determine error type (if any).
        let error_type =
            opt_extern_token.and_then(|extern_token| {
                extern_token.associated_type(intern(ERROR))
                            .map(|tr| tr.type_ref.type_repr())
            });

        // Determine location type and enum type. If using an internal
        // token, that's specified by us, not user.
        if let Some(intern_token) = grammar.intern_token() {
            let loc_type = // usize
                TypeRepr::usize();
            let input_str = // &'input str
                TypeRepr::Ref {
                    lifetime: Some(intern(INPUT_LIFETIME)),
                    mutable: false,
                    referent: Box::new(TypeRepr::str())
                };
            let enum_type = // (usize, &'input str)
                TypeRepr::Tuple(vec![TypeRepr::usize(), input_str.clone()]);

            let mut types = Types::new(&grammar.prefix, Some(loc_type), error_type, enum_type);

            for &literal in &intern_token.literals {
                let terminal_string = TerminalString::Literal(literal);
                let user_name = intern_token.match_to_user_name_map
                                            .as_ref()
                                            .and_then(|it| it.get(&terminal_string))
                                            .cloned()
                                            .unwrap_or(terminal_string);
                types.add_term_type(user_name, input_str.clone());
            }

            types
        } else {
            let extern_token = opt_extern_token.unwrap();
            let loc_type = extern_token.associated_type(intern(LOCATION))
                                       .map(|tr| tr.type_ref.type_repr());
            let enum_type = extern_token.enum_token
                                        .as_ref()
                                        .unwrap()
                                        .type_name
                                        .type_repr();
            let mut types = Types::new(&grammar.prefix, loc_type, error_type, enum_type);

            // For each defined conversion, figure out the type of the
            // terminal and enter it into `types` by hand if it is not the
            // default. For terminals with custom types, the user should
            // have one or more bindings in the pattern -- if more than
            // one, make a tuple.
            //
            // e.g. "(" => Lparen(..) ==> no custom type
            //      "Num" => Num(<u32>) ==> custom type is u32
            //      "Fraction" => Real(<u32>,<u32>) ==> custom type is (u32, u32)
            for conversion in grammar.enum_token().into_iter()
                                                  .flat_map(|et| &et.conversions)
            {
                let mut tys = Vec::new();
                conversion.to.for_each_binding(&mut |ty| tys.push(ty.type_repr()));
                if tys.is_empty() { continue; }
                let ty = maybe_tuple(tys);
                types.add_term_type(conversion.from, ty);
            }

            types
        }
    }

    fn infer_types(mut self) -> NormResult<Types> {
        let ids: Vec<NonterminalString> =
            self.nonterminals.iter()
                             .map(|(&id, _)| id)
                             .collect();

        for id in ids {
            try!(self.nonterminal_type(id));
            debug_assert!(self.types.lookup_nonterminal_type(id).is_some());
        }

        Ok(self.types)
    }

    fn nonterminal_type(&mut self, id: NonterminalString) -> NormResult<TypeRepr> {
        if let Some(repr) = self.types.lookup_nonterminal_type(id) {
            return Ok(repr.clone());
        }

        let nt = self.nonterminals[&id];
        if self.stack.contains(&id) {
            return_err!(nt.span, "cannot infer type of `{}` because it references itself", id);
        }

        let ty = try!(self.push(id, |this| {
            if let &Some(ref type_decl) = nt.type_decl {
                return this.type_ref(type_decl);
            }

            // Try to compute the types of all alternatives; note that
            // some may result in an error. Don't report these errors
            // (yet).
            let mut alternative_types = vec![];
            let mut alternative_errors = vec![];
            for alt in nt.alternatives.iter() {
                match this.alternative_type(alt) {
                    Ok(t) => alternative_types.push(t),
                    Err(e) => alternative_errors.push(e),
                }
            }

            // if it never succeeded, report first error
            if alternative_types.is_empty() {
                match alternative_errors.into_iter().next() {
                    Some(err) => { return Err(err); }
                    None => {
                        // if nothing succeeded, and nothing errored,
                        // must have been nothing to start with
                        return_err!(
                            nt.span,
                            "nonterminal `{}` has no alternatives and hence parse cannot succeed",
                            id);
                    }
                }
            }

            // otherwise, check that all the cases where we had success agree
            for ((ty, alt), i) in
                alternative_types[1..].iter().zip(&nt.alternatives[1..]).zip(1..)
            {
                if &alternative_types[0] != ty {
                    return_err!(alt.span,
                                "type of alternative #{} is `{}`, \
                                 but type of first alternative is `{}`",
                                i+1, ty, alternative_types[0]);
                }
            }

            // and use that type
            Ok(alternative_types.pop().unwrap())
        }));

        self.types.add_type(id, ty.clone());
        Ok(ty)
    }

    fn push<F,R>(&mut self, id: NonterminalString, f: F) -> NormResult<R>
        where F: FnOnce(&mut TypeInferencer) -> NormResult<R>
    {
        self.stack.push(id);
        let r = f(self);
        assert_eq!(self.stack.pop().unwrap(), id);
        r
    }

    fn type_ref(&mut self, type_ref: &TypeRef) -> NormResult<TypeRepr> {
        match *type_ref {
            TypeRef::Tuple(ref types) => {
                let types = try! {
                    types.iter().map(|t| self.type_ref(t)).collect()
                };
                Ok(TypeRepr::Tuple(types))
            }
            TypeRef::Nominal { ref path, ref types } => {
                let types = try! {
                    types.iter().map(|t| self.type_ref(t)).collect()
                };
                Ok(TypeRepr::Nominal(NominalTypeRepr { path: path.clone(),
                                                       types: types }))
            }
            TypeRef::Lifetime(id) => {
                Ok(TypeRepr::Lifetime(id))
            }
            TypeRef::Id(id) => {
                Ok(TypeRepr::Nominal(NominalTypeRepr { path: Path::from_id(id),
                                                       types: vec![] }))
            }
            TypeRef::Ref { lifetime, mutable, ref referent } => {
                Ok(TypeRepr::Ref { lifetime: lifetime,
                                   mutable: mutable,
                                   referent: Box::new(try!(self.type_ref(referent))) })
            }
            TypeRef::OfSymbol(ref symbol) => {
                self.symbol_type(symbol)
            }
        }
    }

    fn alternative_type(&mut self, alt: &Alternative) -> NormResult<TypeRepr> {
        match norm_util::analyze_action(alt) {
            AlternativeAction::User(&ActionKind::User(_)) |
            AlternativeAction::User(&ActionKind::Fallible(_)) => {
                return_err!(alt.span, "cannot infer types if there is custom action code");
            }

            AlternativeAction::User(&ActionKind::Lookahead) |
            AlternativeAction::User(&ActionKind::Lookbehind) => {
                Ok(self.types.opt_terminal_loc_type().unwrap().clone())
            }

            AlternativeAction::Default(Symbols::Named(ref syms)) => {
                return_err!(alt.span,
                            "cannot infer types in the presence of named symbols like `{}:{}`",
                            syms[0].1, syms[0].2);
            }

            AlternativeAction::Default(Symbols::Anon(syms)) => {
                let symbol_types: Vec<TypeRepr> = try! {
                    syms.iter()
                        .map(|&(_, sym)| self.symbol_type(&sym.kind))
                        .collect()
                };
                Ok(maybe_tuple(symbol_types))
            }
        }
    }

    fn symbol_type(&mut self, symbol: &SymbolKind) -> NormResult<TypeRepr> {
        match *symbol {
            SymbolKind::Terminal(id) => Ok(self.types.terminal_type(id).clone()),
            SymbolKind::Nonterminal(id) => self.nonterminal_type(id),
            SymbolKind::Choose(ref s) => self.symbol_type(&s.kind),
            SymbolKind::Name(_, ref s) => self.symbol_type(&s.kind),
            SymbolKind::Error => Ok(self.types.parse_error_type().clone()),

            SymbolKind::Repeat(..) | SymbolKind::Expr(..) | SymbolKind::Macro(..) |
            SymbolKind::AmbiguousId(..) | SymbolKind::Lookahead | SymbolKind::Lookbehind => {
                unreachable!("symbol `{:?}` should have been expanded away", symbol)
            }
        }
    }
}

impl<'grammar> NT<'grammar> {
    fn new(data: &'grammar NonterminalData) -> NT<'grammar> {
        NT { span: data.span, type_decl: &data.type_decl, alternatives: &data.alternatives }
    }
}

fn maybe_tuple(v: Vec<TypeRepr>) -> TypeRepr {
    if v.len() == 1 {
        v.into_iter().next().unwrap()
    } else {
        TypeRepr::Tuple(v)
    }
}
