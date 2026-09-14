use rshell_core::{
    connection::{PaneId, SessionId},
    workspace::{PaneTree, SplitAxis, WorkspaceError},
};

#[test]
fn nested_splits_close_only_the_direct_parent_and_visit_sessions() {
    let first = PaneId::new();
    let second = PaneId::new();
    let third = PaneId::new();
    let first_session = SessionId::new();
    let third_session = SessionId::new();
    let tree = PaneTree::leaf(first)
        .split(first, SplitAxis::Horizontal, second, 0.5)
        .unwrap()
        .split(first, SplitAxis::Vertical, third, 0.4)
        .unwrap();
    let mut tree = tree;

    tree.replace_session(first, Some(first_session)).unwrap();
    tree.replace_session(third, Some(third_session)).unwrap();
    assert_eq!(tree.session_ids(), vec![first_session, third_session]);

    let tree = tree.close(first).unwrap();
    assert_eq!(tree.pane_ids(), vec![third, second]);
    assert_eq!(tree.session_id(third).unwrap(), Some(third_session));
    assert!(matches!(
        tree.find_pane(first),
        Err(WorkspaceError::PaneNotFound(id)) if id == first
    ));
}

#[test]
fn split_rejects_bad_ratios_and_missing_panes() {
    let pane = PaneId::new();
    let other = PaneId::new();

    assert!(matches!(
        PaneTree::leaf(pane).split(pane, SplitAxis::Horizontal, other, 0.09),
        Err(WorkspaceError::InvalidSplitRatio(_))
    ));
    assert!(matches!(
        PaneTree::leaf(pane).split(other, SplitAxis::Horizontal, PaneId::new(), 0.5),
        Err(WorkspaceError::PaneNotFound(id)) if id == other
    ));
}

#[test]
fn closing_last_pane_and_replacing_a_missing_session_are_explicit_errors() {
    let pane = PaneId::new();
    let missing = PaneId::new();
    let mut tree = PaneTree::leaf(pane);

    assert!(matches!(
        tree.clone().close(pane),
        Err(WorkspaceError::LastPane)
    ));
    assert!(matches!(
        tree.replace_session(missing, Some(SessionId::new())),
        Err(WorkspaceError::PaneNotFound(id)) if id == missing
    ));
}
