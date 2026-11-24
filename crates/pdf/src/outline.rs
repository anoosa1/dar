//! Extraction of PDF document outlines (`/Outlines`, ISO 32000-1 §12.3.3) and resolution of
//! explicit and named destinations (§12.3.2).

use std::collections::{HashMap, HashSet};

use hayro::hayro_syntax::Pdf;
use hayro::hayro_syntax::object::{self, Array, Dict, MaybeRef, Name, Object, ObjectIdentifier};
use hayro::hayro_syntax::xref::XRef;

use crate::OutlineEntry;
use crate::text::decode_pdf_text_string;

/// Hard cap on the number of outline entries returned for any document.
const MAX_OUTLINE_ENTRIES: usize = 5_000;

/// Hard cap on outline nesting depth (0-based; depths `0..MAX_OUTLINE_DEPTH` are visited).
const MAX_OUTLINE_DEPTH: usize = 64;

/// Hard caps for walking the `/Names /Dests` name tree and `/Dests` dictionary.
const MAX_NAME_TREE_DEPTH: usize = 64;
const MAX_NAME_TREE_NODES: usize = 10_000;
const MAX_NAMED_DESTS: usize = 100_000;

pub(crate) fn extract_outline(pdf: &Pdf) -> Vec<OutlineEntry> {
    let xref = pdf.xref();
    let Some(catalog) = xref.get::<Dict<'_>>(xref.root_id()) else {
        return Vec::new();
    };
    let Some(outlines_root) = catalog.get::<Dict<'_>>(b"Outlines") else {
        return Vec::new();
    };
    let Some(first) = outlines_root.get::<Dict<'_>>(b"First") else {
        return Vec::new();
    };

    let pages = pdf.pages();
    let page_count = pages.len();
    let mut page_index_by_id = HashMap::with_capacity(page_count);
    for (idx, page) in pages.iter().enumerate() {
        if let Some(id) = page.raw().obj_id() {
            page_index_by_id.insert(id, idx);
        }
    }

    let mut resolver = DestResolver {
        xref,
        catalog: &catalog,
        page_index_by_id: &page_index_by_id,
        page_count,
        named: None,
    };

    let mut entries = Vec::new();
    let mut visited = HashSet::new();
    if let Some(root_id) = outlines_root.obj_id() {
        visited.insert(root_id);
    }

    // Explicit stack for iterative depth-first pre-order traversal: push `/Next` before `/First`
    // so children are visited before siblings without risking stack overflow.
    let mut stack: Vec<(Dict<'_>, usize)> = vec![(first, 0)];

    while let Some((item, depth)) = stack.pop() {
        if entries.len() >= MAX_OUTLINE_ENTRIES {
            break;
        }
        if let Some(id) = item.obj_id()
            && !visited.insert(id)
        {
            continue;
        }

        let title = item
            .get::<object::String<'_>>(b"Title")
            .map(|s| decode_pdf_text_string(s.as_bytes()))
            .unwrap_or_default();
        let page = resolver.resolve_item_dest(&item);

        entries.push(OutlineEntry { title, page, depth });

        if let Some(next) = item.get::<Dict<'_>>(b"Next")
            && next.obj_id().is_none_or(|id| !visited.contains(&id))
        {
            stack.push((next, depth));
        }

        if depth + 1 < MAX_OUTLINE_DEPTH
            && let Some(child) = item.get::<Dict<'_>>(b"First")
            && child.obj_id().is_none_or(|id| !visited.contains(&id))
        {
            stack.push((child, depth + 1));
        }
    }

    entries
}

struct NamedDests {
    /// Destinations from the PDF 1.1 catalog `/Dests` dictionary.
    dict_dests: HashMap<Vec<u8>, usize>,
    /// Destinations from the PDF 1.2+ catalog `/Names /Dests` name tree.
    tree_dests: HashMap<Vec<u8>, usize>,
}

struct DestResolver<'a> {
    xref: &'a XRef,
    catalog: &'a Dict<'a>,
    page_index_by_id: &'a HashMap<ObjectIdentifier, usize>,
    page_count: usize,
    named: Option<NamedDests>,
}

impl<'a> DestResolver<'a> {
    fn resolve_item_dest(&mut self, item: &Dict<'a>) -> Option<usize> {
        if let Some(dest) = item.get::<Object<'a>>(b"Dest") {
            return self.resolve_dest_object(dest);
        }

        let action = item.get::<Dict<'a>>(b"A")?;
        let action_type = action.get::<Name<'a>>(b"S")?;
        if action_type.as_ref() != b"GoTo" {
            return None;
        }
        let dest = action.get::<Object<'a>>(b"D")?;
        self.resolve_dest_object(dest)
    }

    fn resolve_dest_object(&mut self, dest: Object<'a>) -> Option<usize> {
        match dest {
            Object::Array(arr) => {
                resolve_explicit_dest(&arr, self.page_index_by_id, self.page_count)
            }
            Object::Dict(dict) => {
                let arr = dict.get::<Array<'a>>(b"D")?;
                resolve_explicit_dest(&arr, self.page_index_by_id, self.page_count)
            }
            Object::Name(name) => {
                let named = self.ensure_named_dests();
                let key = name.as_ref();
                named
                    .dict_dests
                    .get(key)
                    .or_else(|| named.tree_dests.get(key))
                    .copied()
            }
            Object::String(string) => {
                let named = self.ensure_named_dests();
                let key = string.as_bytes();
                named
                    .tree_dests
                    .get(key)
                    .or_else(|| named.dict_dests.get(key))
                    .copied()
            }
            _ => None,
        }
    }

    fn ensure_named_dests(&mut self) -> &NamedDests {
        self.named.get_or_insert_with(|| {
            let dict_dests = collect_dests_dict(
                self.xref,
                self.catalog,
                self.page_index_by_id,
                self.page_count,
            );
            let tree_dests = collect_dests_name_tree(
                self.xref,
                self.catalog,
                self.page_index_by_id,
                self.page_count,
            );
            NamedDests {
                dict_dests,
                tree_dests,
            }
        })
    }
}

