//! Expands `@today/<key>` mentions on send (see `today::mention`).
use super::Store;
use crate::today::attachments::{turn_images, MAX_TURN_IMAGES};
use crate::today::{mention, statuses, Status};
use std::path::PathBuf;

impl Store {
    /// `text` with the context of every `@today/<key>` that names a task.
    /// Unknown and dismissed keys stay plain text; a store error leaves `text`
    /// as it is.
    pub fn apply_today_to_turn(&self, text: &str) -> Result<String, String> {
        self.apply_today_to_turn_from(text, text)
    }

    /// `text` with the context of every task `typed` mentions. The composer
    /// passes what the user typed as `typed`, so a token that only arrives
    /// inside an appended note never expands.
    pub fn apply_today_to_turn_from(&self, text: &str, typed: &str) -> Result<String, String> {
        let keys = mention::keys_in_text(typed);
        if keys.is_empty() {
            return Ok(text.to_owned());
        }
        let workflow = self.today_statuses()?;
        let mut tasks = Vec::new();
        for key in keys {
            let Some(task) = self.today_task(&key)? else {
                continue;
            };
            if task.status == Status::Dismissed {
                continue;
            }
            let status = statuses::resolve(&task, &workflow).clone();
            let notes = self.today_notes(&task.id)?;
            tasks.push((task, status, notes));
        }
        Ok(mention::attach(text, &tasks))
    }

    /// The image copies to attach to a message for every task `typed` mentions:
    /// the same keys as [`Self::apply_today_to_turn_from`] (unknown and dismissed
    /// ones add nothing), tasks in mention order, newest note first within a
    /// task. One budget covers the whole message: at most `max_count` files (and
    /// never more than `MAX_TURN_IMAGES`) totalling at most `max_bytes`.
    pub fn today_turn_images(
        &self,
        typed: &str,
        max_count: usize,
        max_bytes: u64,
    ) -> Result<Vec<PathBuf>, String> {
        let mut left_count = max_count.min(MAX_TURN_IMAGES);
        let mut left_bytes = max_bytes;
        let mut images: Vec<PathBuf> = Vec::new();
        for key in mention::keys_in_text(typed) {
            if left_count == 0 {
                break;
            }
            let Some(task) = self.today_task(&key)? else {
                continue;
            };
            if task.status == Status::Dismissed {
                continue;
            }
            let notes = self.today_notes(&task.id)?;
            for path in turn_images(&notes, left_count, left_bytes) {
                if images.contains(&path) {
                    continue;
                }
                left_count -= 1;
                left_bytes =
                    left_bytes.saturating_sub(path.metadata().map_or(0, |meta| meta.len()));
                images.push(path);
            }
        }
        Ok(images)
    }
}

#[cfg(test)]
mod tests {
    use crate::store::Store;
    use crate::today::{Actor, NewTask};

    fn png(path: &std::path::Path, pad: usize) {
        let mut bytes = Vec::new();
        image::RgbaImage::new(2, 2)
            .write_to(
                &mut std::io::Cursor::new(&mut bytes),
                image::ImageFormat::Png,
            )
            .unwrap();
        bytes.resize(bytes.len() + pad, 0);
        std::fs::write(path, bytes).unwrap();
    }
    fn task_in(store: &Store, title: &str) -> crate::today::Task {
        store
            .today_add(
                &NewTask {
                    title: title.into(),
                    ..NewTask::default()
                },
                &Actor::User,
                1_800_000_000_000,
            )
            .unwrap()
    }
    fn note_images(store: &Store, task: &str, files: &[std::path::PathBuf]) {
        store
            .today_note_with_files(task, "pics", files, &Actor::User, 1_800_000_000_000)
            .unwrap();
    }

    #[test]
    fn turn_images_follow_mentions_with_one_shared_budget() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src");
        std::fs::create_dir_all(&src).unwrap();
        let store = Store::open(&dir.path().join("monocode.db")).unwrap();
        let a = task_in(&store, "A");
        let b = task_in(&store, "B");
        let gone = task_in(&store, "Gone");
        let one = src.join("one.png");
        let two = src.join("two.png");
        let three = src.join("three.png");
        for file in [&one, &two, &three] {
            png(file, 0);
        }
        let doc = src.join("doc.pdf");
        std::fs::write(&doc, b"%PDF").unwrap();
        note_images(&store, &a.id, &[one, two, doc.clone()]);
        note_images(&store, &b.id, &[three]);
        note_images(&store, &gone.id, &[src.join("one.png")]);
        store
            .today_dismiss(&gone.id, &Actor::User, 1_800_000_000_000)
            .unwrap();

