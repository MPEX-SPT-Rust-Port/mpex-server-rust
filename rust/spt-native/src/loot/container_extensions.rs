//! `Extensions/ContainerExtensions.cs:14-258` — the 2D grid packing the static-container filler
//! uses. Ported warts and all: the scan limits derived from the item's smaller dimension and the
//! rollback-free partial marking are the C#'s, and `AddLootToContainer` depends on both.

/// `Models/Spt/Inventory/FindSlotResult.cs`. `x`/`y`/`rotation` are meaningless unless `success`;
/// the C# leaves them null on the failure path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct FindSlotResult {
    pub success: bool,
    pub x: i32,
    pub y: i32,
    pub rotation: bool,
}

/// `ContainerExtensions.FindSlotForItem` (`ContainerExtensions.cs:14-76`).
///
/// Both dimensions must be positive. A zero one drives the limits *past* the grid
/// (`min(w, h) == 0` gives `limit_y = rows + 1`) and, depending on the grid, either reports a bogus
/// fit — the per-cell sweep spans an empty range, so a 0-wide item "fits" any free cell — or walks
/// off the end and panics, which is the C#'s `IndexOutOfRangeException` path. Callers must skip an
/// item whose size is null or 0 rather than pass it through as 0.
pub fn find_slot_for_item(
    container_2d: &[Vec<u8>],
    item_width_x: i32,
    item_height_y: i32,
) -> FindSlotResult {
    // Quirk: both limits shrink by the item's *smaller* dimension, not the one bounding that axis.
    // Kept verbatim — it only ever over-scans, and the C# is the reference.
    let min_volume = item_width_x.min(item_height_y) - 1;
    let limit_y = row_count(container_2d) - min_volume;
    let limit_x = column_count(container_2d) - min_volume;

    // Every x+y slot taken up in container, exit
    if container_is_full(container_2d) {
        return FindSlotResult::default();
    }

    // Down = y, iterate over rows
    for row in 0..limit_y {
        if row_is_full(container_2d, row) {
            continue;
        }

        // Left to right across columns, look for free position
        for column in 0..limit_x {
            if can_item_be_placed_in_container_at_position(
                container_2d,
                row,
                column,
                item_width_x,
                item_height_y,
            ) {
                return FindSlotResult {
                    success: true,
                    x: column,
                    y: row,
                    rotation: false,
                };
            }

            // Doesn't fit AND rotating won't help. A 1x1 is the only item this skips, and rotating
            // one is a no-op, so the guard is a pure short-circuit.
            if item_width_x + item_height_y <= 2 {
                continue;
            }

            // Rotate item by swapping x and y item values
            if can_item_be_placed_in_container_at_position(
                container_2d,
                row,
                column,
                item_height_y,
                item_width_x,
            ) {
                return FindSlotResult {
                    success: true,
                    x: column,
                    y: row,
                    rotation: true,
                };
            }
        }
    }

    // Tried all possible positions, nothing big enough for item
    FindSlotResult::default()
}

/// `ContainerExtensions.TryFillContainerMapWithItem` (`ContainerExtensions.cs:89-139`), minus the
/// `errorMessage` out param no caller reads.
pub fn try_fill_container_map_with_item(
    container_2d: &mut [Vec<u8>],
    column_start_x: i32,
    row_start_y: i32,
    item_x_width: i32,
    item_y_height: i32,
    is_rotated: bool,
) -> bool {
    // Swap height/width if item needs to be rotated to fit
    let item_width = if is_rotated {
        item_y_height
    } else {
        item_x_width
    };
    let item_height = if is_rotated {
        item_x_width
    } else {
        item_y_height
    };

    let item_row_end_position = row_start_y + (item_height - 1);
    let item_column_end_position = column_start_x + (item_width - 1);

    // Item is a 1x1, flag slot as taken and exit early. Unconditional: an already-used cell is
    // overwritten and still reports success.
    if item_x_width == 1 && item_y_height == 1 {
        container_2d[row_start_y as usize][column_start_x as usize] = 1;

        return true;
    }

    // Loop over rows and columns and flag each as taken by item
    for y in row_start_y..=item_row_end_position {
        for x in column_start_x..=item_column_end_position {
            let cell = &mut container_2d[y as usize][x as usize];
            if *cell != 0 {
                // Quirk: cells flagged before the collision stay flagged, the C# never rolls back.
                return false;
            }

            // Flag slot as used
            *cell = 1;
        }
    }

    true
}

/// Rows.
fn row_count(container_2d: &[Vec<u8>]) -> i32 {
    container_2d.len() as i32
}

