//! Cell-based binary split tree layout manager.
//!
//! All coordinates are in character cells. Separators between splits are 1 cell wide.

extern crate alloc;

use alloc::boxed::Box;
use alloc::vec::Vec;
use crate::{CellViewport, Pane, PaneId};

const SEPARATOR_CELLS: u16 = 1;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SplitDirection {
    /// Left | Right — vertical divider
    Vertical,
    /// Top / Bottom — horizontal divider
    Horizontal,
}

#[derive(Debug)]
pub enum LayoutNode {
    Leaf {
        pane_id: PaneId,
        viewport: CellViewport,
    },
    Split {
        direction: SplitDirection,
        /// Fraction of space allocated to `first` (0.0 – 1.0).
        ratio: f32,
        first: Box<LayoutNode>,
        second: Box<LayoutNode>,
    },
}

pub struct Layout {
    root: LayoutNode,
    panes: Vec<Pane>,
    focused_idx: usize,
    next_pane_id: PaneId,
    total_cols: u16,
    total_rows: u16,
}

// ---------------------------------------------------------------------------
// Layout impl
// ---------------------------------------------------------------------------

impl Layout {
    /// Create a new layout with a single pane occupying the full terminal.
    pub fn new(cols: u16, rows: u16) -> Self {
        let id: PaneId = 0;
        let vp = CellViewport::new(0, 0, cols, rows);
        let pane = Pane::new(id, vp.clone());
        let root = LayoutNode::Leaf { pane_id: id, viewport: vp };
        Self {
            root,
            panes: alloc::vec![pane],
            focused_idx: 0,
            next_pane_id: 1,
            total_cols: cols,
            total_rows: rows,
        }
    }

    // -- Accessors -----------------------------------------------------------

    pub fn focused_pane(&self) -> &Pane {
        &self.panes[self.focused_idx]
    }

    pub fn focused_pane_mut(&mut self) -> &mut Pane {
        &mut self.panes[self.focused_idx]
    }

    pub fn pane_count(&self) -> usize {
        self.panes.len()
    }

    pub fn panes(&self) -> &[Pane] {
        &self.panes
    }

    pub fn panes_mut(&mut self) -> &mut [Pane] {
        &mut self.panes
    }

    pub fn focused_pane_id(&self) -> PaneId {
        self.panes[self.focused_idx].id
    }

    pub fn root(&self) -> &LayoutNode {
        &self.root
    }

    pub fn pane_by_id_mut(&mut self, id: PaneId) -> Option<&mut Pane> {
        self.panes.iter_mut().find(|p| p.id == id)
    }

    // -- Focus cycling -------------------------------------------------------

    pub fn focus_next(&mut self) {
        if self.panes.is_empty() { return; }
        self.focused_idx = (self.focused_idx + 1) % self.panes.len();
    }

    pub fn focus_prev(&mut self) {
        if self.panes.is_empty() { return; }
        if self.focused_idx == 0 {
            self.focused_idx = self.panes.len() - 1;
        } else {
            self.focused_idx -= 1;
        }
    }

    // -- Split ---------------------------------------------------------------

    /// Split the focused pane. The old pane stays, the new pane gets focus.
    pub fn split(&mut self, direction: SplitDirection) {
        let focused_id = self.focused_pane_id();
        let new_id = self.next_pane_id;
        self.next_pane_id += 1;

        // Allocate new pane with temporary viewport; recompute_viewports fixes it.
        let temp_vp = CellViewport::new(0, 0, 1, 1);
        let new_pane = Pane::new(new_id, temp_vp);
        self.panes.push(new_pane);

        // Replace focused leaf in tree with a Split node.
        let root = core::mem::replace(&mut self.root, LayoutNode::Leaf { pane_id: 0, viewport: CellViewport::new(0,0,1,1) });
        self.root = split_node(root, focused_id, new_id, direction);

        // Recompute all viewports and sync panes.
        let full_vp = CellViewport::new(0, 0, self.total_cols, self.total_rows);
        assign_viewports(&mut self.root, full_vp);
        sync_panes(&self.root, &mut self.panes);

        // Focus the new pane (it was pushed last).
        self.focused_idx = self.panes.len() - 1;
    }

    // -- Close focused -------------------------------------------------------

    /// Remove the focused pane. No-op if only 1 pane remains.
    pub fn close_focused(&mut self) {
        if self.panes.len() <= 1 {
            return;
        }

        let focused_id = self.focused_pane_id();

        // Remove leaf from tree, promoting its sibling.
        let root = core::mem::replace(&mut self.root, LayoutNode::Leaf { pane_id: 0, viewport: CellViewport::new(0,0,1,1) });
        self.root = remove_leaf(root, focused_id);

        // Remove pane from Vec.
        if let Some(pos) = self.panes.iter().position(|p| p.id == focused_id) {
            self.panes.remove(pos);
        }

        // Clamp focused_idx.
        if self.focused_idx >= self.panes.len() {
            self.focused_idx = self.panes.len().saturating_sub(1);
        }

        // Reflow viewports.
        let full_vp = CellViewport::new(0, 0, self.total_cols, self.total_rows);
        assign_viewports(&mut self.root, full_vp);
        sync_panes(&self.root, &mut self.panes);
    }

