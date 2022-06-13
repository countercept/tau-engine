use std::collections::HashMap;

use aho_corasick::AhoCorasickBuilder;
use regex::{RegexBuilder, RegexSetBuilder};

use crate::parser::{Expression, Match, MatchType, Search};
use crate::tokeniser::BoolSym;

pub fn coalesce(expression: Expression, identifiers: &HashMap<String, Expression>) -> Expression {
    match expression {
        Expression::BooleanGroup(symbol, expressions) => {
            let mut scratch = vec![];
            for expression in expressions {
                scratch.push(coalesce(expression, identifiers));
            }
            Expression::BooleanGroup(symbol, scratch)
        }
        Expression::BooleanExpression(left, symbol, right) => {
            let left = coalesce(*left, identifiers);
            let right = coalesce(*right, identifiers);
            Expression::BooleanExpression(Box::new(left), symbol, Box::new(right))
        }
        Expression::Identifier(i) => identifiers
            .get(&i)
            .expect("could not get identifier")
            .clone(),
        Expression::Match(symbol, expression) => {
            Expression::Match(symbol, Box::new(coalesce(*expression, identifiers)))
        }
        Expression::Negate(expression) => {
            Expression::Negate(Box::new(coalesce(*expression, identifiers)))
        }
        Expression::Nested(field, expression) => {
            Expression::Nested(field, Box::new(coalesce(*expression, identifiers)))
        }
        Expression::Boolean(_)
        | Expression::Cast(_, _)
        | Expression::Field(_)
        | Expression::Float(_)
        | Expression::Integer(_)
        | Expression::Null
        | Expression::Search(_, _, _) => expression,
    }
}

