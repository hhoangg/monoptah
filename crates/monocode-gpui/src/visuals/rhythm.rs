//! The weekly rhythm: seven rows (Monday to Sunday) by 24 columns (hour of the
//! day), each a Bayer-dithered square whose tone follows the quartile of its
//! count, like the calendar. The caller owns how the pointer makes it repaint.
use super::{
    calendar::{PITCH, SQUARE, Surface, TONES, levels_of},
    dither::{Align, CELL, Grid, Label, Motion, inked, paint_inked, pointer, readout, sheetable},
};
use crate::theme::content;
use gpui::{
    Bounds, Div, Hsla, InteractiveElement, ParentElement, Role, SharedString, Stateful,
    StatefulInteractiveElement, Styled, canvas, div, fill, point, px, size,
};
use std::sync::Arc;

/// Counts by weekday (Monday first) and hour.
pub(crate) type Counts = [[u32; 24]; 7];

/// What the counts count, in the words of the summary and the hover card.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Noun {
    pub one: &'static str,
    pub many: &'static str,
}
pub(crate) const COMMITS: Noun = Noun {
    one: "commit",
    many: "commits",
};

pub(crate) const DAYS: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
const GUTTER: f32 = 28.;
const HOURS: f32 = 18.;

/// The height of a rhythm.
pub(crate) fn height() -> f32 {
    HOURS + (7 * PITCH) as f32 * CELL
}

/// The pitch, in dither cells, a row of `cols` cells gives 24 columns: they
/// fill the width, and are never narrower than 2. Rows keep the calendar's height.
pub(crate) fn col_pitch_for(cols: i32) -> i32 {
    (cols / 24).max(2)
}

/// The (hour, weekday) at `x`, `y` pixels from the rhythm's corner.
pub(crate) fn hit(width: f32, x: f32, y: f32) -> Option<(usize, usize)> {
    let cols = ((width - GUTTER) / CELL).floor().max(0.) as i32;
    cell_at(
        x - GUTTER,
        y - HOURS,
        col_pitch_for(cols) as f32 * CELL,
        PITCH as f32 * CELL,
    )
}

/// What a screen reader gets: the total and the busiest hour.
pub(crate) fn summary(counts: &Counts, noun: &Noun) -> String {
    let total: u32 = counts.iter().flatten().sum();
    let mut best = (0, 0, 0);
    for (day, row) in counts.iter().enumerate() {
        for (hour, n) in row.iter().enumerate() {
            if *n > best.0 {
                best = (*n, day, hour);
            }
        }
    }
    if total == 0 {
        return format!("Weekly rhythm: no {}", noun.many);
    }
    format!(
        "Weekly rhythm: {total} {}, busiest {} {:02}:00",
        noun.many, DAYS[best.1], best.2
    )
}

/// The (hour, weekday) under a point `x`, `y` pixels from the grid's corner,
/// for columns `col` wide and rows `row` high.
pub(crate) fn cell_at(x: f32, y: f32, col: f32, row: f32) -> Option<(usize, usize)> {
    if x < 0. || y < 0. {
        return None;
    }
    let (hour, day) = ((x / col) as usize, (y / row) as usize);
    (hour < 24 && day < 7).then_some((hour, day))
}

/// The hover card's title: `Tue 14:00 - 12 commits`.
pub(crate) fn hover_title(day: usize, hour: usize, count: u32, noun: &Noun) -> String {
    let unit = if count == 1 { noun.one } else { noun.many };
    format!("{} {hour:02}:00 - {count} {unit}", DAYS[day.min(6)])
}

/// The count of each cell, row by row, as the quartile levels see them.
pub(crate) fn rhythm_levels(counts: &Counts) -> Vec<u8> {
    levels_of(&counts.iter().flatten().copied().collect::<Vec<_>>())
}

