//! The activity calendar: weeks as columns, Sunday to Saturday down, each day
//! a Bayer-dithered square in one colour. Shared by the Mono dashboard and
//! the Contributors page; the caller owns what a day is (`Active`) and how the
//! pointer makes it repaint.
use super::dither::{
    Align, CELL, Grid, Label, Motion, Sheets, glow, inked, paint_inked, pointer, readout, sheetable,
};
use crate::theme::content;
use chrono::{Datelike, Duration as Days, NaiveDate};
use gpui::{
    Bounds, Div, Hsla, InteractiveElement, ParentElement, Pixels, Role, SharedString, Stateful,
    StatefulInteractiveElement, Styled, canvas, div, fill, point, px, size,
};
use std::{cell::Cell, rc::Rc, sync::Arc};

/// One day of a calendar.
pub(crate) trait Active {
    fn date(&self) -> NaiveDate;
    /// What the day's intensity is made of; 0 is an empty day.
    fn amount(&self) -> u32;
    /// The rows of the hover card as (swatch, name, value), for a day with
    /// something in it.
    fn details(&self, tint: Hsla) -> Vec<(Hsla, String, String)>;
}

/// The Sunday that opens the first of `weeks` weeks ending with the one of `today`.
pub(crate) fn first_day(today: NaiveDate, weeks: i64) -> NaiveDate {
    today - Days::days(today.weekday().num_days_from_sunday() as i64) - Days::weeks(weeks - 1)
}

/// Intensity, 0 for an empty day and 1 to 4 by quartile of the active ones,
/// so a quiet person still shows contrast.
pub(crate) fn levels_of(amounts: &[u32]) -> Vec<u8> {
    let mut active: Vec<u32> = amounts.iter().copied().filter(|c| *c > 0).collect();
    active.sort_unstable();
    let at = |q: f32| {
        active
            .get(((active.len() as f32 * q) as usize).min(active.len().saturating_sub(1)))
            .copied()
            .unwrap_or(0)
    };
    let cuts = [at(0.25), at(0.5), at(0.75)];
    amounts
        .iter()
        .map(|c| match c {
            0 => 0,
            c => 1 + cuts.iter().filter(|cut| *c > **cut).count() as u8,
        })
        .collect()
}

/// Calendar intensity of each day.
pub(crate) fn levels<D: Active>(days: &[D]) -> Vec<u8> {
    levels_of(&days.iter().map(Active::amount).collect::<Vec<_>>())
}

/// Dithered tone per level: a quarter of the cells up to every cell.
pub(crate) const TONES: [f32; 5] = [0., 0.25, 0.5, 0.75, 1.];
/// Each day is a square of 4×4 dither cells, a cell apart.
pub(crate) const SQUARE: i32 = 4;
pub(crate) const PITCH: i32 = 5;
const GUTTER: f32 = 28.;
const MONTHS: f32 = 18.;
const LEGEND: f32 = 22.;

/// The height of a calendar.
pub(crate) fn height() -> f32 {
    MONTHS + (7 * PITCH) as f32 * CELL + LEGEND
}

/// The grid's columns, the weeks that fit, and the weeks skipped at the left
/// (a narrow panel keeps the latest), for a calendar `width` pixels wide.
fn layout(width: f32, count: usize, weeks: i64) -> (i32, i32, i32) {
    let cols = ((width - GUTTER) / CELL).floor().max(0.) as i32;
    let shown = (count as i32 + 6) / 7;
    let fits = ((cols + PITCH - SQUARE) / PITCH).clamp(1, shown.min(weeks as i32).max(1));
    (cols, fits, shown - fits)
}

/// The day (index into the days) at `x`, `y` pixels from the calendar's corner.
pub(crate) fn hit(width: f32, count: usize, weeks: i64, x: f32, y: f32) -> Option<usize> {
    let (_, fits, skip) = layout(width, count, weeks);
    let (x, y) = (x - GUTTER, y - MONTHS);
    if x < 0. || y < 0. {
        return None;
    }
    let pitch = PITCH as f32 * CELL;
    let (c, r) = ((x / pitch) as i32, (y / pitch) as i32);
    let ix = ((c + skip) * 7 + r) as usize;
    (c < fits && r < 7 && ix < count).then_some(ix)
}

