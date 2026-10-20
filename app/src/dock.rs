//! Free-form docking: a tree of horizontal/vertical splits whose leaves are tab groups of
//! panes. Pure data (serialisable, UI-independent) so layouts can be saved, tested and
//! restored; the app maps each visible group to a screen rectangle with [`Node::layout`].

use std::collections::HashSet;

use egui::{Pos2, Rect, pos2, vec2};
use serde::{Deserialize, Serialize};

use crate::layout::Layout;

pub type PaneId = u64;

/// A path of child indices from the root to a node.
pub type Path = Vec<usize>;

/// Split direction: `Row` lays children left to right, `Column` top to bottom.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Dir {
    Row,
    Column,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Node {
    /// A tab group; only the active pane is visible.
    Tabs {
        panes: Vec<PaneId>,
        #[serde(default)]
        active: usize,
    },
    /// Children share the space along `dir` by `shares` (fractions summing to 1).
    Split { dir: Dir, children: Vec<Node>, shares: Vec<f32> },
}

/// Where a dragged pane lands relative to a group.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Zone {
    /// Add as a tab.
    Center,
    Left,
    Right,
    Top,
    Bottom,
}

impl Zone {
    fn split(self) -> Option<(Dir, bool)> {
        match self {
            Zone::Center => None,
            Zone::Left => Some((Dir::Row, true)),
            Zone::Right => Some((Dir::Row, false)),
            Zone::Top => Some((Dir::Column, true)),
            Zone::Bottom => Some((Dir::Column, false)),
        }
    }

    /// Which zone of `rect` the pointer is over: the middle half is `Center`, otherwise the
    /// nearest edge.
    pub fn at(rect: Rect, p: Pos2) -> Zone {
        let u = (p.x - rect.left()) / rect.width().max(1.0);
        let v = (p.y - rect.top()) / rect.height().max(1.0);
        if (0.25..=0.75).contains(&u) && (0.25..=0.75).contains(&v) {
            return Zone::Center;
        }
        let d = [(u, Zone::Left), (1.0 - u, Zone::Right), (v, Zone::Top), (1.0 - v, Zone::Bottom)];
        d.into_iter().min_by(|a, b| a.0.total_cmp(&b.0)).map_or(Zone::Center, |x| x.1)
    }

    /// The area the pane would occupy if dropped here.
    pub fn preview(self, rect: Rect) -> Rect {
        let (w, h) = (rect.width() / 2.0, rect.height() / 2.0);
        match self {
            Zone::Center => rect,
            Zone::Left => Rect::from_min_size(rect.min, vec2(w, rect.height())),
            Zone::Right => Rect::from_min_max(pos2(rect.left() + w, rect.top()), rect.max),
            Zone::Top => Rect::from_min_size(rect.min, vec2(rect.width(), h)),
            Zone::Bottom => Rect::from_min_max(pos2(rect.left(), rect.top() + h), rect.max),
        }
    }
}

/// A visible tab group and its screen rectangle.
#[derive(Debug, Clone, PartialEq)]
pub struct Group {
    pub path: Path,
    pub rect: Rect,
    pub panes: Vec<PaneId>,
    pub active: usize,
}

impl Group {
    pub fn active_pane(&self) -> Option<PaneId> {
        self.panes.get(self.active).copied()
    }
}

/// A draggable boundary between children `index` and `index + 1` of the split at `path`.
#[derive(Debug, Clone, PartialEq)]
pub struct Divider {
    pub path: Path,
    pub index: usize,
    pub dir: Dir,
    pub rect: Rect,
    /// Pixels shared by the split's children (excluding gaps), to convert drags to shares.
    pub span: f32,
}

/// Smallest size a pane may be resized to, in points.
pub const MIN_PANE: f32 = 90.0;

impl Default for Node {
    fn default() -> Self {
        Node::Tabs { panes: Vec::new(), active: 0 }
    }
}

impl Node {
    pub fn leaf(id: PaneId) -> Node {
        Node::Tabs { panes: vec![id], active: 0 }
    }

    pub fn split(dir: Dir, children: Vec<Node>) -> Node {
        let n = children.len().max(1);
        Node::Split { dir, shares: vec![1.0 / n as f32; children.len()], children }
    }

    pub fn split_with(dir: Dir, children: Vec<Node>, shares: Vec<f32>) -> Node {
        let mut n = Node::Split { dir, children, shares };
        n.normalize();
        n
    }

    /// Every pane in the tree, depth first.
    pub fn panes(&self) -> Vec<PaneId> {
        let mut out = Vec::new();
        self.walk(&mut |n| {
            if let Node::Tabs { panes, .. } = n {
                out.extend(panes.iter().copied());
            }
        });
        out
    }

