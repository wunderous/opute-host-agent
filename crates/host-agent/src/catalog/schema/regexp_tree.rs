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
    LeftParen,
    VerticalBar,
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
    pub copies: u64,
}
#[derive(Default)]
pub(super) struct Arena {
    pub nodes: Vec<Node>,
    free: Vec<usize>,
    sizes: Option<BTreeMap<usize, u64>>,
    heights: Option<BTreeMap<usize, u64>>,
    repeats: u64,
    pub num_runes: usize,
    pub context: Vec<usize>,
    failure: Option<&'static str>,
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
        let node = self.build(kind, runes, children, flags, min, max);
        let id = self.free.pop().unwrap_or(self.nodes.len());
        if id == self.nodes.len() {
            self.nodes.push(node);
        } else {
            self.nodes[id] = node;
        }
        id
    }
    fn build(
        &self,
        kind: Kind,
        runes: Vec<u32>,
        children: Vec<usize>,
        flags: u8,
        min: u64,
        max: Option<u64>,
    ) -> Node {
        let mut copies = children
            .iter()
            .map(|&id| self.nodes[id].copies)
            .max()
            .unwrap_or(1);
        if kind == Kind::Repeat {
            copies = if max == Some(0) {
                0
            } else {
                copies.saturating_mul(max.unwrap_or(min))
            };
        }
        Node {
            kind,
            runes,
            children,
            flags,
            min,
            max,
            copies,
        }
    }
    pub fn reuse(&mut self, id: usize) {
        if let Some(heights) = &mut self.heights {
            heights.remove(&id);
        }
        // Preserve Go's size entry across allocator reuse.
        self.free.push(id);
    }
    fn assign(
        &mut self,
        id: usize,
        kind: Kind,
        runes: Vec<u32>,
        children: Vec<usize>,
        flags: u8,
        bounds: (u64, Option<u64>),
    ) {
        let (min, max) = bounds;
        self.nodes[id] = self.build(kind, runes, children, flags, min, max);
    }
    fn cached(&mut self, root: usize, height: bool) -> u64 {
        let mut pending = vec![(root, true, false)];
        while let Some((id, force, expanded)) = pending.pop() {
            let cache = if height {
                self.heights.as_ref().unwrap()
            } else {
                self.sizes.as_ref().unwrap()
            };
            if !force && cache.contains_key(&id) {
                continue;
            }
            let n = &self.nodes[id];
            if !expanded {
                pending.push((id, force, true));
                for &child in n.children.iter().rev() {
                    pending.push((child, false, false));
                }
                continue;
            }
            let cache = if height {
                self.heights.as_ref().unwrap()
            } else {
                self.sizes.as_ref().unwrap()
            };
            let value = if height {
                1 + n.children.iter().map(|c| cache[c]).max().unwrap_or(0)
            } else {
                let sum = n.children.iter().map(|c| cache[c]).sum::<u64>();
                match n.kind {
                    Kind::Literal => n.runes.len() as u64,
                    Kind::Capture | Kind::Star => sum.saturating_add(2),
                    Kind::Plus | Kind::Quest => sum.saturating_add(1),
                    Kind::Concat => sum,
                    Kind::Alt => sum.saturating_add(n.children.len().saturating_sub(1) as u64),
                    Kind::Repeat => match n.max {
                        Some(m) => sum.saturating_mul(m).saturating_add(m - n.min),
                        None if n.min == 0 => sum.saturating_add(2),
                        None => sum.saturating_mul(n.min).saturating_add(1),
                    },
                    _ => 1,
                }
                .max(1)
            };
            if height {
                self.heights.as_mut().unwrap().insert(id, value);
            } else {
                self.sizes.as_mut().unwrap().insert(id, value);
            }
        }
        if height {
            self.heights.as_ref().unwrap()[&root]
        } else {
            self.sizes.as_ref().unwrap()[&root]
        }
    }
    pub fn limits(&mut self, id: usize, stack: &[usize]) -> Result<(), &'static str> {
        if let Some(error) = self.failure {
            return Err(error);
        }
        if self.num_runes > 33554432 {
            self.failure = Some("expression too large");
        }
        if self.sizes.is_none() {
            self.repeats = self.repeats.max(1);
            let n = &self.nodes[id];
            if n.kind == Kind::Repeat {
                let count = n.max.unwrap_or(n.min).max(1);
                self.repeats = if count > 3355443 / self.repeats {
                    3355443
                } else {
                    self.repeats * count
                };
            }
            if self.nodes.len() as u64 >= 3355443 / self.repeats {
                self.sizes = Some(BTreeMap::new());
                for &node in stack {
                    if self.cached(node, false) > 3355443 {
                        self.failure = Some("expression too large");
                        break;
                    }
                }
            }
        }
        if self.failure.is_none() && self.sizes.is_some() && self.cached(id, false) > 3355443 {
            self.failure = Some("expression too large");
        }
        if self.failure.is_none() && self.nodes.len() >= 1000 {
            if self.heights.is_none() {
                self.heights = Some(BTreeMap::new());
                for &node in stack {
                    if self.cached(node, true) > 1000 {
                        self.failure = Some("expression nests too deeply");
                        break;
                    }
                }
            }
            if self.failure.is_none() && self.cached(id, true) > 1000 {
                self.failure = Some("expression nests too deeply");
            }
        }
        self.failure.map_or(Ok(()), Err)
    }
    fn check_factored(&mut self, id: usize) {
        let context = self.context.clone();
        let _ = self.limits(id, &context);
    }
    pub fn capture(&mut self, id: usize, child: usize) {
        let flags = self.nodes[id].flags;
        self.assign(id, Kind::Capture, vec![], vec![child], flags, (0, None));
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
            let r = &self.nodes[id].runes;
            let folded = r.len() == 4
                && r[0] == r[1]
                && r[2] == r[3]
                && fold(r[0]) == r[2]
                && fold(r[2]) == r[0]
                || r.len() == 2 && r[0] + 1 == r[1] && fold(r[0]) == r[1] && fold(r[1]) == r[0];
            if folded || r.len() == 2 && r[0] == r[1] {
                let rune = r[0];
                self.assign(
                    id,
                    Kind::Literal,
                    vec![rune],
                    vec![],
                    u8::from(folded),
                    (0, None),
                );
            }
        }
        (id, cost)
    }
    fn clean_alt(&mut self, id: usize) -> usize {
        let n = &self.nodes[id];
        if n.kind == Kind::Class {
            let r = clean(&n.runes);
            let kind = if r == [0, 0x10ffff] {
                Kind::Any
            } else if r == [0, 9, 11, 0x10ffff] {
                Kind::AnyNotNL
            } else {
                Kind::Class
            };
            self.assign(
                id,
                kind,
                if kind == Kind::Class { r } else { vec![] },
                vec![],
                0,
                (0, None),
            );
        }
        id
    }
    pub fn class(&mut self, runes: Vec<u32>) -> usize {
        self.node(Kind::Class, clean(&runes), vec![], 0, 0, None)
    }
    pub fn maybe_concat(&mut self, nodes: &mut Vec<usize>, incoming: Option<(u32, u8)>) -> bool {
        let n = nodes.len();
        if n < 2 {
            return false;
        }
        let first = nodes[n - 2];
        let second = nodes[n - 1];
        if self.nodes[first].kind == Kind::Literal
            && self.nodes[second].kind == Kind::Literal
            && self.nodes[first].flags & 1 == self.nodes[second].flags & 1
        {
            let runes = std::mem::take(&mut self.nodes[second].runes);
            self.nodes[first].runes.extend(runes);
            if let Some((r, flags)) = incoming {
                self.assign(second, Kind::Literal, vec![r], vec![], flags, (0, None));
                return true;
            }
            nodes.pop();
            self.reuse(second);
        }
        false
    }
    pub fn merge_literals(&mut self, nodes: &mut Vec<usize>) {
        self.maybe_concat(nodes, None);
    }
    pub fn collapse(&mut self, kind: Kind, children: Vec<usize>) -> usize {
        if children.is_empty() {
            return self.empty();
        }
        if children.len() == 1 {
            return children[0];
        }
        let id = self.node(kind, vec![], vec![], 0, 0, None);
        let mut flat = Vec::new();
        for child in children {
            if self.nodes[child].kind == kind {
                flat.extend(self.nodes[child].children.clone());
                self.reuse(child);
            } else {
                flat.push(child);
            }
        }
        if kind == Kind::Alt {
            flat = self.factor(flat);
        }
        if kind == Kind::Alt && flat.len() == 1 {
            self.reuse(id);
            flat[0]
        } else {
            self.assign(id, kind, vec![], flat, 0, (0, None));
            id
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
        if self.nodes[id].kind == Kind::Concat {
            let child = self.nodes[id].children[0];
            let child = self.remove_string(child, len);
            self.nodes[id].children[0] = child;
            if self.nodes[child].kind == Kind::Empty {
                self.reuse(child);
                if self.nodes[id].children.len() == 2 {
                    let next = self.nodes[id].children[1];
                    self.reuse(id);
                    return next;
                }
                self.nodes[id].children.remove(0);
            }
            let n = self.nodes[id].clone();
            self.assign(id, n.kind, n.runes, n.children, n.flags, (n.min, n.max));
        } else if self.nodes[id].kind == Kind::Literal {
            let runes = self.nodes[id].runes[len..].to_vec();
            let kind = if runes.is_empty() {
                Kind::Empty
            } else {
                Kind::Literal
            };
            let flags = self.nodes[id].flags;
            self.assign(id, kind, runes, vec![], flags, (0, None));
        }
        id
    }
    fn remove_node(&mut self, id: usize, reuse: bool) -> usize {
        if self.nodes[id].kind == Kind::Concat {
            if reuse {
                self.reuse(self.nodes[id].children[0]);
            }
            let children = self.nodes[id].children[1..].to_vec();
            if children.len() == 1 {
                let child = children[0];
                self.reuse(id);
                return child;
            }
            self.assign(
                id,
                if children.is_empty() {
                    Kind::Empty
                } else {
                    Kind::Concat
                },
                vec![],
                children,
                0,
                (0, None),
            );
            id
        } else {
            if reuse {
                self.reuse(id);
            }
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
                    .map(|&id| {
                        let id = self.remove_string(id, prefix.len());
                        self.check_factored(id);
                        id
                    })
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
                    .enumerate()
                    .map(|(i, &id)| {
                        let id = self.remove_node(id, i != 0);
                        self.check_factored(id);
                        id
                    })
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
                while i < children.len() && self.is_class(children[i]) {
                    i += 1;
                }
                let mut run = children[start..i].to_vec();
                if run.len() > 1 {
                    let mut most = 0;
                    for j in 1..run.len() {
                        if self.rank(run[j]) > self.rank(run[most])
                            || self.rank(run[j]) == self.rank(run[most])
                                && self.nodes[run[j]].runes.len()
                                    > self.nodes[run[most]].runes.len()
                        {
                            most = j;
                        }
                    }
                    run.swap(0, most);
                    for &other in &run[1..] {
                        self.merge_class(run[0], other);
                        self.reuse(other);
                    }
                    self.clean_alt(run[0]);
                }
                out.push(run[0]);
            } else {
                out.push(children[i]);
                i += 1;
            }
        }
        let mut final_nodes = Vec::new();
        for (i, &id) in out.iter().enumerate() {
            if self.nodes[id].kind == Kind::Empty
                && out
                    .get(i + 1)
                    .is_some_and(|&next| self.nodes[next].kind == Kind::Empty)
            {
                continue;
            }
            final_nodes.push(id);
        }
        final_nodes
    }
    fn rank(&self, id: usize) -> u8 {
        match self.nodes[id].kind {
            Kind::Literal => 3,
            Kind::Class => 4,
            Kind::AnyNotNL => 5,
            Kind::Any => 6,
            _ => 0,
        }
    }
    fn merge_class(&mut self, dst: usize, src: usize) {
        match self.nodes[dst].kind {
            Kind::Any => {}
            Kind::AnyNotNL => {
                if self
                    .as_ranges(src)
                    .chunks_exact(2)
                    .any(|p| p[0] <= 10 && p[1] >= 10)
                {
                    self.nodes[dst].kind = Kind::Any;
                }
            }
            Kind::Literal
                if self.nodes[src].kind == Kind::Literal
                    && self.nodes[dst].runes == self.nodes[src].runes
                    && self.nodes[dst].flags == self.nodes[src].flags => {}
            Kind::Class => {
                let source = self.as_ranges(src);
                for p in source.chunks_exact(2) {
                    append(&mut self.nodes[dst].runes, p[0], p[1]);
                }
            }
            _ => {
                let mut r = self.as_ranges(dst);
                for p in self.as_ranges(src).chunks_exact(2) {
                    append(&mut r, p[0], p[1]);
                }
                self.nodes[dst].kind = Kind::Class;
                self.nodes[dst].runes = r;
            }
        }
    }
    pub fn swap_bar(&mut self, stack: &mut Vec<usize>) -> bool {
        let n = stack.len();
        if n >= 3
            && self.nodes[stack[n - 2]].kind == Kind::VerticalBar
            && self.is_class(stack[n - 1])
            && self.is_class(stack[n - 3])
        {
            let mut src = stack[n - 1];
            let mut dst = stack[n - 3];
            if self.rank(src) > self.rank(dst) {
                std::mem::swap(&mut src, &mut dst);
                stack[n - 3] = dst;
            }
            self.merge_class(dst, src);
            self.reuse(src);
            stack.pop();
            return true;
        }
        if n >= 2 && self.nodes[stack[n - 2]].kind == Kind::VerticalBar {
            if n >= 3 {
                self.clean_alt(stack[n - 3]);
            }
            stack.swap(n - 1, n - 2);
            return true;
        }
        false
    }
    pub fn clean_last(&mut self, children: &[usize]) {
        if let Some(&id) = children.last() {
            self.clean_alt(id);
        }
    }
    pub fn pseudo(&self, id: usize) -> bool {
        matches!(self.nodes[id].kind, Kind::LeftParen | Kind::VerticalBar)
    }
}