/// What a screen reader gets: the total, the active days and the busiest one.
pub(crate) fn summary<D: Active>(name: &str, days: &[D]) -> String {
    let total: u32 = days.iter().map(Active::amount).sum();
    let active = days.iter().filter(|d| d.amount() > 0).count();
    let mut text = format!("{name}: {total} contributions on {active} days");
    if let Some(best) = days
        .iter()
        .rev()
        .max_by_key(|d| d.amount())
        .filter(|d| d.amount() > 0)
    {
        text.push_str(&format!(
            ", busiest {} with {}",
            best.date().format("%b %-d"),
            best.amount()
        ));
    }
    text
}

/// What a canvas shares with the view around it: where it was laid out, for
/// the pointer, and the image of its ink with the number that says when to
/// remake it (`None`: always paint quads).
#[derive(Default, Clone)]
pub(crate) struct Surface {
    pub area: Rc<Cell<Bounds<Pixels>>>,
    pub sheet: Option<(Sheets, u64)>,
}

/// The calendar of `days` (whole weeks from a Sunday, ending today) in `tint`,
/// the latest `weeks` weeks of it that fit. The element has no pointer
/// handlers: the caller makes it repaint as the pointer moves.
pub(crate) fn calendar<D: Active + 'static>(
    days: Arc<Vec<D>>,
    tint: Hsla,
    motion: Motion,
    weeks: i64,
    id: &'static str,
    label: impl Into<SharedString>,
    surface: Surface,
) -> Stateful<Div> {
    let levels = levels(&days);
    let start = days.first().map(|d| d.date());
    let map = canvas(
        |_, _, _| {},
        move |bounds, _, window, cx| {
            surface.area.set(bounds);
            let Some(start) = start else { return };
            let track = content(0.07);
            let pitch = PITCH as f32 * CELL;
            let square = SQUARE as f32 * CELL;
            let grid = Grid::new(
                bounds.origin + point(px(GUTTER), px(MONTHS)),
                bounds.size.width.as_f32() - GUTTER,
                7. * pitch,
            );
            // A narrow panel keeps the latest weeks, as calendars do.
            let (_, fits, skip) = layout(bounds.size.width.as_f32(), days.len(), weeks);
            let day = |c: i32, r: i32| {
                let ix = ((c + skip) * 7 + r) as usize;
                (r < 7).then_some(ix).filter(|ix| *ix < days.len())
            };
            // Weeks thicken in left to right.
            let grown: Vec<f32> = (0..fits)
                .map(|c| motion.at(c as f32 * 12., 420., window, cx))
                .collect();
            let hovered = pointer(bounds, window).and_then(|(x, y)| {
                let (x, y) = (x - GUTTER, y - MONTHS);
                if x < 0. || y < 0. {
                    return None;
                }
                let (c, r) = ((x / pitch) as i32, (y / pitch) as i32);
                (c < fits).then_some(()).and(day(c, r)).map(|ix| (c, r, ix))
            });
            let today = days.len() - 1;
            if levels[today] > 0 {
                let c = (today / 7) as i32 - skip;
                let r = (today % 7) as f32;
                glow(
                    window,
                    Bounds::new(
                        grid.origin + point(px(c as f32 * pitch), px(r * pitch)),
                        size(px(square), px(square)),
                    ),
                    tint,
                    0.35,
                );
            }
            let settled = grown.iter().all(|g| *g >= 1.);
            let sheet = surface
                .sheet
                .as_ref()
                .filter(|_| settled && sheetable(tint, window));
            if let Some((slot, version)) = sheet {
                // Settled: the squares and one image of the ink, not thousands of quads.
                let square_at = |c: i32, r: i32| {
                    Bounds::new(
                        grid.origin + point(px(c as f32 * pitch), px(r as f32 * pitch)),
                        size(px(square), px(square)),
                    )
                };
                for c in 0..fits {
                    for r in 0..7 {
                        if day(c, r).is_some() {
                            window.paint_quad(fill(square_at(c, r), track));
                        }
                    }
                }
                let key = version ^ ((fits as u64) << 40) ^ ((skip as u64) << 48);
                let painted = paint_inked(slot, key, &grid, tint, window, cx, |col, row| {
                    let (x, y) = (col % PITCH, row % PITCH);
                    let (c, r) = (col / PITCH, row / PITCH);
                    x < SQUARE
                        && y < SQUARE
                        && c < fits
                        && day(c, r).is_some_and(|ix| {
                            levels[ix] > 0 && inked(x, y, TONES[levels[ix] as usize])
                        })
                });
                debug_assert!(painted, "sheetable said the image can be drawn");
                if let Some((c, r, ix)) = hovered.filter(|(_, _, ix)| levels[*ix] > 0) {
                    // The hovered day thickens: its square again, with more ink.
                    window.paint_quad(fill(square_at(c, r), track));
                    let tone = ((TONES[levels[ix] as usize] + 0.3).min(1.) * 16.).round() / 16.;
                    for y in 0..SQUARE {
                        for x in 0..SQUARE {
                            if inked(x, y, tone) {
                                window.paint_quad(fill(
                                    Bounds::new(
                                        grid.origin
                                            + point(
                                                px(c as f32 * pitch + x as f32 * CELL),
                                                px(r as f32 * pitch + y as f32 * CELL),
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
                grid.paint(window, |col, row| {
                    let (x, y) = (col % PITCH, row % PITCH);
                    let (c, r) = (col / PITCH, row / PITCH);
                    if x >= SQUARE || y >= SQUARE || c >= fits {
                        return None;
                    }
                    let ix = day(c, r)?;
                    let level = levels[ix];
                    if level == 0 {
                        return Some(track);
                    }
                    let mut tone = TONES[level as usize] * grown[c as usize];
                    if hovered.is_some_and(|(hc, hr, _)| (hc, hr) == (c, r)) {
                        tone = (tone + 0.3).min(1.);
                    }
                    let tone = (tone * 16.).round() / 16.;
                    Some(if inked(x, y, tone) { tint } else { track })
                });
            }
            let axis = Label::mono(10., content(0.45));
            for (r, name) in [(1, "Mon"), (3, "Wed"), (5, "Fri")] {
                axis.paint(
                    name,
                    grid.origin + point(px(-GUTTER), px(r as f32 * pitch + square / 2.)),
                    Align::Left,
                    window,
                    cx,
                );
            }
            // A month is named over the first week that starts in it.
            let mut last_at = f32::MIN;
            for c in 0..fits {
                let week = start + Days::weeks((c + skip) as i64);
                let label_month = c == 0 || week.day() <= 7;
                let x = c as f32 * pitch;
                if !label_month || x - last_at < 30. {
                    continue;
                }
                last_at = x;
                axis.paint(
                    &week.format("%b").to_string(),
                    grid.origin + point(px(x), px(-MONTHS / 2.)),
                    Align::Left,
                    window,
                    cx,
                );
            }
            // Less ▫▪▪▪▪ More, flush right under the latest weeks.
            let legend_y = grid.origin.y.as_f32() + 7. * pitch + LEGEND / 2. + 2.;
            let more = Label::mono(10., content(0.4));
            let edge = (grid.origin.x.as_f32() + fits as f32 * pitch - CELL).round();
            more.paint(
                "More",
                point(px(edge), px(legend_y)),
                Align::Right,
                window,
                cx,
            );
            let right = edge - more.measure("More", window) - 6.;
            let swatches = (right - 5. * pitch + CELL).round();
            more.paint(
                "Less",
                point(px(swatches - 6.), px(legend_y)),
                Align::Right,
                window,
                cx,
            );
            let key = Grid::new(
                point(px(swatches), px(legend_y - square / 2.)),
                5. * pitch,
                square,
            );
            key.paint(window, |col, row| {
                let (x, level) = (col % PITCH, (col / PITCH) as usize);
                if x >= SQUARE || level > 4 {
                    return None;
                }
                Some(if inked(x, row, TONES[level]) {
                    tint
                } else {
                    track
                })
            });
            if let Some((c, r, ix)) = hovered {
                let d = &days[ix];
                let rows = if d.amount() == 0 {
                    vec![(track, "Contributions".into(), "None".into())]
                } else {
                    d.details(tint)
                };
                readout(
                    &d.date().format("%a, %b %-d").to_string(),
                    &rows,
                    grid.origin + point(px(c as f32 * pitch + square), px(r as f32 * pitch)),
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
    fn levels_split_the_active_amounts_by_quartile() {
        assert_eq!(
            levels_of(&[0, 1, 2, 3, 4, 5, 6, 7, 8]),
            [0, 1, 1, 1, 2, 2, 3, 3, 4]
        );
        assert_eq!(levels_of(&[0, 0]), [0, 0]);
        assert_eq!(levels_of(&[3]), [1]);
        assert!(levels_of(&[]).is_empty());
    }

    struct D(NaiveDate, u32);
    impl Active for D {
        fn date(&self) -> NaiveDate {
            self.0
        }
        fn amount(&self) -> u32 {
            self.1
        }
        fn details(&self, _: Hsla) -> Vec<(Hsla, String, String)> {
            vec![]
        }
    }

    #[test]
    fn the_summary_names_the_total_the_active_days_and_the_busiest() {
        let day = |d, n| D(NaiveDate::from_ymd_opt(2026, 10, d).unwrap(), n);
        let days = [day(1, 0), day(2, 3), day(3, 9), day(4, 9)];
        assert_eq!(
            summary("Activity calendar", &days),
            "Activity calendar: 21 contributions on 3 days, busiest Oct 4 with 9"
        );
        assert_eq!(
            summary("Activity calendar", &[day(1, 0)]),
            "Activity calendar: 0 contributions on 0 days"
        );
    }

    #[test]
    fn a_point_maps_to_a_day_and_the_narrow_panel_keeps_the_latest_weeks() {
        let pitch = PITCH as f32 * CELL;
        // 10 weeks (70 days), wide: column 0 is the first week.
        let wide = 28. + 10. * pitch + 30.;
        assert_eq!(hit(wide, 70, 53, 28. + 1., MONTHS + 1.), Some(0));
        assert_eq!(
            hit(wide, 70, 53, 28. + pitch + 1., MONTHS + pitch + 1.),
            Some(8)
        );
        assert_eq!(hit(wide, 70, 53, 27., MONTHS + 1.), None);
        assert_eq!(hit(wide, 70, 53, 28. + 1., MONTHS - 1.), None);
        assert_eq!(hit(wide, 70, 53, 28. + 1., MONTHS + 7. * pitch + 1.), None);
        // Only 3 weeks fit: the first column is the 8th week.
        let narrow = 28. + 3. * pitch;
        assert_eq!(hit(narrow, 70, 53, 28. + 1., MONTHS + 1.), Some(49));
        // Past the last day.
        assert_eq!(
            hit(
                wide,
                68,
                53,
                28. + 9. * pitch + 1.,
                MONTHS + 6. * pitch + 1.
            ),
            None
        );
    }

    #[test]
    fn the_first_day_is_the_sunday_of_the_earliest_week() {
        // Thursday 29 October 2026.
        let today = NaiveDate::from_ymd_opt(2026, 10, 29).unwrap();
        let first = first_day(today, 53);
        assert_eq!(first.weekday(), chrono::Weekday::Sun);
        assert_eq!((today - first).num_days(), 52 * 7 + 4);
        assert_eq!(
            first_day(today, 1),
            NaiveDate::from_ymd_opt(2026, 10, 25).unwrap()
        );
    }
}
