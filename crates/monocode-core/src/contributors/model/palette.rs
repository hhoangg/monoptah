//! Fixed avatar and chart colors, the same in light and dark.

use super::js_cmp;
use std::collections::{BTreeMap, HashSet};

pub const PERSON_COLORS: [&str; 8] = [
    "#7c9cff", "#e3a008", "#3fb950", "#f85149", "#d946ef", "#14b8a6", "#f97316", "#a78bfa",
];

pub fn color_for(index: i64) -> &'static str {
    PERSON_COLORS[index.rem_euclid(PERSON_COLORS.len() as i64) as usize]
}

fn hash_index(person_id: &str) -> usize {
    let mut hash: u32 = 0x811c9dc5; // FNV-1a over UTF-16 code units, like the web app
    for unit in person_id.encode_utf16() {
        hash ^= u32::from(unit);
        hash = hash.wrapping_mul(0x01000193);
    }
    hash as usize % PERSON_COLORS.len()
}

/// The color of one person from a hash of their id. Two ids can collide; use
/// `person_color_map` for a set of people shown together.
pub fn color_for_person(person_id: &str) -> &'static str {
    PERSON_COLORS[hash_index(person_id)]
}

/// Colors for a group of people so that no two of the first eight share one.
/// Ids are taken in sorted order; each prefers its hashed color and moves to the
/// next free one when that is taken. Once all eight are used a new round starts.
/// The same set always gives the same colors, and adding an id that sorts last
/// changes nobody else's color.
pub fn person_color_map<S: AsRef<str>>(person_ids: &[S]) -> BTreeMap<String, &'static str> {
    let mut ids: Vec<&str> = person_ids.iter().map(AsRef::as_ref).collect();
    ids.sort_by(|a, b| js_cmp(a, b));
    ids.dedup();
    let mut map = BTreeMap::new();
    let mut used = HashSet::new();
    for id in ids {
        if used.len() == PERSON_COLORS.len() {
            used.clear();
        }
        let mut index = hash_index(id);
        while used.contains(&index) {
            index = (index + 1) % PERSON_COLORS.len();
        }
        used.insert(index);
        map.insert(id.to_string(), PERSON_COLORS[index]);
    }
    map
}