    /// The visible (active-tab) panes, in layout order.
    pub fn visible(&self) -> Vec<PaneId> {
        let mut out = Vec::new();
        self.walk(&mut |n| {
            if let Node::Tabs { panes, active } = n
                && let Some(&p) = panes.get(*active)
            {
                out.push(p);
            }
        });
        out
    }

    fn walk(&self, f: &mut impl FnMut(&Node)) {
        f(self);
        if let Node::Split { children, .. } = self {
            for c in children {
                c.walk(f);
            }
        }
    }

    pub fn contains(&self, id: PaneId) -> bool {
        self.find(id).is_some()
    }

    pub fn is_empty(&self) -> bool {
        self.panes().is_empty()
    }

    /// Path to the tab group holding `id`.
    pub fn find(&self, id: PaneId) -> Option<Path> {
        match self {
            Node::Tabs { panes, .. } => panes.contains(&id).then(Vec::new),
            Node::Split { children, .. } => children.iter().enumerate().find_map(|(i, c)| {
                let mut p = c.find(id)?;
                p.insert(0, i);
                Some(p)
            }),
        }
    }

    pub fn get(&self, path: &[usize]) -> Option<&Node> {
        match (path.split_first(), self) {
            (None, n) => Some(n),
            (Some((i, rest)), Node::Split { children, .. }) => children.get(*i)?.get(rest),
            _ => None,
        }
    }

    pub fn get_mut(&mut self, path: &[usize]) -> Option<&mut Node> {
        match path.split_first() {
            None => Some(self),
            Some((i, rest)) => match self {
                Node::Split { children, .. } => children.get_mut(*i)?.get_mut(rest),
                Node::Tabs { .. } => None,
            },
        }
    }

    /// Make `id` the active tab of its group.
    pub fn activate(&mut self, id: PaneId) -> bool {
        let Some(path) = self.find(id) else { return false };
        if let Some(Node::Tabs { panes, active }) = self.get_mut(&path) {
            *active = panes.iter().position(|&p| p == id).unwrap_or(0);
            return true;
        }
        false
    }

    /// Swap pane `old` for `new` in place (same tab slot).
    pub fn replace(&mut self, old: PaneId, new: PaneId) -> bool {
        let Some(path) = self.find(old) else { return false };
        if let Some(Node::Tabs { panes, .. }) = self.get_mut(&path) {
            for p in panes.iter_mut().filter(|p| **p == old) {
                *p = new;
            }
            return true;
        }
        false
    }

    /// Rewrite every pane id through `f`, dropping panes it maps to `None`.
    pub fn remap(&mut self, f: &impl Fn(PaneId) -> Option<PaneId>) {
        match self {
            Node::Tabs { panes, .. } => *panes = panes.iter().filter_map(|&p| f(p)).collect(),
            Node::Split { children, .. } => children.iter_mut().for_each(|c| c.remap(f)),
        }
        self.normalize();
    }

    /// Remove `id` and tidy the tree. Returns whether it was present.
    pub fn remove(&mut self, id: PaneId) -> bool {
        let found = self.remove_raw(id);
        self.normalize();
        found
    }

    fn remove_raw(&mut self, id: PaneId) -> bool {
        match self {
            Node::Tabs { panes, active } => {
                let Some(i) = panes.iter().position(|&p| p == id) else { return false };
                panes.remove(i);
                if *active > i || *active >= panes.len() {
                    *active = active.saturating_sub(1);
                }
                true
            }
            Node::Split { children, .. } => children.iter_mut().any(|c| c.remove_raw(id)),
        }
    }

    /// Dock `id` next to (or as a tab with) the group holding `target`.
    pub fn insert(&mut self, target: PaneId, zone: Zone, id: PaneId) -> bool {
        if target == id {
            return false;
        }
        let Some(path) = self.find(target) else { return false };
        self.insert_at(&path, zone, id);
        true
    }

    /// Dock `id` relative to the node at `path` (`[]` = the whole window).
    pub fn insert_at(&mut self, path: &[usize], zone: Zone, id: PaneId) {
        self.remove_raw(id);
        let Some(node) = self.get_mut(path) else { return };
        match zone.split() {
            None => match node {
                Node::Tabs { panes, active } => {
                    panes.push(id);
                    *active = panes.len() - 1;
                }
                // Tabbing onto a split: add to its first group.
                Node::Split { children, .. } => {
                    let mut first = &mut children[0];
                    while let Node::Split { children, .. } = first {
                        first = &mut children[0];
                    }
                    if let Node::Tabs { panes, active } = first {
                        panes.push(id);
                        *active = panes.len() - 1;
                    }
                }
            },
            Some((dir, before)) => {
                // Empty group: just take its place.
                if matches!(node, Node::Tabs { panes, .. } if panes.is_empty()) {
                    *node = Node::leaf(id);
                } else {
                    let old = std::mem::take(node);
                    let children = if before { vec![Node::leaf(id), old] } else { vec![old, Node::leaf(id)] };
                    *node = Node::split(dir, children);
                }
            }
        }
        self.normalize();
    }