/// The rhythm of `counts` of `noun` in `tint`. The element has no pointer handlers: the
/// caller makes it repaint as the pointer moves.
pub(crate) fn rhythm(
    counts: Arc<Counts>,
    tint: Hsla,
    motion: Motion,
    id: &'static str,
    surface: Surface,
    noun: Noun,
) -> Stateful<Div> {
    let label: SharedString = summary(&counts, &noun).into();
    let levels = rhythm_levels(&counts);
    let map = canvas(
        |_, _, _| {},
        move |bounds, _, window, cx| {
            surface.area.set(bounds);
            let track = content(0.07);
            let grid = Grid::new(
                bounds.origin + point(px(GUTTER), px(HOURS)),
                bounds.size.width.as_f32() - GUTTER,
                (7 * PITCH) as f32 * CELL,
            );
            // Columns stretch to the width, rows keep their height; the gap stays.
            let col_cells = col_pitch_for(grid.cols);
            let wide_cells = (col_cells - (PITCH - SQUARE)).max(1);
            let high_cells = SQUARE;
            let (col, row) = (col_cells as f32 * CELL, PITCH as f32 * CELL);
            let (wide, high) = (wide_cells as f32 * CELL, high_cells as f32 * CELL);
            let grown = motion.at(0., 420., window, cx);
            let hovered =
                pointer(bounds, window).and_then(|(x, y)| cell_at(x - GUTTER, y - HOURS, col, row));
            let sheet = surface
                .sheet
                .as_ref()
                .filter(|_| grown >= 1. && sheetable(tint, window));
            if let Some((slot, version)) = sheet {
                // Settled: the squares and one image of the ink, not thousands of quads.
                let square_at = |hour: usize, day: usize| {
                    Bounds::new(
                        grid.origin + point(px(hour as f32 * col), px(day as f32 * row)),
                        size(px(wide), px(high)),
                    )
                };
                for day in 0..7 {
                    for hour in 0..24 {
                        window.paint_quad(fill(square_at(hour, day), track));
                    }
                }
                let key = version ^ ((col_cells as u64) << 40);
                let painted = paint_inked(slot, key, &grid, tint, window, cx, |c, r| {
                    let (x, y) = (c % col_cells, r % PITCH);
                    let (hour, day) = ((c / col_cells) as usize, (r / PITCH) as usize);
                    x < wide_cells
                        && y < high_cells
                        && hour < 24
                        && day < 7
                        && levels[day * 24 + hour] > 0
                        && inked(x, y, TONES[levels[day * 24 + hour] as usize])
                });
                debug_assert!(painted, "sheetable said the image can be drawn");
                if let Some((hour, day)) = hovered.filter(|(h, d)| levels[d * 24 + h] > 0) {
                    // The hovered cell thickens: its square again, with more ink.
                    window.paint_quad(fill(square_at(hour, day), track));
                    let level = levels[day * 24 + hour] as usize;
                    let tone = ((TONES[level] + 0.3).min(1.) * 16.).round() / 16.;
                    for y in 0..high_cells {
                        for x in 0..wide_cells {
                            if inked(x, y, tone) {
                                window.paint_quad(fill(
                                    Bounds::new(
                                        grid.origin
                                            + point(
                                                px(hour as f32 * col + x as f32 * CELL),
                                                px(day as f32 * row + y as f32 * CELL),
                                            ),
                                        size(px(CELL), px(CELL)),
                                    ),
                                    tint,
                                ));
                            }
                        }
                    }
                }
            } else {
                grid.paint(window, |c, r| {
                    let (x, y) = (c % col_cells, r % PITCH);
                    let (hour, day) = ((c / col_cells) as usize, (r / PITCH) as usize);
                    if x >= wide_cells || y >= high_cells || hour >= 24 || day >= 7 {
                        return None;
                    }
                    let level = levels[day * 24 + hour];
                    if level == 0 {
                        return Some(track);
                    }
                    let mut tone = TONES[level as usize] * grown;
                    if hovered == Some((hour, day)) {
                        tone = (tone + 0.3).min(1.);
                    }
                    let tone = (tone * 16.).round() / 16.;
                    Some(if inked(x, y, tone) { tint } else { track })
                });
            }
            let axis = Label::mono(10., content(0.45));
            for (r, name) in DAYS.iter().enumerate() {
                axis.paint(
                    name,
                    grid.origin + point(px(-GUTTER), px(r as f32 * row + high / 2.)),
                    Align::Left,
                    window,
                    cx,
                );
            }
            for hour in [0, 6, 12, 18] {
                axis.paint(
                    &format!("{hour:02}"),
                    grid.origin + point(px(hour as f32 * col), px(-HOURS / 2.)),
                    Align::Left,
                    window,
                    cx,
                );
            }
            if let Some((hour, day)) = hovered {
                let n = counts[day][hour];
                readout(
                    &hover_title(day, hour, n, &noun),
                    &[],
                    grid.origin
                        + point(
                            px(hour as f32 * col + wide),
                            px(day as f32 * row + high / 2.),
                        ),
                    bounds,
                    window,
                    cx,
                );
            }
        },
    )
    .size_full();
    div()
        .id(id)
        .debug_selector(|| id.into())
        .role(Role::Image)
        .aria_label(label)
        .w_full()
        .h(px(height()))
        .child(map)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_title_names_weekday_hour_and_count() {
        assert_eq!(hover_title(1, 14, 12, &COMMITS), "Tue 14:00 - 12 commits");
        assert_eq!(hover_title(6, 3, 1, &COMMITS), "Sun 03:00 - 1 commit");
        assert_eq!(hover_title(0, 0, 0, &COMMITS), "Mon 00:00 - 0 commits");
    }

    #[test]
    fn another_noun_changes_the_words_and_nothing_else() {
        let tickets = Noun {
            one: "ticket",
            many: "tickets",
        };
        assert_eq!(hover_title(1, 14, 12, &tickets), "Tue 14:00 - 12 tickets");
        assert_eq!(hover_title(6, 3, 1, &tickets), "Sun 03:00 - 1 ticket");
        let mut counts: Counts = [[0; 24]; 7];
        assert_eq!(summary(&counts, &tickets), "Weekly rhythm: no tickets");
        counts[1][14] = 2;
        assert_eq!(
            summary(&counts, &tickets),
            "Weekly rhythm: 2 tickets, busiest Tue 14:00"
        );
    }

    #[test]
    fn the_summary_names_the_total_and_the_busiest_hour() {
        let mut counts: Counts = [[0; 24]; 7];
        assert_eq!(summary(&counts, &COMMITS), "Weekly rhythm: no commits");
        counts[1][14] = 12;
        counts[4][9] = 3;
        assert_eq!(
            summary(&counts, &COMMITS),
            "Weekly rhythm: 15 commits, busiest Tue 14:00"
        );
    }

    #[test]
    fn a_pixel_maps_to_an_hour_and_a_weekday_through_the_gutters() {
        let (c, r) = (
            col_pitch_for(400 / CELL as i32) as f32 * CELL,
            PITCH as f32 * CELL,
        );
        let width = 28. + 400.;
        assert_eq!(hit(width, 28. + 1., HOURS + 1.), Some((0, 0)));
        assert_eq!(
            hit(width, 28. + 5. * c + 1., HOURS + 2. * r + 1.),
            Some((5, 2))
        );
        // The last column ends where the available width ends, not 40% in.
        assert_eq!(hit(width, 28. + 23. * c + 1., HOURS + 1.), Some((23, 0)));
        assert_eq!(hit(width, 10., HOURS + 1.), None);
        assert_eq!(hit(width, 28. + 1., 4.), None);
    }

    #[test]
    fn a_point_maps_to_an_hour_and_a_weekday() {
        assert_eq!(cell_at(0., 0., 15., 10.), Some((0, 0)));
        assert_eq!(cell_at(14.9, 9.9, 15., 10.), Some((0, 0)));
        assert_eq!(cell_at(15., 20., 15., 10.), Some((1, 2)));
        assert_eq!(
            cell_at(23. * 15. + 1., 6. * 10. + 1., 15., 10.),
            Some((23, 6))
        );
        assert_eq!(cell_at(24. * 15., 0., 15., 10.), None);
        assert_eq!(cell_at(0., 7. * 10., 15., 10.), None);
        assert_eq!(cell_at(-1., 0., 15., 10.), None);
        assert_eq!(cell_at(0., -1., 15., 10.), None);
    }

    #[test]
    fn the_columns_stretch_with_the_width_but_not_below_two_cells() {
        assert_eq!(col_pitch_for(24 * 5), 5);
        // Wider than the old fixed pitch: the columns grow to fill it.
        assert_eq!(col_pitch_for(24 * 12 + 7), 12);
        assert_eq!(col_pitch_for(24 * 4), 4);
        assert_eq!(col_pitch_for(10), 2);
        assert_eq!(col_pitch_for(-3), 2);
    }

    #[test]
    fn levels_follow_the_quartiles_of_the_busy_hours() {
        let mut counts: Counts = [[0; 24]; 7];
        for (i, n) in [1, 2, 3, 4, 5, 6, 7, 8].into_iter().enumerate() {
            counts[0][i + 8] = n;
        }
        let levels = rhythm_levels(&counts);
        assert_eq!(levels.len(), 168);
        assert_eq!(levels[0], 0);
        assert_eq!(levels[8], 1);
        assert_eq!(levels[15], 4);
        assert!(rhythm_levels(&[[0; 24]; 7]).iter().all(|l| *l == 0));
    }
}
