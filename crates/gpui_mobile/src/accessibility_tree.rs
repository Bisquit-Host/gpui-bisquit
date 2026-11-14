//! Modal isolation for GPUI's complete accessibility snapshots.

use gpui::accesskit::{NodeId, Role, TreeUpdate};
use std::collections::{HashMap, HashSet};

/// Keep the window root and the last dialog in child paint order. Walking the
/// hierarchy rather than the node vector also selects an inner nested dialog.
/// GPUI 0.3.6 sends complete trees; this must not be used on incremental updates.
pub(crate) fn isolate_modal(tree: &mut TreeUpdate) -> bool {
    let Some(root) = tree.tree.as_ref().map(|tree| tree.root) else {
        return false;
    };
    let nodes: HashMap<_, _> = tree.nodes.iter().map(|(id, node)| (*id, node)).collect();
    let mut pending = vec![root];
    let mut visited = HashSet::new();
    let mut modal = None;
    while let Some(id) = pending.pop() {
        if !visited.insert(id) {
            continue;
        }
        if let Some(node) = nodes.get(&id) {
            if node.role() == Role::Dialog {
                modal = Some(id);
            }
            pending.extend(node.children().iter().rev().copied());
        }
    }
    let Some(modal) = modal else {
        return false;
    };
    let mut retained = HashSet::<NodeId>::new();
    pending.push(modal);
    while let Some(id) = pending.pop() {
        if retained.insert(id) {
            if let Some(node) = nodes.get(&id) {
                pending.extend(node.children());
            }
        }
    }
    if !retained.contains(&tree.focus) {
        tree.focus = modal;
    }
    retained.insert(root);
    tree.nodes.retain(|(id, _)| retained.contains(id));
    if modal != root {
        if let Some((_, node)) = tree.nodes.iter_mut().find(|(id, _)| *id == root) {
            node.set_children([modal]);
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::accesskit::{Node, Tree, TreeId};

    fn snapshot(focus: u64) -> TreeUpdate {
        let mut root = Node::new(Role::Window);
        root.set_children([NodeId(1), NodeId(2), NodeId(4)]);
        let mut outer = Node::new(Role::Dialog);
        outer.set_children([NodeId(3)]);
        let mut inner = Node::new(Role::Dialog);
        inner.set_children([NodeId(5)]);
        TreeUpdate {
            // Deliberately out of paint order, like GPUI's postorder emission.
            nodes: vec![
                (NodeId(5), Node::new(Role::Button)),
                (NodeId(4), inner),
                (NodeId(3), Node::new(Role::Button)),
                (NodeId(2), outer),
                (NodeId(1), Node::new(Role::Button)),
                (NodeId(0), root),
            ],
            tree: Some(Tree::new(NodeId(0))),
            tree_id: TreeId::ROOT,
            focus: NodeId(focus),
        }
    }

    #[test]
    fn topmost_dialog_hides_background_and_previous_modal() {
        let mut tree = snapshot(1);
        assert!(isolate_modal(&mut tree));
        assert_eq!(tree.focus, NodeId(4));
        assert_eq!(
            tree.nodes.iter().map(|(id, _)| id.0).collect::<Vec<_>>(),
            [5, 4, 0]
        );
        assert_eq!(tree.nodes.last().unwrap().1.children(), [NodeId(4)]);
    }

    #[test]
    fn nested_dialog_keeps_its_text_runs_and_focus() {
        let mut tree = snapshot(5);
        let input = &mut tree
            .nodes
            .iter_mut()
            .find(|(id, _)| *id == NodeId(5))
            .unwrap()
            .1;
        input.set_role(Role::TextInput);
        input.set_children([NodeId(6)]);
        tree.nodes.push((NodeId(6), Node::new(Role::TextRun)));
        tree.nodes
            .iter_mut()
            .find(|(id, _)| *id == NodeId(2))
            .unwrap()
            .1
            .set_children([NodeId(3), NodeId(4)]);
        tree.nodes
            .iter_mut()
            .find(|(id, _)| *id == NodeId(0))
            .unwrap()
            .1
            .set_children([NodeId(1), NodeId(2)]);
        assert!(isolate_modal(&mut tree));
        assert_eq!(tree.focus, NodeId(5));
        assert_eq!(tree.nodes.len(), 4);
        assert!(tree.nodes.iter().any(|(id, _)| *id == NodeId(6)));
    }

    #[test]
    fn closing_dialog_restores_complete_snapshot() {
        let mut tree = snapshot(1);
        for (_, node) in &mut tree.nodes {
            if node.role() == Role::Dialog {
                node.set_role(Role::GenericContainer);
            }
        }
        let original = tree.clone();
        assert!(!isolate_modal(&mut tree));
        assert_eq!(tree, original);
    }
}