pub fn shake(expression: Expression) -> Expression {
    match expression {
        Expression::BooleanGroup(symbol, expressions) => {
            let length = expressions.len();
            let expressions = match symbol {
                BoolSym::And => {
                    let mut scratch = vec![];
                    for expression in expressions {
                        let shaken = shake(expression);
                        scratch.push(shaken);
                    }
                    scratch
                }
                BoolSym::Or => {
                    let mut needles = HashMap::new();
                    let mut nested = HashMap::new();
                    let mut patterns = HashMap::new();

                    // NOTE: Order is crucial here just like in the parser, thus we copy its ideal
                    // ordering.
                    let mut any = vec![];
                    let mut exact = vec![];
                    let mut starts_with = vec![];
                    let mut ends_with = vec![];
                    let mut contains = vec![];
                    let mut aho = vec![];
                    let mut regex = vec![];
                    let mut regex_set = vec![];
                    let mut rest = vec![];

                    for expression in expressions {
                        let shaken = shake(expression);

                        match shaken {
                            Expression::Nested(field, expression) => {
                                let expressions = nested.entry(field).or_insert(vec![]);
                                (*expressions).push(*expression);
                            }
                            Expression::Search(
                                Search::AhoCorasick(_, contexts, insensitive),
                                field,
                                cast,
                            ) => {
                                let expressions =
                                    needles.entry((field, cast, insensitive)).or_insert(vec![]);
                                for context in contexts {
                                    let value = context.value().to_owned();
                                    (*expressions).push((context.clone(), value));
                                }
                            }
                            Expression::Search(Search::Contains(value), field, cast) => {
                                let expressions =
                                    needles.entry((field, cast, false)).or_insert(vec![]);
                                (*expressions).push((MatchType::Contains(value.clone()), value));
                            }
                            Expression::Search(Search::EndsWith(value), field, cast) => {
                                let expressions =
                                    needles.entry((field, cast, false)).or_insert(vec![]);
                                (*expressions).push((MatchType::EndsWith(value.clone()), value));
                            }
                            Expression::Search(Search::Exact(value), field, cast) => {
                                let expressions =
                                    needles.entry((field, cast, false)).or_insert(vec![]);
                                (*expressions).push((MatchType::Exact(value.clone()), value));
                            }
                            Expression::Search(Search::StartsWith(value), field, cast) => {
                                let expressions =
                                    needles.entry((field, cast, false)).or_insert(vec![]);
                                (*expressions).push((MatchType::StartsWith(value.clone()), value));
                            }
                            Expression::Search(Search::Any, _, _) => {
                                any.push(shaken);
                            }
                            Expression::Search(Search::Regex(r, insensitive), field, cast) => {
                                let patterns =
                                    patterns.entry((field, cast, insensitive)).or_insert(vec![]);
                                (*patterns).push(r.as_str().to_owned());
                            }
                            Expression::Search(Search::RegexSet(r, insensitive), field, cast) => {
                                let patterns =
                                    patterns.entry((field, cast, insensitive)).or_insert(vec![]);
                                for pattern in r.patterns() {
                                    (*patterns).push(pattern.to_owned());
                                }
                            }
                            _ => rest.push(shaken),
                        }
                    }

                    for ((field, cast, insensitive), searches) in needles {
                        if !insensitive && searches.len() == 1 {
                            let search = searches.into_iter().next().expect("could not get search");
                            match search.0 {
                                MatchType::Contains(v) => {
                                    contains.push(Expression::Search(
                                        Search::Contains(v),
                                        field,
                                        cast,
                                    ));
                                }
                                MatchType::EndsWith(v) => {
                                    ends_with.push(Expression::Search(
                                        Search::EndsWith(v),
                                        field,
                                        cast,
                                    ));
                                }
                                MatchType::Exact(v) => {
                                    exact.push(Expression::Search(Search::Exact(v), field, cast));
                                }
                                MatchType::StartsWith(v) => {
                                    starts_with.push(Expression::Search(
                                        Search::StartsWith(v),
                                        field,
                                        cast,
                                    ));
                                }
                            };
                        } else {
                            let (context, needles): (Vec<_>, Vec<_>) = searches.into_iter().unzip();
                            let expression = Expression::Search(
                                Search::AhoCorasick(
                                    Box::new(
                                        AhoCorasickBuilder::new()
                                            .dfa(true)
                                            .ascii_case_insensitive(insensitive)
                                            .build(needles),
                                    ),
                                    context,
                                    insensitive,
                                ),
                                field,
                                cast,
                            );
                            aho.push(expression);
                        };
                    }

                    for (field, expressions) in nested {
                        let shaken = if expressions.len() == 1 {
                            shake(
                                expressions
                                    .into_iter()
                                    .next()
                                    .expect("could not get expression"),
                            )
                        } else {
                            shake(Expression::BooleanGroup(symbol, expressions))
                        };
                        rest.push(Expression::Nested(field, Box::new(shaken)));
                    }

                    for ((field, cast, insensitive), patterns) in patterns {
                        if patterns.len() == 1 {
                            let pattern =
                                patterns.into_iter().next().expect("could not get pattern");
                            let expression = Expression::Search(
                                Search::Regex(
                                    RegexBuilder::new(&pattern)
                                        .case_insensitive(insensitive)
                                        .build()
                                        .expect("could not build regex"),
                                    insensitive,
                                ),
                                field,
                                cast,
                            );
                            regex.push(expression);
                        } else {
                            let expression = Expression::Search(
                                Search::RegexSet(
                                    RegexSetBuilder::new(patterns)
                                        .case_insensitive(insensitive)
                                        .build()
                                        .expect("could not build regex set"),
                                    insensitive,
                                ),
                                field,
                                cast,
                            );
                            regex_set.push(expression);
                        }
                    }

                    let mut scratch = vec![];
                    scratch.extend(any);
                    exact.sort_by(|x, y| match (x, y) {
                        (
                            Expression::Search(Search::Exact(a), _, _),
                            Expression::Search(Search::Exact(b), _, _),
                        ) => a.len().cmp(&b.len()),
                        _ => std::cmp::Ordering::Equal,
                    });
                    scratch.extend(exact);
                    starts_with.sort_by(|x, y| match (x, y) {
                        (
                            Expression::Search(Search::StartsWith(a), _, _),
                            Expression::Search(Search::StartsWith(b), _, _),
                        ) => a.len().cmp(&b.len()),
                        _ => std::cmp::Ordering::Equal,
                    });
                    scratch.extend(starts_with);
                    ends_with.sort_by(|x, y| match (x, y) {
                        (
                            Expression::Search(Search::EndsWith(a), _, _),
                            Expression::Search(Search::EndsWith(b), _, _),
                        ) => a.len().cmp(&b.len()),
                        _ => std::cmp::Ordering::Equal,
                    });
                    scratch.extend(ends_with);
                    contains.sort_by(|x, y| match (x, y) {
                        (
                            Expression::Search(Search::Contains(a), _, _),
                            Expression::Search(Search::Contains(b), _, _),
                        ) => a.len().cmp(&b.len()),
                        _ => std::cmp::Ordering::Equal,
                    });
                    scratch.extend(contains);
                    aho.sort_by(|x, y| match (x, y) {
                        (
                            Expression::Search(Search::AhoCorasick(_, a, case0), _, _),
                            Expression::Search(Search::AhoCorasick(_, b, case1), _, _),
                        ) => (b.len(), case1).cmp(&(a.len(), case0)),
                        _ => std::cmp::Ordering::Equal,
                    });
                    scratch.extend(aho);
                    regex.sort_by(|x, y| match (x, y) {
                        (
                            Expression::Search(Search::Regex(reg0, case0), _, _),
                            Expression::Search(Search::Regex(reg1, case1), _, _),
                        ) => (reg0.as_str(), case0).cmp(&(reg1.as_str(), case1)),
                        _ => std::cmp::Ordering::Equal,
                    });
                    scratch.extend(regex);
                    regex_set.sort_by(|x, y| match (x, y) {
                        (
                            Expression::Search(Search::RegexSet(set0, case0), _, _),
                            Expression::Search(Search::RegexSet(set1, case1), _, _),
                        ) => (set0.patterns(), case0).cmp(&(set1.patterns(), case1)),
                        _ => std::cmp::Ordering::Equal,
                    });
                    scratch.extend(regex_set);
                    scratch.extend(rest);
                    scratch
                }
                _ => unreachable!(),
            };
            if expressions.len() != length {
                shake(Expression::BooleanGroup(symbol, expressions))
            } else if expressions.len() == 1 {
                expressions
                    .into_iter()
                    .next()
                    .expect("could not get expression")
            } else {
                Expression::BooleanGroup(symbol, expressions)
            }
        }
        Expression::BooleanExpression(left, symbol, right) => {
            let left = shake(*left);
            let right = shake(*right);
            match (left, symbol, right) {
                (
                    Expression::BooleanGroup(BoolSym::And, mut left),
                    BoolSym::And,
                    Expression::BooleanGroup(BoolSym::And, right),
                ) => {
                    left.extend(right);
                    shake(Expression::BooleanGroup(BoolSym::And, left))
                }
                (Expression::BooleanGroup(BoolSym::And, mut left), BoolSym::And, right) => {
                    left.push(right);
                    shake(Expression::BooleanGroup(BoolSym::And, left))
                }
                (left, BoolSym::And, Expression::BooleanGroup(BoolSym::And, right)) => {
                    let mut left = vec![left];
                    left.extend(right);
                    shake(Expression::BooleanGroup(BoolSym::And, left))
                }
                (
                    Expression::BooleanGroup(BoolSym::Or, mut left),
                    BoolSym::Or,
                    Expression::BooleanGroup(BoolSym::Or, right),
                ) => {
                    left.extend(right);
                    shake(Expression::BooleanGroup(BoolSym::Or, left))
                }
                (Expression::BooleanGroup(BoolSym::Or, mut left), BoolSym::Or, right) => {
                    left.push(right);
                    shake(Expression::BooleanGroup(BoolSym::Or, left))
                }
                (left, BoolSym::Or, Expression::BooleanGroup(BoolSym::Or, right)) => {
                    let mut left = vec![left];
                    left.extend(right);
                    shake(Expression::BooleanGroup(BoolSym::Or, left))
                }
                (Expression::BooleanExpression(x, BoolSym::And, y), BoolSym::And, z) => {
                    shake(Expression::BooleanGroup(BoolSym::And, vec![*x, *y, z]))
                }
                (x, BoolSym::And, Expression::BooleanExpression(y, BoolSym::And, z)) => {
                    shake(Expression::BooleanGroup(BoolSym::And, vec![x, *y, *z]))
                }
                (Expression::BooleanExpression(x, BoolSym::Or, y), BoolSym::Or, z) => {
                    shake(Expression::BooleanGroup(BoolSym::Or, vec![*x, *y, z]))
                }
                (x, BoolSym::Or, Expression::BooleanExpression(y, BoolSym::Or, z)) => {
                    shake(Expression::BooleanGroup(BoolSym::Or, vec![x, *y, *z]))
                }
                (Expression::Negate(left), BoolSym::And, Expression::Negate(right)) => {
                    shake(Expression::Negate(Box::new(shake(
                        Expression::BooleanExpression(left, BoolSym::Or, right),
                    ))))
                }
                (left, _, right) => {
                    Expression::BooleanExpression(Box::new(left), symbol, Box::new(right))
                }
            }
        }
        Expression::Match(Match::All, expression) => {
            // NOTE: We have to be careful what we optimise here as we could really break the
            // logic...
            match *expression {
                Expression::BooleanGroup(BoolSym::Or, expressions) => {
                    Expression::BooleanGroup(BoolSym::And, expressions)
                }
                _ => Expression::Match(Match::All, expression),
            }
        }
        Expression::Negate(expression) => match *expression {
            Expression::BooleanGroup(BoolSym::Or, _) => {
                shake(Expression::Match(Match::Of(0), expression))
            }
            Expression::Negate(inner) => shake(*inner),
            _ => Expression::Negate(Box::new(shake(*expression))),
        },
        Expression::Nested(field, expression) => {
            Expression::Nested(field, Box::new(shake(*expression)))
        }
        Expression::Boolean(_)
        | Expression::Cast(_, _)
        | Expression::Field(_)
        | Expression::Float(_)
        | Expression::Identifier(_)
        | Expression::Integer(_)
        | Expression::Match(_, _)
        | Expression::Null
        | Expression::Search(_, _, _) => expression,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coalesce_basic() {
        let mut identifiers = HashMap::new();
        identifiers.insert(
            "A".to_owned(),
            Expression::BooleanExpression(
                Box::new(Expression::Field("count".to_owned())),
                BoolSym::Equal,
                Box::new(Expression::Integer(1)),
            ),
        );
        let expression = Expression::Identifier("A".to_owned());

        let coalesced = coalesce(expression, &identifiers);

        let expected = Expression::BooleanExpression(
            Box::new(Expression::Field("count".to_owned())),
            BoolSym::Equal,
            Box::new(Expression::Integer(1)),
        );

        assert_eq!(coalesced, expected);
    }

    #[test]
    fn shake_and_nots() {
        let expression = Expression::BooleanExpression(
            Box::new(Expression::Negate(Box::new(Expression::Null))),
            BoolSym::And,
            Box::new(Expression::Negate(Box::new(Expression::Null))),
        );
        let shaken = shake(expression);

        let expected = Expression::Negate(Box::new(Expression::BooleanExpression(
            Box::new(Expression::Null),
            BoolSym::Or,
            Box::new(Expression::Null),
        )));

        assert_eq!(shaken, expected);

        let expression = Expression::BooleanExpression(
            Box::new(Expression::Negate(Box::new(Expression::Null))),
            BoolSym::And,
            Box::new(Expression::BooleanExpression(
                Box::new(Expression::Negate(Box::new(Expression::Null))),
                BoolSym::And,
                Box::new(Expression::Negate(Box::new(Expression::Null))),
            )),
        );
        let shaken = shake(expression);

        let expected = Expression::Match(
            Match::Of(0),
            Box::new(Expression::BooleanGroup(
                BoolSym::Or,
                vec![Expression::Null, Expression::Null, Expression::Null],
            )),
        );

        assert_eq!(shaken, expected);
    }

    #[test]
    fn shake_ands() {
        let expression = Expression::BooleanExpression(
            Box::new(Expression::Null),
            BoolSym::And,
            Box::new(Expression::Null),
        );
        let shaken = shake(expression);

        let expected = Expression::BooleanExpression(
            Box::new(Expression::Null),
            BoolSym::And,
            Box::new(Expression::Null),
        );

        assert_eq!(shaken, expected);

        let expression = Expression::BooleanExpression(
            Box::new(Expression::Null),
            BoolSym::And,
            Box::new(Expression::BooleanExpression(
                Box::new(Expression::Null),
                BoolSym::And,
                Box::new(Expression::Null),
            )),
        );
        let shaken = shake(expression);

        let expected = Expression::BooleanGroup(
            BoolSym::And,
            vec![Expression::Null, Expression::Null, Expression::Null],
        );

        assert_eq!(shaken, expected);
    }

    #[test]
    fn shake_ors() {
        let expression = Expression::BooleanExpression(
            Box::new(Expression::Null),
            BoolSym::Or,
            Box::new(Expression::Null),
        );
        let shaken = shake(expression);

        let expected = Expression::BooleanExpression(
            Box::new(Expression::Null),
            BoolSym::Or,
            Box::new(Expression::Null),
        );

        assert_eq!(shaken, expected);

        let expression = Expression::BooleanExpression(
            Box::new(Expression::Null),
            BoolSym::Or,
            Box::new(Expression::BooleanExpression(
                Box::new(Expression::Null),
                BoolSym::Or,
                Box::new(Expression::Null),
            )),
        );
        let shaken = shake(expression);

        let expected = Expression::BooleanGroup(
            BoolSym::Or,
            vec![Expression::Null, Expression::Null, Expression::Null],
        );

        assert_eq!(shaken, expected);
    }

    #[test]
    fn shake_group_of_nested() {
        let expression = Expression::BooleanGroup(
            BoolSym::Or,
            vec![
                Expression::Nested(
                    "ids".to_owned(),
                    Box::new(Expression::Search(
                        Search::Exact("e2ec14cb-299e-4adf-bb09-04a6a8417bca".to_owned()),
                        "id".to_owned(),
                        false,
                    )),
                ),
                Expression::Nested(
                    "ids".to_owned(),
                    Box::new(Expression::Search(
                        Search::Exact("e2ec14cb-299e-4adf-bb09-04a6a8417bcb".to_owned()),
                        "id".to_owned(),
                        false,
                    )),
                ),
                Expression::Nested(
                    "ids".to_owned(),
                    Box::new(Expression::Search(
                        Search::Exact("e2ec14cb-299e-4adf-bb09-04a6a8417bcc".to_owned()),
                        "id".to_owned(),
                        false,
                    )),
                ),
            ],
        );
        let shaken = shake(expression);

        let expected = Expression::Nested(
            "ids".to_owned(),
            Box::new(Expression::Search(
                Search::AhoCorasick(
                    Box::new(AhoCorasickBuilder::new().dfa(true).build(vec![
                        "e2ec14cb-299e-4adf-bb09-04a6a8417bca",
                        "e2ec14cb-299e-4adf-bb09-04a6a8417bcb",
                        "e2ec14cb-299e-4adf-bb09-04a6a8417bcc",
                    ])),
                    vec![
                        MatchType::Exact("e2ec14cb-299e-4adf-bb09-04a6a8417bca".to_owned()),
                        MatchType::Exact("e2ec14cb-299e-4adf-bb09-04a6a8417bcb".to_owned()),
                        MatchType::Exact("e2ec14cb-299e-4adf-bb09-04a6a8417bcc".to_owned()),
                    ],
                    false,
                ),
                "id".to_owned(),
                false,
            )),
        );

        assert_eq!(shaken, expected);
    }

    #[test]
    fn shake_group_or() {
        // NOTE: This is not a solvable expression but tests what we need testing
        let expression = Expression::BooleanGroup(
            BoolSym::Or,
            vec![
                Expression::Search(
                    Search::AhoCorasick(
                        Box::new(
                            AhoCorasickBuilder::new()
                                .dfa(true)
                                .ascii_case_insensitive(false)
                                .build(vec![
                                    "Quick".to_owned(),
                                    "Brown".to_owned(),
                                    "Fox".to_owned(),
                                ]),
                        ),
                        vec![
                            MatchType::Contains("Quick".to_owned()),
                            MatchType::Exact("Brown".to_owned()),
                            MatchType::EndsWith("Fox".to_owned()),
                        ],
                        false,
                    ),
                    "name".to_owned(),
                    false,
                ),
                Expression::Search(
                    Search::AhoCorasick(
                        Box::new(
                            AhoCorasickBuilder::new()
                                .dfa(true)
                                .ascii_case_insensitive(true)
                                .build(vec![
                                    "quick".to_owned(),
                                    "brown".to_owned(),
                                    "fox".to_owned(),
                                ]),
                        ),
                        vec![
                            MatchType::Contains("quick".to_owned()),
                            MatchType::Exact("brown".to_owned()),
                            MatchType::EndsWith("fox".to_owned()),
                        ],
                        true,
                    ),
                    "name".to_owned(),
                    false,
                ),
                Expression::Search(Search::Any, "name".to_owned(), false),
                Expression::Search(Search::Contains("afoo".to_owned()), "a".to_owned(), false),
                Expression::Search(Search::Contains("foo".to_owned()), "name".to_owned(), false),
                Expression::Search(Search::EndsWith("bbar".to_owned()), "b".to_owned(), false),
                Expression::Search(Search::EndsWith("bar".to_owned()), "name".to_owned(), false),
                Expression::Search(Search::Exact("cbaz".to_owned()), "c".to_owned(), false),
                Expression::Search(Search::Exact("baz".to_owned()), "name".to_owned(), false),
                Expression::Search(
                    Search::Regex(
                        RegexBuilder::new("foo")
                            .case_insensitive(false)
                            .build()
                            .unwrap(),
                        false,
                    ),
                    "name".to_owned(),
                    false,
                ),
                Expression::Search(
                    Search::Regex(
                        RegexBuilder::new("bar")
                            .case_insensitive(true)
                            .build()
                            .unwrap(),
                        true,
                    ),
                    "name".to_owned(),
                    false,
                ),
                Expression::Search(
                    Search::RegexSet(
                        RegexSetBuilder::new(vec!["lorem"])
                            .case_insensitive(false)
                            .build()
                            .unwrap(),
                        false,
                    ),
                    "name".to_owned(),
                    false,
                ),
                Expression::Search(
                    Search::RegexSet(
                        RegexSetBuilder::new(vec!["ipsum"])
                            .case_insensitive(true)
                            .build()
                            .unwrap(),
                        true,
                    ),
                    "name".to_owned(),
                    false,
                ),
                Expression::Search(
                    Search::StartsWith("dfoobar".to_owned()),
                    "d".to_owned(),
                    false,
                ),
                Expression::Search(
                    Search::StartsWith("foobar".to_owned()),
                    "name".to_owned(),
                    false,
                ),
            ],
        );
        let shaken = shake(expression);

        let expected = Expression::BooleanGroup(
            BoolSym::Or,
            vec![
                Expression::Search(Search::Any, "name".to_owned(), false),
                Expression::Search(Search::Exact("cbaz".to_owned()), "c".to_owned(), false),
                Expression::Search(
                    Search::StartsWith("dfoobar".to_owned()),
                    "d".to_owned(),
                    false,
                ),
                Expression::Search(Search::EndsWith("bbar".to_owned()), "b".to_owned(), false),
                Expression::Search(Search::Contains("afoo".to_owned()), "a".to_owned(), false),
                Expression::Search(
                    Search::AhoCorasick(
                        Box::new(
                            AhoCorasickBuilder::new()
                                .dfa(true)
                                .ascii_case_insensitive(false)
                                .build(vec![
                                    "Quick".to_owned(),
                                    "Brown".to_owned(),
                                    "Fox".to_owned(),
                                    "foo".to_owned(),
                                    "bar".to_owned(),
                                    "baz".to_owned(),
                                    "foobar".to_owned(),
                                ]),
                        ),
                        vec![
                            MatchType::Contains("Quick".to_owned()),
                            MatchType::Exact("Brown".to_owned()),
                            MatchType::EndsWith("Fox".to_owned()),
                            MatchType::Contains("foo".to_owned()),
                            MatchType::EndsWith("bar".to_owned()),
                            MatchType::Exact("baz".to_owned()),
                            MatchType::StartsWith("foobar".to_owned()),
                        ],
                        false,
                    ),
                    "name".to_owned(),
                    false,
                ),
                Expression::Search(
                    Search::AhoCorasick(
                        Box::new(
                            AhoCorasickBuilder::new()
                                .dfa(true)
                                .ascii_case_insensitive(true)
                                .build(vec![
                                    "quick".to_owned(),
                                    "brown".to_owned(),
                                    "fox".to_owned(),
                                ]),
                        ),
                        vec![
                            MatchType::Contains("quick".to_owned()),
                            MatchType::Exact("brown".to_owned()),
                            MatchType::EndsWith("fox".to_owned()),
                        ],
                        true,
                    ),
                    "name".to_owned(),
                    false,
                ),
                Expression::Search(
                    Search::RegexSet(
                        RegexSetBuilder::new(vec!["bar", "ipsum"])
                            .case_insensitive(true)
                            .build()
                            .unwrap(),
                        true,
                    ),
                    "name".to_owned(),
                    false,
                ),
                Expression::Search(
                    Search::RegexSet(
                        RegexSetBuilder::new(vec!["foo", "lorem"])
                            .case_insensitive(false)
                            .build()
                            .unwrap(),
                        false,
                    ),
                    "name".to_owned(),
                    false,
                ),
            ],
        );

        assert_eq!(shaken, expected);
    }

    #[test]
    fn shake_group_or_1() {
        // NOTE: This is not a solvable expression but tests what we need testing
        let expression = Expression::BooleanGroup(BoolSym::Or, vec![Expression::Null]);
        let shaken = shake(expression);

        let expected = Expression::Null;

        assert_eq!(shaken, expected);
    }

    #[test]
    fn shake_nested() {
        let expression = Expression::Nested(
            "ids".to_owned(),
            Box::new(Expression::BooleanGroup(
                BoolSym::Or,
                vec![
                    Expression::Search(
                        Search::Exact("e2ec14cb-299e-4adf-bb09-04a6a8417bca".to_owned()),
                        "id".to_owned(),
                        false,
                    ),
                    Expression::Search(
                        Search::Exact("e2ec14cb-299e-4adf-bb09-04a6a8417bcb".to_owned()),
                        "id".to_owned(),
                        false,
                    ),
                    Expression::Search(
                        Search::Exact("e2ec14cb-299e-4adf-bb09-04a6a8417bcc".to_owned()),
                        "id".to_owned(),
                        false,
                    ),
                ],
            )),
        );
        let shaken = shake(expression);

        let expected = Expression::Nested(
            "ids".to_owned(),
            Box::new(Expression::Search(
                Search::AhoCorasick(
                    Box::new(AhoCorasickBuilder::new().dfa(true).build(vec![
                        "e2ec14cb-299e-4adf-bb09-04a6a8417bca",
                        "e2ec14cb-299e-4adf-bb09-04a6a8417bcb",
                        "e2ec14cb-299e-4adf-bb09-04a6a8417bcc",
                    ])),
                    vec![
                        MatchType::Exact("e2ec14cb-299e-4adf-bb09-04a6a8417bca".to_owned()),
                        MatchType::Exact("e2ec14cb-299e-4adf-bb09-04a6a8417bcb".to_owned()),
                        MatchType::Exact("e2ec14cb-299e-4adf-bb09-04a6a8417bcc".to_owned()),
                    ],
                    false,
                ),
                "id".to_owned(),
                false,
            )),
        );

        assert_eq!(shaken, expected);
    }

    #[test]
    fn shake_match() {
        let expression = Expression::Match(
            Match::All,
            Box::new(Expression::BooleanGroup(
                BoolSym::Or,
                vec![Expression::Null, Expression::Null],
            )),
        );
        let shaken = shake(expression);

        let expected =
            Expression::BooleanGroup(BoolSym::And, vec![Expression::Null, Expression::Null]);

        assert_eq!(shaken, expected);
    }

    #[test]
    fn shake_negate() {
        let expression =
            Expression::Negate(Box::new(Expression::Negate(Box::new(Expression::Null))));
        let shaken = shake(expression);

        let expected = Expression::Null;

        assert_eq!(shaken, expected);
    }
}