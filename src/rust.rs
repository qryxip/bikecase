use anyhow::{anyhow, Context as _};
use if_chain::if_chain;
use itertools::Itertools as _;
use pulldown_cmark::{CodeBlockKind, Event, Options, Parser, Tag};
use syn::{Lit, Meta, MetaNameValue};

use std::borrow::Cow;
use std::fmt::Display;
use std::iter;
use std::ops::Range;

pub(crate) fn extract_cargo_lang_code<C: Display + Send + Sync + 'static, F: FnOnce() -> C>(
    code: &str,
    on_not_found: F,
) -> anyhow::Result<String> {
    let (_, cargo_lang_code) = replace_cargo_lang_code(code, "", on_not_found)?;
    Ok(cargo_lang_code)
}

pub(crate) fn replace_cargo_lang_code_with_default(code: &str) -> anyhow::Result<(String, String)> {
    return replace_cargo_lang_code(code, MANIFEST, || {
        anyhow!("could not find the `cargo` code block")
    });

    static MANIFEST: &str = "# Leave blank.";
}

pub(crate) fn replace_cargo_lang_code<C: Display + Send + Sync + 'static, F: FnOnce() -> C>(
    code: &str,
    with: &str,
    on_not_found: F,
) -> anyhow::Result<(String, String)> {
    let mut code_lines = code.lines().map(Cow::from).map(Some).collect::<Vec<_>>();

    let syn::File { shebang, attrs, .. } = syn::parse_file(code)?;
    if shebang.is_some() {
        code_lines[0] = None;
    }

    let mut remove = |i: usize, start: _, end: Option<_>| {
        let entry = &mut code_lines[i];
        if let Some(line) = entry {
            let first = &line[..start];
            let second = match end {
                Some(end) if end < line.len() => &line[end..],
                _ => "",
            };
            *line = format!("{}{}", first, second).into();
            if line.is_empty() {
                *entry = None;
            }
        }
    };

    let mut doc = "".to_owned();

    for attr in attrs {
        if_chain! {
            if let Ok(meta) = attr.parse_meta();
            if let Meta::NameValue(MetaNameValue { path, lit, .. }) = meta;
            if path.get_ident().map_or(false, |i| i == "doc");
            if let Lit::Str(lit_str) = lit;
            then {
                doc += lit_str.value().trim_start_matches(' ');
                doc += "\n";

                for tt in attr.tokens {
                    let (start, end) = (tt.span().start(), tt.span().end());
                    if start.line == end.line {
                        remove(start.line - 1, start.column, Some(end.column));
                    } else {
                        remove(start.line - 1, start.column, None);
                        for i in start.line..end.line - 1 {
                            remove(i, 0, None);
                        }
                        remove(end.line - 1, 0, Some(end.column));
                    }
                }
            }
        }
    }

    let doc_span = Parser::new_ext(&doc, Options::all())
        .into_offset_iter()
        .fold(State::None, |mut state, (event, span)| {
            match &state {
                State::None => {
                    if let Event::Start(Tag::CodeBlock(CodeBlockKind::Fenced(kind))) = event {
                        if &*kind == "cargo" {
                            state = State::Start;
                        }
                    }
                }
                State::Start => {
                    if let Event::Text(_) = event {
                        state = State::Text(span);
                    }
                }
                State::Text(span) => {
                    if let Event::End(Tag::CodeBlock(CodeBlockKind::Fenced(kind))) = event {
                        if &*kind == "cargo" {
                            state = State::End(span.clone());
                        }
                    }
                }
                State::End(_) => {}
            }
            state
        })
        .end()
        .with_context(on_not_found)?;

    let with = if with.is_empty() || with.ends_with('\n') {
        with.to_owned()
    } else {
        format!("{}\n", with)
    };

    let converted_doc = format!("{}{}{}", &doc[..doc_span.start], with, &doc[doc_span.end..]);

    let converted_code = shebang
        .map(Into::into)
        .into_iter()
        .chain(converted_doc.lines().map(|line| {
            if line.is_empty() {
                "//!".into()
            } else {
                format!("//! {}", line).into()
            }
        }))
        .chain(code_lines.into_iter().flatten())
        .interleave_shortest(iter::repeat("\n".into()))
        .join("");

    return Ok((converted_code, doc[doc_span].to_owned()));

    #[derive(Debug)]
    enum State {
        None,
        Start,
        Text(Range<usize>),
        End(Range<usize>),
    }

    impl State {
        fn end(self) -> Option<Range<usize>> {
            match self {
                Self::End(span) => Some(span),
                _ => None,
            }
        }
    }
}