    /// Remove empty groups, collapse single-child splits, flatten nested splits in the same
    /// direction and renormalise shares. Keeps an empty root group if nothing is left.
    pub fn normalize(&mut self) {
        if let Node::Split { dir, children, shares } = self {
            shares.resize(children.len(), 0.0);
            let mut kids = Vec::new();
            let mut ks = Vec::new();
            for (mut c, s) in std::mem::take(children).into_iter().zip(std::mem::take(shares)) {
                c.normalize();
                if c.is_empty() {
                    continue;
                }
                let s = if s.is_finite() && s > 0.0 { s } else { 0.0 };
                match c {
                    Node::Split { dir: d, children: cc, shares: cs } if d == *dir => {
                        let total: f32 = cs.iter().sum::<f32>().max(1e-6);
                        for (g, gs) in cc.into_iter().zip(cs) {
                            kids.push(g);
                            ks.push(s * gs / total);
                        }
                    }
                    c => {
                        kids.push(c);
                        ks.push(s);
                    }
                }
            }
            let n = kids.len();
            let total: f32 = ks.iter().sum();
            if total <= 1e-6 {
                ks = vec![1.0 / n.max(1) as f32; n];
            } else {
                for s in &mut ks {
                    *s /= total;
                }
            }
            match n {
                0 => *self = Node::default(),
                1 => *self = kids.pop().expect("one child"),
                _ => {
                    *children = kids;
                    *shares = ks;
                }
            }
        }
        if let Node::Tabs { panes, active } = self {
            let mut seen = HashSet::new();
            panes.retain(|p| seen.insert(*p));
            *active = (*active).min(panes.len().saturating_sub(1));
        }
    }

    /// Drop panes not in `known` and duplicates across groups (first occurrence wins).
    pub fn retain_known(&mut self, known: &HashSet<PaneId>) {
        let mut seen = HashSet::new();
        self.retain_inner(known, &mut seen);
        self.normalize();
    }

    fn retain_inner(&mut self, known: &HashSet<PaneId>, seen: &mut HashSet<PaneId>) {
        match self {
            Node::Tabs { panes, .. } => panes.retain(|p| known.contains(p) && seen.insert(*p)),
            Node::Split { children, .. } => children.iter_mut().for_each(|c| c.retain_inner(known, seen)),
        }
    }

    /// Screen rectangles for every tab group, plus the dividers between split children.
    pub fn layout(&self, rect: Rect, gap: f32) -> (Vec<Group>, Vec<Divider>) {
        let mut groups = Vec::new();
        let mut dividers = Vec::new();
        self.layout_inner(rect, gap, &mut Vec::new(), &mut groups, &mut dividers);
        (groups, dividers)
    }

    fn layout_inner(&self, rect: Rect, gap: f32, path: &mut Path, groups: &mut Vec<Group>, divs: &mut Vec<Divider>) {
        match self {
            Node::Tabs { panes, active } => {
                groups.push(Group { path: path.clone(), rect, panes: panes.clone(), active: *active })
            }
            Node::Split { dir, children, shares } => {
                let n = children.len();
                let len = match dir {
                    Dir::Row => rect.width(),
                    Dir::Column => rect.height(),
                };
                let span = (len - gap * (n.saturating_sub(1)) as f32).max(0.0);
                let mut at = 0.0;
                for (i, c) in children.iter().enumerate() {
                    let size = if i + 1 == n { span - at + 0.0 } else { (span * shares[i]).round() };
                    let offset = at + gap * i as f32;
                    let r = match dir {
                        Dir::Row => {
                            Rect::from_min_size(pos2(rect.left() + offset, rect.top()), vec2(size, rect.height()))
                        }
                        Dir::Column => {
                            Rect::from_min_size(pos2(rect.left(), rect.top() + offset), vec2(rect.width(), size))
                        }
                    };
                    path.push(i);
                    c.layout_inner(r, gap, path, groups, divs);
                    path.pop();
                    if i + 1 < n {
                        let d = match dir {
                            Dir::Row => Rect::from_min_size(pos2(r.right(), rect.top()), vec2(gap, rect.height())),
                            Dir::Column => Rect::from_min_size(pos2(rect.left(), r.bottom()), vec2(rect.width(), gap)),
                        };
                        divs.push(Divider { path: path.clone(), index: i, dir: *dir, rect: d, span });
                    }
                    at += size;
                }
            }
        }
    }

