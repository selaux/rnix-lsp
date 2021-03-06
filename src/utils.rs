use lsp_types::*;
use rnix::{
    types::*,
    SyntaxNode,
    TextRange,
    TextUnit,
    TokenAtOffset,
};
use std::{
    collections::HashMap,
    convert::TryFrom,
    path::PathBuf,
    rc::Rc,
};

pub fn uri_path(uri: &Url) -> Option<PathBuf> {
    if uri.scheme() != "file" || uri.has_host() {
        return None;
    }
    Some(PathBuf::from(uri.path()))
}
pub fn lookup_pos(code: &str, pos: Position) -> Option<usize> {
    let mut lines = code.split('\n');

    let mut offset = 0;
    for _ in 0..pos.line {
        let line = lines.next()?;

        offset += line.len() + 1;
    }

    lines.next()
        .and_then(|line| {
            Some(
                offset +
                    line.chars()
                    .take(usize::try_from(pos.character).ok()?)
                    .map(char::len_utf8)
                        .sum::<usize>()
            )
        })
}
pub fn offset_to_pos(code: &str, offset: usize) -> Position {
    let start_of_line = code[..offset].rfind('\n').map_or(0, |n| n+1);
    Position {
        line: code[..start_of_line].chars().filter(|&c| c == '\n').count() as u64,
        character: code[start_of_line..offset].chars().map(|c| c.len_utf16() as u64).sum()
    }
}
pub fn range(code: &str, range: TextRange) -> Range {
    Range {
        start: offset_to_pos(code, range.start().to_usize()),
        end: offset_to_pos(code, range.end().to_usize()),
    }
}
pub struct CursorInfo {
    pub path: Vec<String>,
    pub ident: Ident,
}
pub fn ident_at(root: &SyntaxNode, offset: usize) -> Option<CursorInfo> {
    let ident = match root.token_at_offset(TextUnit::from_usize(offset)) {
        TokenAtOffset::None => None,
        TokenAtOffset::Single(node) => Ident::cast(node.parent()),
        TokenAtOffset::Between(left, right) => Ident::cast(left.parent()).or_else(|| Ident::cast(right.parent()))
    }?;
    let parent = ident.node().parent();
    if let Some(attr) = parent.clone().and_then(Key::cast) {
        let mut path = Vec::new();
        for item in attr.path() {
            if item == *ident.node() {
                return Some(CursorInfo {
                    path,
                    ident,
                });
            }

            path.push(Ident::cast(item)?.as_str().into());
        }
        panic!("identifier at cursor is somehow not a child of its parent");
    } else if let Some(mut index) = parent.and_then(Select::cast) {
        let mut path = Vec::new();
        while let Some(new) = Select::cast(index.set()?) {
            path.push(Ident::cast(new.index()?)?.as_str().into());
            index = new;
        }
        if index.set()? != *ident.node() {
            // Only push if not the cursor ident, so that
            // a . b
            //  ^
            // is not [a] and a, but rather [] and a
            path.push(Ident::cast(index.set()?)?.as_str().into());
        }
        path.reverse();
        Some(CursorInfo {
            path,
            ident
        })
    } else {
        Some(CursorInfo {
            path: Vec::new(),
            ident
        })
    }
}

#[derive(Debug)]
pub struct Var {
    pub file: Rc<Url>,
    pub set: SyntaxNode,
    pub key: SyntaxNode,
    pub value: Option<SyntaxNode>
}
pub fn populate<T: EntryHolder>(
    file: &Rc<Url>,
    scope: &mut HashMap<String, Var>,
    set: &T
) -> Option<()> {
    for entry in set.entries() {
        let attr = entry.key()?;
        let mut path = attr.path();
        if let Some(ident) = path.next().and_then(Ident::cast) {
            if !scope.contains_key(ident.as_str()) {
                scope.insert(ident.as_str().into(), Var {
                    file: Rc::clone(file),
                    set: set.node().to_owned(),
                    key: ident.node().to_owned(),
                    value: Some(entry.value()?.to_owned())
                });
            }
        }
    }
    Some(())
}
pub fn scope_for(file: &Rc<Url>, node: SyntaxNode) -> Option<HashMap<String, Var>> {
    let mut scope = HashMap::new();

    let mut current = Some(node);
    while let Some(node) = current {
        match ParsedType::try_from(node.clone()) {
            Ok(ParsedType::LetIn(let_in)) => { populate(&file, &mut scope, &let_in); },
            Ok(ParsedType::LegacyLet(let_)) => { populate(&file, &mut scope, &let_); },
            Ok(ParsedType::AttrSet(set)) => if set.recursive() {
                populate(&file, &mut scope, &set);
            },
            Ok(ParsedType::Lambda(lambda)) => match ParsedType::try_from(lambda.arg()?) {
                Ok(ParsedType::Ident(ident)) => if !scope.contains_key(ident.as_str()) {
                    scope.insert(ident.as_str().into(), Var {
                        file: Rc::clone(&file),
                        set: lambda.node().clone(),
                        key: ident.node().clone(),
                        value: None
                    });
                },
                Ok(ParsedType::Pattern(pattern)) => {
                    for entry in pattern.entries() {
                        let ident = entry.name()?;
                        if !scope.contains_key(ident.as_str()) {
                            scope.insert(ident.as_str().into(), Var {
                                file: Rc::clone(&file),
                                set: lambda.node().to_owned(),
                                key: ident.node().to_owned(),
                                value: None
                            });
                        }
                    }
                },
                _ => ()
            },
            _ => ()
        }
        current = node.parent();
    }

    Some(scope)
}
pub fn selection_ranges(root: &SyntaxNode, content: &str, pos: Position) -> Option<SelectionRange> {
    let pos = lookup_pos(content, pos)?;
    let node = root.token_at_offset(TextUnit::from_usize(pos)).left_biased()?;

    let mut root = None;
    let mut cursor = &mut root;

    let mut last = None;
    for parent in node.ancestors() {
        // De-duplicate
        if last.as_ref() == Some(&parent) {
            continue;
        }

        let text_range = parent.text_range();
        *cursor = Some(Box::new(SelectionRange {
            range: range(content, text_range),
            parent: None,
        }));
        cursor = &mut cursor.as_mut().unwrap().parent;

        last = Some(parent);
    }

    root.map(|b| *b)
}