        let typed = format!(
            "@today/{} @today/nope:1 @today/{} @today/{} @today/{}",
            b.key, gone.key, a.key, b.key
        );
        let all = store.today_turn_images(&typed, 20, u64::MAX).unwrap();
        // B first (mention order), then A's two images; no pdf, no dismissed task, no repeat.
        assert_eq!(all.len(), 3);
        assert!(all[0].to_string_lossy().contains("three"));
        assert!(all.iter().all(|p| p.is_file()));
        assert!(all.iter().all(|p| p.extension().unwrap() == "png"));

        // One count budget for the whole message.
        assert_eq!(
            store.today_turn_images(&typed, 2, u64::MAX).unwrap().len(),
            2
        );
        assert!(store
            .today_turn_images(&typed, 0, u64::MAX)
            .unwrap()
            .is_empty());
        // One byte budget: room for exactly two files.
        let size = all[0].metadata().unwrap().len();
        assert_eq!(
            store.today_turn_images(&typed, 20, size * 2).unwrap().len(),
            2
        );
        assert!(store
            .today_turn_images(&typed, 20, size - 1)
            .unwrap()
            .is_empty());
        // Nothing mentioned, nothing attached.
        assert!(store
            .today_turn_images("no mention", 20, u64::MAX)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn turn_images_put_the_newest_note_first_within_a_task() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("monocode.db")).unwrap();
        let task = task_in(&store, "Order");
        let old = dir.path().join("old.png");
        let new = dir.path().join("new.png");
        png(&old, 0);
        png(&new, 0);
        note_images(&store, &task.id, &[old]);
        store
            .today_note_with_files(&task.id, "later", &[new], &Actor::User, 1_800_000_001_000)
            .unwrap();
        let typed = format!("@today/{}", task.key);
        let names: Vec<_> = store
            .today_turn_images(&typed, 10, u64::MAX)
            .unwrap()
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        // Stored names carry a unique prefix; the order is what matters.
        assert_eq!(names.len(), 2);
        assert!(names[0].ends_with("new.png") && names[1].ends_with("old.png"));
    }

    #[test]
    fn turn_images_never_exceed_the_turn_cap() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src");
        std::fs::create_dir_all(&src).unwrap();
        let store = Store::open(&dir.path().join("monocode.db")).unwrap();
        let mut keys = Vec::new();
        for n in 0..4 {
            let task = task_in(&store, &format!("T{n}"));
            let files: Vec<_> = (0..4)
                .map(|i| {
                    let file = src.join(format!("t{n}-{i}.png"));
                    png(&file, 0);
                    file
                })
                .collect();
            note_images(&store, &task.id, &files);
            keys.push(format!("@today/{}", task.key));
        }
        let images = store
            .today_turn_images(&keys.join(" "), 100, u64::MAX)
            .unwrap();
        assert_eq!(images.len(), crate::today::attachments::MAX_TURN_IMAGES);
    }

    #[test]
    fn a_known_key_expands_and_an_unknown_one_stays_plain() {
        let store = Store::open(std::path::Path::new(":memory:")).unwrap();
        let task = store
            .today_add(
                &NewTask {
                    title: "Ship the report".into(),
                    ..NewTask::default()
                },
                &Actor::User,
                1_800_000_000_000,
            )
            .unwrap();
        store
            .today_note(
                &task.id,
                "Remember the totals",
                &Actor::User,
                1_800_000_000_000,
            )
            .unwrap();
        let text = format!("Start @today/{} and @today/nope:1", task.key);
        let out = store.apply_today_to_turn(&text).unwrap();
        assert!(out.starts_with(&text));
        assert!(out.contains("title: \"Ship the report\""));
        assert!(out.contains("Remember the totals"));
        assert_eq!(out.matches("--- BEGIN TASK DATA ---").count(), 1);
        assert!(!out.contains("nope:1\""));

        // Only the typed text decides what expands.
        let moved = store
            .apply_today_to_turn_from("other text", &format!("go @today/{}", task.key))
            .unwrap();
        assert!(moved.starts_with("other text") && moved.contains("Ship the report"));
        assert_eq!(
            store
                .apply_today_to_turn_from(&out, "no mention here")
                .unwrap(),
            out
        );

        let plain = "nothing @today/nope:1 here";
        assert_eq!(store.apply_today_to_turn(plain).unwrap(), plain);
        assert_eq!(
            store.apply_today_to_turn("no mention").unwrap(),
            "no mention"
        );
    }

    #[test]
    fn a_dismissed_task_does_not_expand() {
        let store = Store::open(std::path::Path::new(":memory:")).unwrap();
        let task = store
            .today_add(
                &NewTask {
                    title: "Gone".into(),
                    ..NewTask::default()
                },
                &Actor::User,
                1_800_000_000_000,
            )
            .unwrap();
        store
            .today_dismiss(&task.id, &Actor::User, 1_800_000_000_000)
            .unwrap();
        let text = format!("see @today/{}", task.key);
        assert_eq!(store.apply_today_to_turn(&text).unwrap(), text);
        assert!(!store.today_mention_keys().unwrap().contains(&task.key));
    }
}