    /// Move the divider after child `index` of the split at `path` by `delta` points.
    pub fn resize(&mut self, path: &[usize], index: usize, delta: f32, span: f32) {
        let Some(Node::Split { shares, .. }) = self.get_mut(path) else { return };
        if index + 1 >= shares.len() || span <= 0.0 {
            return;
        }
        let min = (MIN_PANE / span).min(0.45);
        let pair = shares[index] + shares[index + 1];
        let a = (shares[index] + delta / span).clamp(min.min(pair / 2.0), (pair - min).max(pair / 2.0));
        shares[index] = a;
        shares[index + 1] = pair - a;
    }

    /// Give every child of the split at `path` an equal share.
    pub fn equalize(&mut self, path: &[usize]) {
        if let Some(Node::Split { shares, .. }) = self.get_mut(path) {
            let n = shares.len() as f32;
            shares.iter_mut().for_each(|s| *s = 1.0 / n);
        }
    }

    /// Build a preset arrangement from `panes`; panes beyond the preset's slots become extra
    /// tabs in the last slot.
    pub fn preset(layout: Layout, panes: &[PaneId]) -> Node {
        let slots = layout.slots().min(panes.len().max(1));
        let mut groups: Vec<Node> = panes.iter().take(slots).map(|&p| Node::leaf(p)).collect();
        if groups.is_empty() {
            return Node::default();
        }
        if let Some(Node::Tabs { panes: last, .. }) = groups.last_mut() {
            last.extend(panes.iter().skip(slots).copied());
        }
        let mut take = |n: usize| groups.drain(..n.min(groups.len())).collect::<Vec<_>>();
        let node = match layout {
            Layout::One => take(1).pop().expect("one group"),
            Layout::TwoCols => Node::split(Dir::Row, take(2)),
            Layout::TwoRows => Node::split(Dir::Column, take(2)),
            Layout::MainRight2 => {
                let main = take(1);
                let right = take(2);
                let rest = Node::split(Dir::Column, right);
                Node::split_with(Dir::Row, main.into_iter().chain([rest]).collect(), vec![0.62, 0.38])
            }
            Layout::Grid2x2 => {
                let top = Node::split(Dir::Row, take(2));
                let bottom = Node::split(Dir::Row, take(2));
                Node::split(Dir::Column, vec![top, bottom])
            }
            Layout::Grid3x2 => {
                let top = Node::split(Dir::Row, take(3));
                let bottom = Node::split(Dir::Row, take(3));
                Node::split(Dir::Column, vec![top, bottom])
            }
        };
        let mut node = node;
        node.normalize();
        node
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn area() -> Rect {
        Rect::from_min_size(pos2(0.0, 0.0), vec2(1000.0, 600.0))
    }

    #[test]
    fn insert_split_tab_and_remove() {
        let mut n = Node::leaf(1);
        assert!(n.insert(1, Zone::Right, 2));
        assert!(matches!(&n, Node::Split { dir: Dir::Row, children, .. } if children.len() == 2));
        // Another Right split of pane 2 flattens into the same row.
        n.insert(2, Zone::Right, 3);
        let Node::Split { children, shares, .. } = &n else { panic!() };
        assert_eq!(children.len(), 3);
        assert!((shares.iter().sum::<f32>() - 1.0).abs() < 1e-5);
        n.insert(3, Zone::Bottom, 4);
        n.insert(1, Zone::Center, 5);
        assert_eq!(n.find(5), n.find(1));
        assert_eq!(n.visible(), vec![5, 2, 3, 4]);
        assert!(n.activate(1));
        assert_eq!(n.visible()[0], 1);
        assert_eq!(n.panes().len(), 5);
        // Removing collapses empty groups and single-child splits.
        for id in [4, 3, 2] {
            assert!(n.remove(id));
        }
        assert_eq!(n, Node::Tabs { panes: vec![1, 5], active: 0 });
        assert!(!n.remove(42));
        n.remove(1);
        n.remove(5);
        assert!(n.is_empty());
        assert_eq!(n, Node::default());
    }

    #[test]
    fn moving_a_pane_onto_itself_or_its_group() {
        let mut n = Node::preset(Layout::TwoCols, &[1, 2]);
        assert!(!n.insert(1, Zone::Left, 1));
        // Dragging pane 2 to the left edge of pane 1's group swaps their order.
        n.insert(1, Zone::Left, 2);
        assert_eq!(n.visible(), vec![2, 1]);
        // Docking into an empty root replaces it.
        let mut e = Node::default();
        e.insert_at(&[], Zone::Right, 7);
        assert_eq!(e, Node::leaf(7));
    }

    #[test]
    fn layout_tiles_area_and_reports_dividers() {
        for l in Layout::ALL {
            let ids: Vec<PaneId> = (1..=l.slots() as u64).collect();
            let n = Node::preset(l, &ids);
            let (groups, divs) = n.layout(area(), 6.0);
            assert_eq!(groups.len(), l.slots(), "{l:?}");
            let covered: f32 =
                groups.iter().map(|g| g.rect.area()).sum::<f32>() + divs.iter().map(|d| d.rect.area()).sum::<f32>();
            assert!(covered <= area().area() + 1.0, "{l:?}");
            for g in &groups {
                assert!(area().expand(0.5).contains_rect(g.rect), "{l:?} {:?}", g.rect);
            }
            for (i, a) in groups.iter().enumerate() {
                for b in &groups[i + 1..] {
                    assert!(a.rect.intersect(b.rect).area() < 1.0, "{l:?} overlap");
                }
            }
        }
    }

    #[test]
    fn preset_puts_extra_panes_in_tabs() {
        let n = Node::preset(Layout::TwoCols, &[1, 2, 3, 4]);
        assert_eq!(n.visible(), vec![1, 2]);
        assert_eq!(n.panes(), vec![1, 2, 3, 4]);
        assert_eq!(Node::preset(Layout::Grid2x2, &[1]).panes(), vec![1]);
        assert_eq!(Node::preset(Layout::One, &[]), Node::default());
    }

    #[test]
    fn resize_clamps_and_equalize() {
        let mut n = Node::preset(Layout::TwoCols, &[1, 2]);
        let (_, divs) = n.layout(area(), 6.0);
        let d = &divs[0];
        n.resize(&d.path, d.index, 200.0, d.span);
        let Node::Split { shares, .. } = &n else { panic!() };
        assert!((shares[0] - (0.5 + 200.0 / d.span)).abs() < 1e-4);
        n.resize(&d.path, d.index, 10_000.0, d.span);
        let (groups, _) = n.layout(area(), 6.0);
        assert!(groups[1].rect.width() >= MIN_PANE - 1.0);
        n.equalize(&[]);
        let Node::Split { shares, .. } = &n else { panic!() };
        assert_eq!(shares, &vec![0.5, 0.5]);
    }

    #[test]
    fn zones_and_previews() {
        let r = area();
        assert_eq!(Zone::at(r, r.center()), Zone::Center);
        assert_eq!(Zone::at(r, pos2(10.0, 300.0)), Zone::Left);
        assert_eq!(Zone::at(r, pos2(990.0, 300.0)), Zone::Right);
        assert_eq!(Zone::at(r, pos2(500.0, 5.0)), Zone::Top);
        assert_eq!(Zone::at(r, pos2(500.0, 595.0)), Zone::Bottom);
        assert_eq!(Zone::Right.preview(r).left(), 500.0);
        assert_eq!(Zone::Center.preview(r), r);
    }

    #[test]
    fn json_round_trip_and_repair() {
        let mut n = Node::preset(Layout::MainRight2, &[1, 2, 3]);
        n.insert(2, Zone::Center, 9);
        let json = serde_json::to_string_pretty(&n).unwrap();
        assert!(json.contains("\"type\": \"split\"") && json.contains("\"dir\": \"row\""));
        let back: Node = serde_json::from_str(&json).unwrap();
        assert_eq!(back, n);
        // Hand-edited JSON with unknown/duplicate panes and bad shares is repaired.
        let raw = r#"{"type":"split","dir":"column","shares":[0,0,0],"children":[
            {"type":"tabs","panes":[1,1,77]},{"type":"tabs","panes":[2],"active":9},{"type":"tabs","panes":[]}]}"#;
        let mut n: Node = serde_json::from_str(raw).unwrap();
        n.retain_known(&HashSet::from([1, 2]));
        assert_eq!(n.panes(), vec![1, 2]);
        let Node::Split { shares, children, .. } = &n else { panic!() };
        assert_eq!(shares, &vec![0.5, 0.5]);
        assert_eq!(children[1], Node::leaf(2));
        assert!(n.replace(2, 20));
        assert_eq!(n.panes(), vec![1, 20]);
        n.remap(&|p| (p != 1).then_some(p + 100));
        assert_eq!(n, Node::leaf(120));
    }
}