    // -- Resize --------------------------------------------------------------

    pub fn resize(&mut self, cols: u16, rows: u16) {
        self.total_cols = cols;
        self.total_rows = rows;

        let full_vp = CellViewport::new(0, 0, cols, rows);
        assign_viewports(&mut self.root, full_vp);
        sync_panes(&self.root, &mut self.panes);
    }
}

// ---------------------------------------------------------------------------
// Tree helpers (free functions to avoid borrow-checker wrestling with &mut self)
// ---------------------------------------------------------------------------

/// Replace the leaf with `target_id` with a new Split node containing the
/// old leaf and a fresh Leaf for `new_id`.
fn split_node(node: LayoutNode, target_id: PaneId, new_id: PaneId, direction: SplitDirection) -> LayoutNode {
    match node {
        LayoutNode::Leaf { pane_id, viewport } if pane_id == target_id => {
            // Build the two children. Viewports are temporary; assign_viewports fixes them.
            let first = Box::new(LayoutNode::Leaf { pane_id, viewport: viewport.clone() });
            let second = Box::new(LayoutNode::Leaf { pane_id: new_id, viewport: viewport.clone() });
            LayoutNode::Split { direction, ratio: 0.5, first, second }
        }
        LayoutNode::Split { direction: dir, ratio, first, second } => {
            let first = Box::new(split_node(*first, target_id, new_id, direction));
            let second = Box::new(split_node(*second, target_id, new_id, direction));
            LayoutNode::Split { direction: dir, ratio, first, second }
        }
        other => other,
    }
}

/// Remove the leaf with `target_id` from the tree, promoting its sibling
/// into the parent's slot. If the root IS the target leaf, this panics
/// (caller should guard against that — we never call it when len == 1).
fn remove_leaf(node: LayoutNode, target_id: PaneId) -> LayoutNode {
    match node {
        LayoutNode::Leaf { pane_id, .. } => {
            // Caller should have prevented this case.
            LayoutNode::Leaf { pane_id, viewport: CellViewport::new(0,0,1,1) }
        }
        LayoutNode::Split { direction, ratio, first, second } => {
            let first_contains = leaf_contains(&first, target_id);
            let second_contains = leaf_contains(&second, target_id);

            if first_contains && is_direct_leaf(&first, target_id) {
                // Promote second
                *second
            } else if second_contains && is_direct_leaf(&second, target_id) {
                // Promote first
                *first
            } else if first_contains {
                let new_first = Box::new(remove_leaf(*first, target_id));
                LayoutNode::Split { direction, ratio, first: new_first, second }
            } else if second_contains {
                let new_second = Box::new(remove_leaf(*second, target_id));
                LayoutNode::Split { direction, ratio, first, second: new_second }
            } else {
                LayoutNode::Split { direction, ratio, first, second }
            }
        }
    }
}

fn leaf_contains(node: &LayoutNode, id: PaneId) -> bool {
    match node {
        LayoutNode::Leaf { pane_id, .. } => *pane_id == id,
        LayoutNode::Split { first, second, .. } => {
            leaf_contains(first, id) || leaf_contains(second, id)
        }
    }
}

fn is_direct_leaf(node: &LayoutNode, id: PaneId) -> bool {
    matches!(node, LayoutNode::Leaf { pane_id, .. } if *pane_id == id)
}

/// Recursively assign CellViewports to each leaf based on their split ratios.
fn assign_viewports(node: &mut LayoutNode, vp: CellViewport) {
    match node {
        LayoutNode::Leaf { viewport, .. } => {
            *viewport = vp;
        }
        LayoutNode::Split { direction, ratio, first, second } => {
            let (vp1, vp2) = split_viewport(vp, *direction, *ratio);
            assign_viewports(first, vp1);
            assign_viewports(second, vp2);
        }
    }
}