fn resolve_maybe_obj<'a>(xref: &'a XRef, item: MaybeRef<Object<'a>>) -> Option<Object<'a>> {
    match item {
        MaybeRef::Ref(obj_ref) => xref.get::<Object<'a>>(obj_ref.into()),
        MaybeRef::NotRef(obj) => Some(obj),
    }
}

fn resolve_named_dest_value<'a>(
    xref: &'a XRef,
    val: MaybeRef<Object<'a>>,
    page_index_by_id: &HashMap<ObjectIdentifier, usize>,
    page_count: usize,
) -> Option<usize> {
    let obj = resolve_maybe_obj(xref, val)?;
    match obj {
        Object::Array(arr) => resolve_explicit_dest(&arr, page_index_by_id, page_count),
        Object::Dict(dict) => {
            let arr = dict.get::<Array<'a>>(b"D")?;
            resolve_explicit_dest(&arr, page_index_by_id, page_count)
        }
        _ => None,
    }
}

fn resolve_explicit_dest(
    arr: &Array<'_>,
    page_index_by_id: &HashMap<ObjectIdentifier, usize>,
    page_count: usize,
) -> Option<usize> {
    let first = arr.raw_iter().next()?;
    match first {
        MaybeRef::Ref(obj_ref) => page_index_by_id
            .get(&ObjectIdentifier::from(obj_ref))
            .copied(),
        MaybeRef::NotRef(Object::Number(n)) => {
            let idx = usize::try_from(n.as_i64()).ok()?;
            (idx < page_count).then_some(idx)
        }
        _ => None,
    }
}

fn collect_dests_dict(
    xref: &XRef,
    catalog: &Dict<'_>,
    page_index_by_id: &HashMap<ObjectIdentifier, usize>,
    page_count: usize,
) -> HashMap<Vec<u8>, usize> {
    let mut out = HashMap::new();
    let Some(dests_dict) = catalog.get::<Dict<'_>>(b"Dests") else {
        return out;
    };

    for (name, val) in dests_dict.entries().take(MAX_NAMED_DESTS) {
        if let Some(page) = resolve_named_dest_value(xref, val, page_index_by_id, page_count) {
            out.insert(name.as_ref().to_vec(), page);
        }
    }
    out
}

fn collect_dests_name_tree(
    xref: &XRef,
    catalog: &Dict<'_>,
    page_index_by_id: &HashMap<ObjectIdentifier, usize>,
    page_count: usize,
) -> HashMap<Vec<u8>, usize> {
    let mut out = HashMap::new();
    let Some(names_dict) = catalog.get::<Dict<'_>>(b"Names") else {
        return out;
    };
    let Some(dests_tree) = names_dict.get::<Dict<'_>>(b"Dests") else {
        return out;
    };

    let mut visited = HashSet::new();
    let mut nodes_seen = 0usize;
    let mut stack: Vec<(Dict<'_>, usize)> = vec![(dests_tree, 0)];

    while let Some((node, depth)) = stack.pop() {
        if out.len() >= MAX_NAMED_DESTS || nodes_seen >= MAX_NAME_TREE_NODES {
            break;
        }
        if let Some(id) = node.obj_id()
            && !visited.insert(id)
        {
            continue;
        }
        nodes_seen += 1;

        if let Some(names_arr) = node.get::<Array<'_>>(b"Names") {
            let mut iter = names_arr.raw_iter();
            while let Some(raw_key) = iter.next() {
                if out.len() >= MAX_NAMED_DESTS {
                    break;
                }
                let Some(raw_val) = iter.next() else {
                    break;
                };
                let Some(Object::String(key)) = resolve_maybe_obj(xref, raw_key) else {
                    continue;
                };
                if let Some(page) =
                    resolve_named_dest_value(xref, raw_val, page_index_by_id, page_count)
                {
                    out.insert(key.as_bytes().to_vec(), page);
                }
            }
        }

        if depth + 1 < MAX_NAME_TREE_DEPTH
            && let Some(kids_arr) = node.get::<Array<'_>>(b"Kids")
        {
            let kids: Vec<Dict<'_>> = kids_arr
                .raw_iter()
                .filter_map(|raw| resolve_maybe_obj(xref, raw)?.into_dict())
                .collect();
            for kid in kids.into_iter().rev() {
                if kid.obj_id().is_none_or(|id| !visited.contains(&id)) {
                    stack.push((kid, depth + 1));
                }
            }
        }
    }

    out
}
