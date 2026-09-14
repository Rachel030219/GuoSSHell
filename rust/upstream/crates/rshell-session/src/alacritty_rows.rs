use alacritty_terminal::{
    Term,
    event::EventListener,
    grid::{Dimensions, Grid},
    index::Line,
    term::cell::Cell,
};

const ANCHOR_ROWS: usize = 3;

pub(crate) struct CapacityAnchor {
    start: usize,
    maximum_shift: usize,
    identities: [usize; ANCHOR_ROWS],
    oldest_history_identity: Option<usize>,
}

pub(crate) fn capture<T: EventListener>(
    terminal: &Term<T>,
    maximum_shift: usize,
) -> Option<CapacityAnchor> {
    let grid = terminal.grid();
    if grid.total_lines() < ANCHOR_ROWS {
        return None;
    }
    let start = grid.total_lines() - ANCHOR_ROWS;
    Some(CapacityAnchor {
        start,
        maximum_shift: maximum_shift.min(start),
        identities: std::array::from_fn(|offset| row_identity(grid, start + offset)),
        oldest_history_identity: (grid.history_size() != 0).then(|| row_identity(grid, 0)),
    })
}

pub(crate) fn completed_shift<T: EventListener>(
    terminal: &Term<T>,
    history_limit: usize,
    anchor: Option<CapacityAnchor>,
) -> usize {
    let Some(anchor) = anchor else {
        return 0;
    };
    let grid = terminal.grid();
    if grid.history_size() != history_limit {
        return 0;
    }
    let lower = anchor.start.saturating_sub(anchor.maximum_shift);
    for candidate in (lower..=anchor.start).rev() {
        let identities = std::array::from_fn(|offset| row_identity(grid, candidate + offset));
        if identities == anchor.identities {
            return anchor.start - candidate;
        }
    }
    if anchor
        .oldest_history_identity
        .is_some_and(|identity| row_identity(grid, 0) == identity)
    {
        return 0;
    }

    // The bounded window cannot evict an anchored active row and reuse its slot.
    // Addresses are copied before feeding and compared only in this window. If no
    // retained row is observable, reserve a disjoint stable range rather than reuse IDs.
    grid.total_lines()
}

fn row_identity(grid: &Grid<Cell>, offset: usize) -> usize {
    let history = grid.history_size();
    let line = Line(offset as i32 - history as i32);
    std::ptr::from_ref(&grid[line]) as usize
}