/// Split a viewport into two, accounting for the 1-cell separator.
fn split_viewport(vp: CellViewport, direction: SplitDirection, ratio: f32) -> (CellViewport, CellViewport) {
    match direction {
        SplitDirection::Vertical => {
            // Left | Right
            let total = vp.cols;
            let available = total.saturating_sub(SEPARATOR_CELLS);
            let first_cols = ((available as f32 * ratio) as u16).max(1).min(available.saturating_sub(1));
            let second_cols = available - first_cols;
            let vp1 = CellViewport::new(vp.col, vp.row, first_cols, vp.rows);
            let vp2 = CellViewport::new(vp.col + first_cols + SEPARATOR_CELLS, vp.row, second_cols, vp.rows);
            (vp1, vp2)
        }
        SplitDirection::Horizontal => {
            // Top / Bottom
            let total = vp.rows;
            let available = total.saturating_sub(SEPARATOR_CELLS);
            let first_rows = ((available as f32 * ratio) as u16).max(1).min(available.saturating_sub(1));
            let second_rows = available - first_rows;
            let vp1 = CellViewport::new(vp.col, vp.row, vp.cols, first_rows);
            let vp2 = CellViewport::new(vp.col, vp.row + first_rows + SEPARATOR_CELLS, vp.cols, second_rows);
            (vp1, vp2)
        }
    }
}

/// Walk the tree and call `pane.resize()` on any pane whose viewport changed.
fn sync_panes(node: &LayoutNode, panes: &mut Vec<Pane>) {
    match node {
        LayoutNode::Leaf { pane_id, viewport } => {
            if let Some(pane) = panes.iter_mut().find(|p| p.id == *pane_id) {
                if pane.viewport != *viewport {
                    pane.resize(viewport.clone());
                }
            }
        }
        LayoutNode::Split { first, second, .. } => {
            sync_panes(first, panes);
            sync_panes(second, panes);
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_layout_has_one_pane() {
        let layout = Layout::new(80, 24);
        assert_eq!(layout.pane_count(), 1);
        assert_eq!(layout.focused_pane().viewport.cols, 80);
        assert_eq!(layout.focused_pane().viewport.rows, 24);
    }

    #[test]
    fn split_vertical_creates_two_panes() {
        let mut layout = Layout::new(80, 24);
        layout.split(SplitDirection::Vertical);
        assert_eq!(layout.pane_count(), 2);
        let p0 = &layout.panes()[0];
        let p1 = &layout.panes()[1];
        assert_eq!(p0.viewport.cols + 1 + p1.viewport.cols, 80);
    }

    #[test]
    fn split_horizontal_creates_two_panes() {
        let mut layout = Layout::new(80, 24);
        layout.split(SplitDirection::Horizontal);
        assert_eq!(layout.pane_count(), 2);
        let p0 = &layout.panes()[0];
        let p1 = &layout.panes()[1];
        assert_eq!(p0.viewport.rows + 1 + p1.viewport.rows, 24);
    }

    #[test]
    fn focus_next_cycles() {
        let mut layout = Layout::new(80, 24);
        layout.split(SplitDirection::Vertical);
        let first = layout.focused_pane_id();
        layout.focus_next();
        assert_ne!(layout.focused_pane_id(), first);
        layout.focus_next();
        assert_eq!(layout.focused_pane_id(), first);
    }

    #[test]
    fn focus_prev_cycles() {
        let mut layout = Layout::new(80, 24);
        layout.split(SplitDirection::Vertical);
        let first = layout.focused_pane_id();
        layout.focus_prev();
        assert_ne!(layout.focused_pane_id(), first);
        layout.focus_prev();
        assert_eq!(layout.focused_pane_id(), first);
    }

    #[test]
    fn close_focused_removes_pane() {
        let mut layout = Layout::new(80, 24);
        layout.split(SplitDirection::Vertical);
        assert_eq!(layout.pane_count(), 2);
        layout.close_focused();
        assert_eq!(layout.pane_count(), 1);
        assert_eq!(layout.focused_pane().viewport.cols, 80);
    }

    #[test]
    fn close_last_pane_is_noop() {
        let mut layout = Layout::new(80, 24);
        layout.close_focused();
        assert_eq!(layout.pane_count(), 1);
    }

    #[test]
    fn resize_reflows_all_panes() {
        let mut layout = Layout::new(80, 24);
        layout.split(SplitDirection::Vertical);
        layout.resize(120, 40);
        let total_cols: u16 = layout.panes().iter()
            .map(|p| p.viewport.cols)
            .sum::<u16>() + 1;
        assert_eq!(total_cols, 120);
    }

    #[test]
    fn pane_by_id_returns_correct_pane() {
        let mut layout = Layout::new(80, 24);
        layout.split(SplitDirection::Vertical);
        let id = layout.panes()[1].id;
        assert!(layout.pane_by_id_mut(id).is_some());
        assert!(layout.pane_by_id_mut(9999).is_none());
    }

    #[test]
    fn nested_splits() {
        let mut layout = Layout::new(120, 40);
        layout.split(SplitDirection::Vertical);
        layout.split(SplitDirection::Horizontal);
        assert_eq!(layout.pane_count(), 3);
    }
}