/// A person's color from the shared map, falling back to the hash for an id not in it.
pub fn color_from_map(colors: &BTreeMap<String, &'static str>, person_id: &str) -> &'static str {
    colors
        .get(person_id)
        .copied()
        .unwrap_or_else(|| color_for_person(person_id))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{HashMap, HashSet};

    fn ids(n: usize) -> Vec<String> {
        (0..n).map(|i| format!("p{i}@x.io")).collect()
    }

    fn strs(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn palette_has_8_distinct_hex_colors() {
        assert_eq!(PERSON_COLORS.len(), 8);
        assert_eq!(PERSON_COLORS.iter().collect::<HashSet<_>>().len(), 8);
        for c in PERSON_COLORS {
            assert!(
                c.len() == 7
                    && c.starts_with('#')
                    && c[1..].chars().all(|ch| ch.is_ascii_hexdigit()),
                "{c}"
            );
        }
    }

    #[test]
    fn palette_picks_by_index_and_wraps_around() {
        assert_eq!(color_for(0), PERSON_COLORS[0]);
        assert_eq!(color_for(7), PERSON_COLORS[7]);
        assert_eq!(color_for(8), PERSON_COLORS[0]);
        assert_eq!(color_for(19), PERSON_COLORS[3]);
    }

    #[test]
    fn palette_wraps_a_negative_index() {
        assert_eq!(color_for(-1), PERSON_COLORS[7]);
    }

    #[test]
    fn color_for_person_is_stable_for_the_same_id_and_always_a_palette_color() {
        let id = "ana@example.com";
        assert_eq!(color_for_person(id), color_for_person(id));
        assert!(PERSON_COLORS.contains(&color_for_person(id)));
        assert!(PERSON_COLORS.contains(&color_for_person("")));
    }

    #[test]
    fn color_for_person_does_not_depend_on_the_order_ids_are_asked_for() {
        let ids = ["a@x.io", "b@x.io", "c@x.io", "p-ana"];
        let forward: Vec<_> = ids.iter().map(|id| color_for_person(id)).collect();
        let mut backward: Vec<_> = ids.iter().rev().map(|id| color_for_person(id)).collect();
        backward.reverse();
        assert_eq!(backward, forward);
    }

    #[test]
    fn color_for_person_spreads_many_ids_over_most_of_the_palette() {
        let used: HashSet<_> = (0..200)
            .map(|i| color_for_person(&format!("user{i}@x.io")))
            .collect();
        assert!(used.len() >= 6, "{}", used.len());
    }

    #[test]
    fn color_for_person_matches_the_js_fnv_1a_hash() {
        // Indexes computed with hashIndex() in palette.ts.
        assert_eq!(color_for_person(""), PERSON_COLORS[5]);
        assert_eq!(color_for_person("a"), PERSON_COLORS[4]);
        assert_eq!(color_for_person("ana@example.com"), PERSON_COLORS[0]);
        assert_eq!(color_for_person("Đạt 😀"), PERSON_COLORS[3]);
    }

    #[test]
    fn person_color_map_gives_up_to_eight_people_eight_different_colors_for_any_ids() {
        for trial in 0..200 {
            let people: Vec<String> = (0..8)
                .map(|i| format!("user{trial}-{i}@corp.com"))
                .collect();
            for n in 1..=8 {
                let map = person_color_map(&people[..n]);
                assert_eq!(map.values().collect::<HashSet<_>>().len(), n);
            }
        }
    }

    #[test]
    fn person_color_map_is_the_same_whatever_order_the_ids_come_in() {
        let people = ids(6);
        let mut reversed = people.clone();
        reversed.reverse();
        assert_eq!(person_color_map(&reversed), person_color_map(&people));
    }

    #[test]
    fn person_color_map_only_uses_palette_colors_and_ignores_duplicate_ids() {
        let map = person_color_map(&strs(&["a", "b", "a"]));
        let mut keys: Vec<_> = map.keys().cloned().collect();
        keys.sort();
        assert_eq!(keys, ["a", "b"]);
        for color in map.values() {
            assert!(PERSON_COLORS.contains(color));
        }
    }

    #[test]
    fn person_color_map_keeps_the_hashed_color_for_the_first_person_in_sorted_order() {
        let map = person_color_map(&strs(&["zed@x.io", "amy@x.io"]));
        assert_eq!(map["amy@x.io"], color_for_person("amy@x.io"));
    }

    #[test]
    fn person_color_map_moves_a_person_to_the_next_free_color_when_the_preferred_one_is_taken() {
        // Two ids with the same preferred color.
        let pool = ids(40);
        let (clash, other) = pool
            .iter()
            .enumerate()
            .find_map(|(i, a)| {
                pool[i + 1..]
                    .iter()
                    .find(|b| color_for_person(a) == color_for_person(b))
                    .map(|b| (a.clone(), b.clone()))
            })
            .unwrap();
        let map = person_color_map(&[clash.clone(), other.clone()]);
        assert_ne!(map[&clash], map[&other]);
        let mut both = [clash, other];
        both.sort();
        assert_eq!(map[&both[0]], color_for_person(&both[0]));
    }

    #[test]
    fn person_color_map_starts_reusing_colors_only_after_all_eight_are_taken() {
        let map = person_color_map(&ids(12));
        let mut counts: HashMap<&str, usize> = HashMap::new();
        for color in map.values() {
            *counts.entry(color).or_default() += 1;
        }
        assert_eq!(counts.len(), 8);
        assert_eq!(*counts.values().max().unwrap(), 2);
    }

    #[test]
    fn person_color_map_does_not_change_when_someone_sorted_after_is_added() {
        let base = person_color_map(&strs(&["a@x.io", "b@x.io", "c@x.io"]));
        let more = person_color_map(&strs(&["a@x.io", "b@x.io", "c@x.io", "d@x.io"]));
        for id in ["a@x.io", "b@x.io", "c@x.io"] {
            assert_eq!(more[id], base[id]);
        }
    }

    #[test]
    fn color_from_map_falls_back_to_the_hash_for_an_id_not_in_the_map() {
        let map = person_color_map(&strs(&["a@x.io"]));
        assert_eq!(color_from_map(&map, "a@x.io"), map["a@x.io"]);
        assert_eq!(color_from_map(&map, "zz@x.io"), color_for_person("zz@x.io"));
    }
}
