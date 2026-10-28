//! X-axis label placement.

/// Indexes of the buckets that get an x-axis label: the first, the last and
/// about every Nth one between, so labels never overlap however many buckets
/// there are. A tick that would sit right next to the last one is dropped.
pub fn axis_ticks(count: usize) -> Vec<usize> {
    axis_ticks_with(count, 8)
}

pub fn axis_ticks_with(count: usize, max_labels: usize) -> Vec<usize> {
    if count == 0 {
        return vec![];
    }
    let step = count.div_ceil(max_labels.max(1)).max(1);
    let last = count - 1;
    if last == 0 {
        return vec![0];
    }
    // Every bucket is labelled only when each has its own slot; otherwise two
    // neighbours would crowd each other.
    let gap = if step == 1 {
        1
    } else {
        step.div_ceil(2).max(2)
    };
    let mut ticks: Vec<usize> = (0..last)
        .step_by(step)
        .filter(|&i| last - i >= gap || i == 0)
        .collect();
    ticks.push(last);
    ticks
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn axis_ticks_is_empty_for_nothing_and_one_tick_for_one_bucket() {
        assert_eq!(axis_ticks(0), Vec::<usize>::new());
        assert_eq!(axis_ticks(1), [0]);
    }

    #[test]
    fn axis_ticks_labels_every_bucket_when_there_are_few() {
        assert_eq!(axis_ticks(5), [0, 1, 2, 3, 4]);
        assert_eq!(axis_ticks(8), [0, 1, 2, 3, 4, 5, 6, 7]);
    }

    #[test]
    fn axis_ticks_always_includes_the_first_and_the_last_bucket() {
        for n in [2, 9, 13, 24, 53, 120, 521] {
            let ticks = axis_ticks(n);
            assert_eq!(ticks[0], 0);
            assert_eq!(ticks[ticks.len() - 1], n - 1);
        }
    }

    #[test]
    fn axis_ticks_keeps_ticks_increasing_bounded_in_number_and_never_crowded() {
        for n in [9usize, 13, 14, 24, 53, 120, 521] {
            let ticks = axis_ticks(n);
            let step = n.div_ceil(8);
            assert!(ticks.len() <= 10);
            for pair in ticks.windows(2) {
                assert!(pair[1] > pair[0]);
                assert!(pair[1] - pair[0] >= step.div_ceil(2));
            }
        }
    }

    #[test]
    fn axis_ticks_never_labels_two_neighbouring_buckets() {
        for count in 9..=60 {
            let ticks = axis_ticks(count);
            for pair in ticks.windows(2) {
                assert!(pair[1] - pair[0] >= 2);
            }
        }
    }

    #[test]
    fn axis_ticks_honors_a_custom_label_budget() {
        let ticks = axis_ticks_with(100, 4);
        assert!(ticks.len() <= 6, "{ticks:?}");
        assert_eq!(ticks[0], 0);
        assert_eq!(*ticks.last().unwrap(), 99);
    }
}
