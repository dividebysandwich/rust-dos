//! A drive's directory tree, for the Tree panel and the NCD tree.

use crate::host::Host;
use crate::panel::{FRAME, HEADING, Layout, MARKED_CURSOR};
use crate::path;
use crate::screen::{Screen, pad};

/// How deep and how big a tree gets, so that a huge host folder (or one
/// that links back into itself) can't hold RC up.
const MAX_DEPTH: usize = 32;
const MAX_NODES: usize = 20_000;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Node {
    pub path: String,
    pub name: String,
    /// For each level above: whether a line goes on down there.
    pub lines: Vec<bool>,
    pub last: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Tree {
    pub drive: u8,
    pub nodes: Vec<Node>,
    pub truncated: bool,
}

impl Tree {
    /// Read the tree of a drive, depth first, its directories by name.
    pub fn read(host: &mut dyn Host, drive: u8) -> Tree {
        let root = path::root(drive);
        let mut tree = Tree {
            drive,
            nodes: vec![Node {
                path: root.clone(),
                name: "\\".to_string(),
                lines: Vec::new(),
                last: true,
            }],
            truncated: false,
        };
        // Each level's directories still to go: (path, lines above).
        let mut stack: Vec<(Vec<String>, Vec<bool>)> = vec![(subdirs(host, &root), Vec::new())];
        while let Some((rest, lines)) = stack.last_mut() {
            if rest.is_empty() {
                stack.pop();
                continue;
            }
            let dir = rest.remove(0);
            let last = rest.is_empty();
            let lines = lines.clone();
            if tree.nodes.len() >= MAX_NODES {
                tree.truncated = true;
                break;
            }
            tree.nodes.push(Node {
                name: path::name(&dir).to_string(),
                path: dir.clone(),
                lines: lines.clone(),
                last,
            });
            if lines.len() + 1 < MAX_DEPTH {
                let mut below = lines;
                below.push(!last);
                stack.push((subdirs(host, &dir), below));
            } else {
                tree.truncated = true;
            }
        }
        tree
    }

    /// The node of a directory, or the nearest one above it.
    pub fn find(&self, dir: &str) -> usize {
        let mut dir = dir.to_string();
        loop {
            if let Some(i) = self
                .nodes
                .iter()
                .position(|n| n.path.eq_ignore_ascii_case(&dir))
            {
                return i;
            }
            match path::parent(&dir) {
                Some(up) => dir = up,
                None => return 0,
            }
        }
    }

    /// A node's line: the lines of the levels above, then its own.
    pub fn line(&self, i: usize) -> String {
        let n = &self.nodes[i];
        if i == 0 {
            return "\\".to_string();
        }
        let mut out = String::new();
        for &on in &n.lines {
            out.push_str(if on { "│  " } else { "   " });
        }
        out.push_str(if n.last { "└──" } else { "├──" });
        out.push_str(&n.name);
        out
    }

    /// The Tree panel's rows, `top` the first shown; the cursor marked as
    /// NC marks it, highlighted when `active`, in brackets when not.
    pub fn draw(&self, s: &mut Screen, l: &Layout, cursor: usize, top: usize, active: bool) {
        let rows = l.list + 1;
        let w = l.w - 2;
        for r in 0..rows {
            let i = top + r;
            if i >= self.nodes.len() {
                break;
            }
            let text = format!(" {}", self.line(i));
            let bytes = pad(&crate::cp437::bytes(&text), w);
            s.text(1 + r, l.x + 1, &bytes, FRAME);
            if i != cursor {
                continue;
            }
            // The name, with the cell before it, 14 wide.
            let lead = text.chars().count()
                - self.nodes[i].name.chars().count()
                - if i == 0 { 0 } else { 1 };
            let lead = if i == 0 { 0 } else { lead };
            let name = &self.nodes[i].name;
            let at = l.x + 1 + lead;
            if active {
                s.text(
                    1 + r,
                    at,
                    &pad(format!(" {}", name).as_bytes(), 14),
                    MARKED_CURSOR,
                );
                s.set(1 + r, l.x + l.w - 2, 0x11, HEADING);
            } else {
                let mut b = vec![b'['];
                b.extend(pad(name.as_bytes(), 12));
                b.push(b']');
                s.text(1 + r, at, &b, FRAME);
                s.set(1 + r, l.x + l.w - 2, 0x11, FRAME);
            }
        }
        if l.status {
            let dir = self
                .nodes
                .get(cursor)
                .map_or(String::new(), |n| n.path.clone());
            s.text(l.h - 2, l.x + 1, &pad(&crate::cp437::bytes(&dir), w), FRAME);
        }
    }

    /// The lines of the whole tree, for a list in a dialog.
    pub fn lines(&self) -> Vec<String> {
        let mut out: Vec<String> = (0..self.nodes.len()).map(|i| self.line(i)).collect();
        if self.truncated {
            out.push("(more directories not shown)".to_string());
        }
        out
    }
}

/// A directory's directories, by name.
fn subdirs(host: &mut dyn Host, dir: &str) -> Vec<String> {
    let mut names: Vec<String> = host
        .list(dir)
        .unwrap_or_default()
        .into_iter()
        .filter(|e| e.is_dir() && e.name != "." && e.name != "..")
        .map(|e| e.name.to_ascii_uppercase())
        .collect();
    names.sort();
    names.into_iter().map(|n| path::join(dir, &n)).collect()
}
