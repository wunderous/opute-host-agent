// Copyright 2009 The Go Authors. All rights reserved.
// SPDX-License-Identifier: BSD-3-Clause; see licenses/Go-BSD.txt.
//! Owned arena for regexp admission: no recursive ownership/drop or matcher.
use serde::Deserialize;
use std::{collections::BTreeMap, sync::OnceLock};
#[derive(Deserialize)]
struct Class {
    plain: Vec<u32>,
    fold: Vec<u32>,
}
#[derive(Deserialize)]
struct Data {
    classes: BTreeMap<String, Class>,
    posix: BTreeMap<String, Class>,
    folds: BTreeMap<u32, u32>,
}
fn data() -> &'static Data {
    static DATA: OnceLock<Data> = OnceLock::new();
    DATA.get_or_init(|| serde_json::from_str(include_str!("regexp_data.json")).unwrap())
}
pub(super) fn fold(r: u32) -> u32 {
    *data().folds.get(&r).unwrap_or(&r)
}
pub(super) fn min_fold(r: u32) -> u32 {
    let mut m = r;
    let mut c = fold(r);
    while c != r {
        m = m.min(c);
        c = fold(c);
    }
    m
}
pub(super) fn table(name: &str, case_fold: bool, posix: bool) -> Option<Vec<u32>> {
    let c = if posix {
        data().posix.get(name)
    } else {
        data().classes.get(name)
    }?;
    Some(if case_fold {
        c.fold.clone()
    } else {
        c.plain.clone()
    })
}
pub(super) fn append(r: &mut Vec<u32>, lo: u32, hi: u32) {
    for i in [2, 4] {
        if r.len() >= i {
            let n = r.len() - i;
            if lo <= r[n + 1] + 1 && r[n] <= hi + 1 {
                r[n] = r[n].min(lo);
                r[n + 1] = r[n + 1].max(hi);
                return;
            }
        }
    }
    r.extend([lo, hi]);
}
pub(super) fn append_folded(r: &mut Vec<u32>, mut lo: u32, mut hi: u32) {
    if lo <= 0x41 && hi >= 0x1e943 || hi < 0x41 || lo > 0x1e943 {
        append(r, lo, hi);
        return;
    }
    if lo < 0x41 {
        append(r, lo, 0x40);
        lo = 0x41;
    }
    if hi > 0x1e943 {
        append(r, 0x1e944, hi);
        hi = 0x1e943;
    }
    for c in lo..=hi {
        append(r, c, c);
        let mut f = fold(c);
        while f != c {
            append(r, f, f);
            f = fold(f);
        }
    }
}
pub(super) fn clean(r: &[u32]) -> Vec<u32> {
    let mut pairs: Vec<_> = r.chunks_exact(2).map(|p| (p[0], p[1])).collect();
    pairs.sort_unstable();
    let mut out: Vec<u32> = Vec::new();
    for (lo, hi) in pairs {
        let n = out.len();
        if n > 0 && lo <= out[n - 1] + 1 {
            out[n - 1] = out[n - 1].max(hi);
        } else {
            out.extend([lo, hi]);
        }
    }
    out
}
pub(super) fn negate(r: &[u32]) -> Vec<u32> {
    let r = clean(r);
    let mut out = Vec::new();
    let mut next = 0;
    for p in r.chunks_exact(2) {
        if next < p[0] {
            out.extend([next, p[0] - 1]);
        }
        next = p[1] + 1;
    }
    if next <= 0x10ffff {
        out.extend([next, 0x10ffff]);
    }
    out
}
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Kind {
    Empty,
    Any,
    AnyNotNL,
    Literal,
    Class,
    Atom(u8),
    Capture,
    Star,
    Plus,
    Quest,
    Repeat,
    Concat,
    Alt,
}
#[derive(Clone)]
pub(super) struct Node {
    pub kind: Kind,
    pub runes: Vec<u32>,
    pub children: Vec<usize>,
    pub flags: u8,
    pub min: u64,
    pub max: Option<u64>,
    pub height: usize,
    pub copies: u64,
    pub size: u64,
}
#[derive(Default)]
pub(super) struct Arena {
    pub nodes: Vec<Node>,
}
impl Arena {
    pub fn node(
        &mut self,
        kind: Kind,
        runes: Vec<u32>,
        children: Vec<usize>,
        flags: u8,
        min: u64,
        max: Option<u64>,
    ) -> usize {
        let height = 1 + children
            .iter()
            .map(|&id| self.nodes[id].height)
            .max()
            .unwrap_or(0);
        let mut copies = children
            .iter()
            .map(|&id| self.nodes[id].copies)
            .max()
            .unwrap_or(1);
        let sum = children.iter().map(|&id| self.nodes[id].size).sum::<u64>();
        let size = match kind {
            Kind::Literal => runes.len() as u64,
            Kind::Capture | Kind::Star => sum.saturating_add(2),
            Kind::Plus | Kind::Quest => sum.saturating_add(1),
            Kind::Concat => sum,
            Kind::Alt => sum.saturating_add(children.len().saturating_sub(1) as u64),
            Kind::Repeat => {
                copies = if max == Some(0) {
                    0
                } else {
                    copies.saturating_mul(max.unwrap_or(min))
                };
                match max {
                    Some(m) => sum.saturating_mul(m).saturating_add(m - min),
                    None if min == 0 => sum.saturating_add(2),
                    None => sum.saturating_mul(min).saturating_add(1),
                }
            }
            _ => 1,
        }
        .max(1);
        let id = self.nodes.len();
        self.nodes.push(Node {
            kind,
            runes,
            children,
            flags,
            min,
            max,
            height,
            copies,
            size,
        });
        id
    }
    pub fn empty(&mut self) -> usize {
        self.node(Kind::Empty, vec![], vec![], 0, 0, None)
    }
    pub fn literal(&mut self, runes: Vec<u32>, folded: bool) -> usize {
        self.node(Kind::Literal, runes, vec![], u8::from(folded), 0, None)
    }
    pub fn normalize_push(&mut self, id: usize) -> (usize, usize) {
        let cost = self.nodes[id].runes.len();
        if self.nodes[id].kind == Kind::Class {
            let ranges = self.nodes[id].runes.clone();
            (self.class(ranges), cost)
        } else {
            (id, cost)
        }
    }
    fn clean_alt(&mut self, id: usize) -> usize {
        let n = &self.nodes[id];
        if n.kind != Kind::Class {
            return id;
        }
        let kind = if n.runes == [0, 0x10ffff] {
            Some(Kind::Any)
        } else if n.runes == [0, 9, 11, 0x10ffff] {
            Some(Kind::AnyNotNL)
        } else {
            None
        };
        if let Some(kind) = kind {
            self.node(kind, vec![], vec![], 0, 0, None)
        } else {
            id
        }
    }
    pub fn class(&mut self, runes: Vec<u32>) -> usize {
        let r = clean(&runes);
        if r.len() == 2 && r[0] == r[1] {
            return self.literal(vec![r[0]], false);
        }
        if r.len() == 4 && r[0] == r[1] && r[2] == r[3] && fold(r[0]) == r[2] && fold(r[2]) == r[0]
            || r.len() == 2 && r[0] + 1 == r[1] && fold(r[0]) == r[1] && fold(r[1]) == r[0]
        {
            return self.literal(vec![r[0]], true);
        }
        self.node(Kind::Class, r, vec![], 0, 0, None)
    }
    pub fn merge_literals(&mut self, nodes: &mut Vec<usize>) {
        let n = nodes.len();
        if n < 2 {
            return;
        }
        let a = &self.nodes[nodes[n - 2]];
        let b = &self.nodes[nodes[n - 1]];
        if a.kind == Kind::Literal && b.kind == Kind::Literal && a.flags == b.flags {
            let first = nodes[n - 2];
            let second = nodes[n - 1];
            let runes = std::mem::take(&mut self.nodes[second].runes);
            self.nodes[first].runes.extend(runes);
            self.nodes[first].size = self.nodes[first].runes.len() as u64;
            nodes.pop();
        }
    }
    pub fn collapse(&mut self, kind: Kind, children: Vec<usize>) -> usize {
        if children.is_empty() {
            return self.empty();
        }
        if children.len() == 1 {
            return if kind == Kind::Alt {
                self.clean_alt(children[0])
            } else {
                children[0]
            };
        }
        let mut flat = Vec::new();
        for id in children {
            if self.nodes[id].kind == kind {
                flat.extend(self.nodes[id].children.clone());
            } else {
                flat.push(id);
            }
        }
        if kind == Kind::Alt {
            if let Some(last) = flat.last_mut() {
                *last = self.clean_alt(*last);
            }
            flat = self.factor(flat);
        }
        if flat.len() == 1 {
            flat[0]
        } else {
            self.node(kind, vec![], flat, 0, 0, None)
        }
    }
    fn leading(&self, id: usize) -> usize {
        if self.nodes[id].kind == Kind::Concat {
            self.nodes[id].children[0]
        } else {
            id
        }
    }
    fn remove_string(&mut self, id: usize, len: usize) -> usize {
        let n = self.nodes[id].clone();
        if n.kind == Kind::Concat {
            let mut children = n.children;
            children[0] = self.remove_string(children[0], len);
            if self.nodes[children[0]].kind == Kind::Empty {
                children.remove(0);
            }
            return self.collapse(Kind::Concat, children);
        }
        if n.kind == Kind::Literal {
            if len == n.runes.len() {
                self.empty()
            } else {
                self.literal(n.runes[len..].to_vec(), n.flags != 0)
            }
        } else {
            id
        }
    }
    fn remove_node(&mut self, id: usize) -> usize {
        if self.nodes[id].kind == Kind::Concat {
            let children = self.nodes[id].children[1..].to_vec();
            self.collapse(Kind::Concat, children)
        } else {
            self.empty()
        }
    }
    fn is_class(&self, id: usize) -> bool {
        let n = &self.nodes[id];
        matches!(n.kind, Kind::Class | Kind::Any | Kind::AnyNotNL)
            || n.kind == Kind::Literal && n.runes.len() == 1
    }
    fn equal(&self, a: usize, b: usize) -> bool {
        let a = &self.nodes[a];
        let b = &self.nodes[b];
        if a.kind != b.kind {
            return false;
        }
        if matches!(a.kind, Kind::Literal | Kind::Class) {
            return a.runes == b.runes;
        }
        if matches!(a.kind, Kind::Repeat | Kind::Star | Kind::Plus | Kind::Quest)
            && (a.flags != b.flags || a.min != b.min || a.max != b.max)
        {
            return false;
        }
        a.children.len() == b.children.len()
            && a.children
                .iter()
                .zip(&b.children)
                .all(|(&a, &b)| self.equal(a, b))
    }
    fn as_ranges(&self, id: usize) -> Vec<u32> {
        let n = &self.nodes[id];
        if n.kind == Kind::Any {
            return vec![0, 0x10ffff];
        }
        if n.kind == Kind::AnyNotNL {
            return vec![0, 9, 11, 0x10ffff];
        }
        if n.kind == Kind::Class {
            n.runes.clone()
        } else {
            let r = n.runes[0];
            let mut ranges = vec![r, r];
            if n.flags != 0 {
                let mut f = fold(r);
                while f != r {
                    append(&mut ranges, f, f);
                    f = fold(f);
                }
            }
            clean(&ranges)
        }
    }
    fn factor(&mut self, children: Vec<usize>) -> Vec<usize> {
        // Go's four ordered rounds: literal prefixes, simple fixed prefixes,
        // runs of classes, and adjacent empty alternatives.
        let mut out = Vec::new();
        let mut start = 0;
        while start < children.len() {
            let first = self.nodes[self.leading(children[start])].clone();
            let mut prefix = if first.kind == Kind::Literal {
                first.runes.clone()
            } else {
                vec![]
            };
            let mut end = start + 1;
            while end < children.len() && !prefix.is_empty() {
                let n = &self.nodes[self.leading(children[end])];
                if n.kind != Kind::Literal || n.flags != first.flags {
                    break;
                }
                let common = prefix
                    .iter()
                    .zip(&n.runes)
                    .take_while(|(a, b)| a == b)
                    .count();
                if common == 0 {
                    break;
                }
                prefix.truncate(common);
                end += 1;
            }
            if end - start > 1 {
                let p = self.literal(prefix.clone(), first.flags != 0);
                let suffixes = children[start..end]
                    .iter()
                    .map(|&id| self.remove_string(id, prefix.len()))
                    .collect();
                let suffix = self.collapse(Kind::Alt, suffixes);
                out.push(self.node(Kind::Concat, vec![], vec![p, suffix], 0, 0, None));
            } else {
                out.push(children[start]);
            }
            start = end;
        }
        let children = out;
        let mut out = Vec::new();
        let mut start = 0;
        while start < children.len() {
            let first = self.leading(children[start]);
            let n = &self.nodes[first];
            let eligible = self.is_class(first)
                || n.kind == Kind::Repeat && n.max == Some(n.min) && self.is_class(n.children[0]);
            let mut end = start + 1;
            if eligible {
                while end < children.len() && self.equal(first, self.leading(children[end])) {
                    end += 1;
                }
            }
            if end - start > 1 {
                let suffixes = children[start..end]
                    .iter()
                    .map(|&id| self.remove_node(id))
                    .collect();
                let suffix = self.collapse(Kind::Alt, suffixes);
                out.push(self.node(Kind::Concat, vec![], vec![first, suffix], 0, 0, None));
            } else {
                out.push(children[start]);
            }
            start = end;
        }
        let children = out;
        let mut out = Vec::new();
        let mut i = 0;
        while i < children.len() {
            if self.is_class(children[i]) {
                let start = i;
                let mut ranges = Vec::new();
                while i < children.len() && self.is_class(children[i]) {
                    for p in self.as_ranges(children[i]).chunks_exact(2) {
                        append(&mut ranges, p[0], p[1]);
                    }
                    i += 1;
                }
                if i - start == 1 {
                    out.push(children[start]);
                } else {
                    out.push(self.node(Kind::Class, clean(&ranges), vec![], 0, 0, None));
                }
            } else {
                out.push(children[i]);
                i += 1;
            }
        }
        let mut final_nodes: Vec<usize> = Vec::new();
        for id in out {
            if self.nodes[id].kind == Kind::Empty
                && final_nodes
                    .last()
                    .is_some_and(|&last| self.nodes[last].kind == Kind::Empty)
            {
                continue;
            }
            final_nodes.push(id);
        }
        final_nodes
    }
    // swapVerticalBar merges simple adjacent classes before factoring. Keep
    // merged nodes as classes (do not apply push's singleton rewrite).
    pub fn branch(&mut self, branches: &mut Vec<usize>, node: usize) {
        if let Some(&previous) = branches.last() {
            if self.is_class(previous) && self.is_class(node) {
                let mut ranges = self.as_ranges(previous);
                for p in self.as_ranges(node).chunks_exact(2) {
                    append(&mut ranges, p[0], p[1]);
                }
                branches.pop();
                branches.push(self.node(Kind::Class, clean(&ranges), vec![], 0, 0, None));
                return;
            }
        }
        branches.push(node);
    }
}