/// Columns. The C# `int[,]` is rectangular by construction, so row 0 speaks for all of them.
fn column_count(container_2d: &[Vec<u8>]) -> i32 {
    container_2d.first().map_or(0, Vec::len) as i32
}

/// `ContainerExtensions.RowIsFull` (`ContainerExtensions.cs:147-161`).
fn row_is_full(container_2d: &[Vec<u8>], row_index: i32) -> bool {
    container_2d[row_index as usize]
        .iter()
        .all(|&cell| cell != 0)
}

/// `ContainerExtensions.ContainerIsFull` (`ContainerExtensions.cs:168-190`).
pub(crate) fn container_is_full(container_2d: &[Vec<u8>]) -> bool {
    container_2d.iter().flatten().all(|&cell| cell != 0)
}

/// `ContainerExtensions.CanItemBePlacedInContainerAtPosition` (`ContainerExtensions.cs:212-258`).
fn can_item_be_placed_in_container_at_position(
    container: &[Vec<u8>],
    item_start_vertical_pos: i32,
    item_start_horizontal_pos: i32,
    item_width: i32,
    item_height: i32,
) -> bool {
    let item_end_col_position = item_start_horizontal_pos + item_width - 1;
    let item_end_row_position = item_start_vertical_pos + item_height - 1;

    // Check item isn't bigger than container when at position
    if item_end_col_position > column_count(container) - 1
        || item_end_row_position > row_count(container) - 1
    {
        // Item is bigger than container, will never fit
        return false;
    }

    // Early exit if exact spot is taken
    if container[item_start_vertical_pos as usize][item_start_horizontal_pos as usize] == 1 {
        return false;
    }

    // Single slot item, do direct check
    if item_width == 1 && item_height == 1 {
        return container[item_start_vertical_pos as usize][item_start_horizontal_pos as usize]
            == 0;
    }

    for row in item_start_vertical_pos..=item_end_row_position {
        for column in item_start_horizontal_pos..=item_end_col_position {
            if container[row as usize][column as usize] == 1 {
                // Occupied by something
                return false;
            }
        }
    }

    // Slot is free
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grid(rows: usize, columns: usize) -> Vec<Vec<u8>> {
        vec![vec![0u8; columns]; rows]
    }

    #[test]
    fn find_slot_scans_rows_outer_columns_inner_and_takes_the_first_fit() {
        let container = vec![vec![1, 1, 0], vec![0, 0, 0]];

        let result = find_slot_for_item(&container, 1, 1);

        // Rows are the outer loop and columns the inner one, so the single free cell at the end of
        // row 0 beats the entirely free row below it.
        assert_eq!(
            result,
            FindSlotResult {
                success: true,
                x: 2,
                y: 0,
                rotation: false,
            }
        );
    }

    #[test]
    fn find_slot_skips_full_rows_and_keeps_scanning() {
        let container = vec![vec![1, 1, 1], vec![1, 0, 1]];

        let result = find_slot_for_item(&container, 1, 1);

        assert_eq!(
            result,
            FindSlotResult {
                success: true,
                x: 1,
                y: 1,
                rotation: false,
            }
        );
    }

    #[test]
    fn find_slot_fails_when_every_cell_is_taken() {
        let container = vec![vec![1, 1], vec![1, 1]];

        // The ContainerIsFull early exit: failure carries no position, like `new FindSlotResult(false)`.
        assert_eq!(
            find_slot_for_item(&container, 1, 1),
            FindSlotResult::default()
        );
    }

    #[test]
    fn find_slot_rotates_when_the_item_only_fits_sideways() {
        let container = grid(3, 2);

        // 3 wide x 2 high never fits in 2 columns; 3 + 2 > 2 so the rotated 2 wide x 3 high check
        // runs and fills the grid exactly.
        let result = find_slot_for_item(&container, 3, 2);

        assert_eq!(
            result,
            FindSlotResult {
                success: true,
                x: 0,
                y: 0,
                rotation: true,
            }
        );
    }

    #[test]
    fn find_slot_prefers_the_unrotated_fit_when_both_orientations_fit() {
        let container = grid(3, 3);

        // Row 0 / column 0 takes a 2 wide x 1 high item either way round, and the unrotated check
        // runs first, so rotation stays off. Trying the rotated orientation first would report
        // `rotation: true` here and stamp a 1x2 footprint instead of a 2x1 one.
        let result = find_slot_for_item(&container, 2, 1);

        assert_eq!(
            result,
            FindSlotResult {
                success: true,
                x: 0,
                y: 0,
                rotation: false,
            }
        );
    }

    #[test]
    fn find_slot_tries_both_orientations_per_cell_before_moving_on() {
        // Rotation is attempted at each cell, not in a second pass over the whole grid: 2 wide x 1
        // high cannot start at (0, 0) (taken) and cannot fit unrotated at (0, 1) (one column left),
        // but rotated it stands in column 1 across both rows. A full unrotated pass followed by a
        // full rotated pass would settle for (0, 1) unrotated instead.
        let container = vec![vec![1, 0], vec![0, 0]];

        let result = find_slot_for_item(&container, 2, 1);

        assert_eq!(
            result,
            FindSlotResult {
                success: true,
                x: 1,
                y: 0,
                rotation: true,
            }
        );
    }

    #[test]
    fn find_slot_loop_bounds_use_the_smaller_dimension_quirk() {
        // Quirk 1: both limits come from `min(w, h) - 1` rather than the dimension that actually
        // bounds each axis (`ContainerExtensions.cs:20-24`).
        //
        // Hand-computed for this case — rows = 3, cols = 1, item 3 wide x 1 high:
        //   minVolume = min(3, 1) - 1 = 0
        //   limitY    = 3 - 0 = 3   -> rows 0, 1, 2 are scanned
        //   limitX    = 1 - 0 = 1   -> column 0 is scanned
        //   row 0, column 0: unrotated wants columns 0..2 of a 1-column grid -> rejected on bounds.
        //                    3 + 1 > 2, so the rotated 1 wide x 3 high check runs: rows 0..2 of
        //                    column 0, all free -> fit.
        // So the C# math yields success at (0, 0) rotated. Pinned as-is, not "fixed".
        let container = grid(3, 1);

        let result = find_slot_for_item(&container, 3, 1);

        assert_eq!(
            result,
            FindSlotResult {
                success: true,
                x: 0,
                y: 0,
                rotation: true,
            }
        );
    }

    #[test]
    fn find_slot_fails_without_panicking_when_the_bounds_go_negative() {
        // 2x2 grid, 5x5 item: minVolume = 4, so limitY = limitX = -2 and neither loop body runs.
        // Only oversized items are safe this way — a 0-sized one overshoots instead and panics, see
        // the note on `find_slot_for_item`.
        let container = grid(2, 2);

        assert!(!find_slot_for_item(&container, 5, 5).success);
    }

    #[test]
    fn try_fill_marks_every_cell_the_item_covers() {
        let mut container = grid(3, 3);

        assert!(try_fill_container_map_with_item(
            &mut container,
            1,
            0,
            2,
            2,
            false
        ));
        assert_eq!(container, vec![vec![0, 1, 1], vec![0, 1, 1], vec![0, 0, 0]]);
    }

    #[test]
    fn try_fill_swaps_width_and_height_when_rotated() {
        let mut container = grid(3, 3);

        // 1 wide x 3 high rotated covers 3 columns of one row instead.
        assert!(try_fill_container_map_with_item(
            &mut container,
            0,
            0,
            1,
            3,
            true
        ));
        assert_eq!(container, vec![vec![1, 1, 1], vec![0, 0, 0], vec![0, 0, 0]]);
    }

    #[test]
    fn try_fill_leaves_partial_marks_behind_when_it_collides_quirk() {
        // Quirk 2: the C# bails on the first taken cell and never rolls back the cells it already
        // flagged (`ContainerExtensions.cs:120-136`). AddLootToContainer discards the result with
        // `out _`, so the container keeps the phantom marks.
        let mut container = vec![vec![0, 0, 1]];

        let filled = try_fill_container_map_with_item(&mut container, 0, 0, 3, 1, false);

        assert!(!filled);
        assert_eq!(container, vec![vec![1, 1, 1]]);
    }

    #[test]
    fn try_fill_1x1_marks_blindly_and_succeeds_even_on_a_taken_cell() {
        let mut container = vec![vec![1, 0]];

        // The 1x1 fast path writes without checking, and still reports success.
        assert!(try_fill_container_map_with_item(
            &mut container,
            0,
            0,
            1,
            1,
            false
        ));
        assert_eq!(container, vec![vec![1, 0]]);
    }

    #[test]
    fn find_slot_then_try_fill_agree_on_the_rotated_footprint() {
        let mut container = grid(3, 2);

        let slot = find_slot_for_item(&container, 3, 2);
        assert!(slot.rotation);

        assert!(try_fill_container_map_with_item(
            &mut container,
            slot.x,
            slot.y,
            3,
            2,
            slot.rotation
        ));
        assert!(container.iter().flatten().all(|&cell| cell == 1));
    }
}
