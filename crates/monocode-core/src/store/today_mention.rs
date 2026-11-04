//! Expands `@today/<key>` mentions on send (see `today::mention`).
use super::Store;
use crate::today::{mention, statuses, Status};

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
}

#[cfg(test)]
mod tests {
    use crate::store::Store;
    use crate::today::{Actor, NewTask};

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
