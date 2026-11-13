use super::token::{find, incomplete_tail, pick, seed, strip};
use super::*;
use std::collections::HashMap;

fn emoji(file: &str, emotion: Option<&str>) -> Emoji {
    Emoji {
        file: file.into(),
        hash: format!("hash-{file}"),
        name: file.into(),
        format: Format::Png,
        emotion: emotion.map(str::to_owned),
    }
}

fn library_with(emoji_list: Vec<Emoji>) -> (Library, String) {
    let mut library = Library {
        enabled: true,
        ..Library::default()
    };
    let pack = library.add_pack("Pack", None, emoji_list);
    (library, pack)
}

#[test]
fn default_library_matches_the_spec() {
    let library = Library::default();
    assert!(!library.enabled);
    assert_eq!(library.version, 1);
    assert_eq!(library.frequency, Frequency::Sometimes);
    assert!(library.packs.is_empty());
    assert_eq!(library.emotions.len(), 13);
    assert_eq!(library.emotions[0].name, "happy");
    assert_eq!(
        library.emotions[0].description,
        "Glad, pleased, things went well"
    );
    assert_eq!(library.emotions[12].name, "chill");
    assert_eq!(library.emotions[12].description, "Relaxed, nothing urgent");
}

#[test]
fn json_roundtrip_uses_camel_case_and_tolerates_missing_fields() {
    let (mut library, pack) = library_with(vec![emoji("a.png", Some("happy"))]);
    library.packs[0].sorted_by = Some("Model".into());
    library.frequency = Frequency::Often;
    let text = serde_json::to_string(&library).unwrap();
    assert!(text.contains("\"sortedBy\""));
    assert!(text.contains("\"frequency\":\"often\""));
    assert!(text.contains("\"format\":\"png\""));
    let back: Library = serde_json::from_str(&text).unwrap();
    assert_eq!(back, library);
    assert_eq!(back.packs[0].id, pack);
    let sparse: Library = serde_json::from_str(r#"{"enabled":true,"future":1}"#).unwrap();
    assert!(sparse.enabled);
    assert_eq!(sparse.emotions.len(), 13);
}

#[test]
fn dir_validates_the_mono_id() {
    let root = Path::new("/r");
    assert_eq!(dir(root, "m-1").unwrap(), Path::new("/r/m-1/emoji"));
    assert!(dir(root, "../x").is_err());
    assert!(dir(root, "").is_err());
}

#[test]
fn load_missing_is_default_and_save_roundtrips() {
    let temp = tempfile::tempdir().unwrap();
    let loaded = load(temp.path(), "m1").unwrap();
    assert!(!loaded.read_only);
    assert_eq!(loaded.library, Library::default());
    let (library, _) = library_with(vec![emoji("a.png", Some("happy"))]);
    save(temp.path(), "m1", &library).unwrap();
    let again = load(temp.path(), "m1").unwrap();
    assert_eq!(again.library, library);
    assert!(!again.read_only);
}

#[test]
fn newer_version_is_read_only_and_save_refuses() {
    let temp = tempfile::tempdir().unwrap();
    let folder = dir(temp.path(), "m1").unwrap();
    std::fs::create_dir_all(&folder).unwrap();
    let text = r#"{"version":2,"enabled":true,"extra":{"a":1}}"#;
    std::fs::write(folder.join("emoji.json"), text).unwrap();
    let loaded = load(temp.path(), "m1").unwrap();
    assert!(loaded.read_only);
    assert!(loaded.library.enabled);
    assert!(save(temp.path(), "m1", &loaded.library).is_err());
    assert_eq!(
        std::fs::read_to_string(folder.join("emoji.json")).unwrap(),
        text
    );
}

#[test]
fn unreadable_json_is_an_error_and_left_alone() {
    let temp = tempfile::tempdir().unwrap();
    let folder = dir(temp.path(), "m1").unwrap();
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(folder.join("emoji.json"), "{ not json").unwrap();
    assert!(load(temp.path(), "m1").is_err());
    assert!(save(temp.path(), "m1", &Library::default()).is_err());
    assert_eq!(
        std::fs::read_to_string(folder.join("emoji.json")).unwrap(),
        "{ not json"
    );
}

#[test]
fn load_removes_staging_folders_only() {
    let temp = tempfile::tempdir().unwrap();
    let folder = dir(temp.path(), "m1").unwrap();
    std::fs::create_dir_all(folder.join(".staging-abc")).unwrap();
    std::fs::write(folder.join(".staging-abc/x.png"), b"x").unwrap();
    std::fs::create_dir_all(folder.join("packs/pack-1")).unwrap();
    load(temp.path(), "m1").unwrap();
    assert!(!folder.join(".staging-abc").exists());
    assert!(folder.join("packs/pack-1").exists());
}

#[test]
fn paths_stay_inside_their_folders() {
    let dir = Path::new("/d");
    let pack = Pack {
        id: "../evil".into(),
        ..Pack::default()
    };
    let item = emoji("../../x.png", None);
    assert_eq!(pack_dir(dir, "p1"), Path::new("/d/packs/p1"));
    assert_eq!(
        image_path(dir, &pack, &item),
        Path::new("/d/packs/evil/x.png")
    );
}

#[test]
fn add_emotion_makes_names_unique() {
    let mut library = Library::default();
    let first = library.add_emotion();
    let second = library.add_emotion();
    assert_ne!(first, second);
    let names: Vec<_> = library.emotions[13..]
        .iter()
        .map(|e| e.name.as_str())
        .collect();
    assert_eq!(names, ["new emotion", "new emotion 2"]);
}

#[test]
fn rename_emotion_validates() {
    let mut library = Library::default();
    assert!(library.rename_emotion("happy", "  glad  ").is_ok());
    assert_eq!(library.emotions[0].name, "glad");
    assert!(library.rename_emotion("happy", "happy").is_ok());
    assert!(library.rename_emotion("happy", "SAD").is_err());
    assert!(library.rename_emotion("happy", "   ").is_err());
    assert!(library.rename_emotion("happy", "a[b").is_err());
    assert!(library.rename_emotion("happy", "a]b").is_err());
    assert!(library.rename_emotion("happy", "a\nb").is_err());
    assert!(library.rename_emotion("happy", &"x".repeat(33)).is_err());
    assert!(library.rename_emotion("happy", &"x".repeat(32)).is_ok());
    assert!(library.rename_emotion("nope", "fine").is_err());
}

#[test]
fn describe_and_delete_emotion() {
    let (mut library, pack) = library_with(vec![emoji("a.png", Some("happy"))]);
    library.describe_emotion("happy", "  Very glad ");
    assert_eq!(library.emotions[0].description, "Very glad");
    library.delete_emotion("happy");
    assert!(library.emotion_id("happy").is_none());
    assert_eq!(
        library.packs.iter().find(|p| p.id == pack).unwrap().emoji[0].emotion,
        None
    );
}

#[test]
fn move_emoji_and_pack_toggle() {
    let (mut library, pack) = library_with(vec![emoji("a.png", None)]);
    library.move_emoji(&pack, "a.png", Some("sad"));
    assert_eq!(library.packs[0].emoji[0].emotion.as_deref(), Some("sad"));
    library.move_emoji(&pack, "a.png", Some("missing"));
    assert_eq!(library.packs[0].emoji[0].emotion, None);
    library.set_pack_enabled(&pack, false);
    assert!(!library.packs[0].enabled);
}

#[test]
fn remove_pack_deletes_only_its_folder() {
    let temp = tempfile::tempdir().unwrap();
    let (mut library, pack) = library_with(vec![emoji("a.png", None)]);
    let other = library.add_pack("Other", None, vec![]);
    let folder = temp.path();
    for id in [&pack, &other] {
        std::fs::create_dir_all(pack_dir(folder, id)).unwrap();
        std::fs::write(pack_dir(folder, id).join("a.png"), b"x").unwrap();
    }
    assert!(library.remove_pack(folder, "../x").is_err());
    assert!(library.remove_pack(folder, "pack-missing").is_err());
    library.remove_pack(folder, &pack).unwrap();
    assert!(!pack_dir(folder, &pack).exists());
    assert!(pack_dir(folder, &other).join("a.png").exists());
    assert_eq!(library.packs.len(), 1);
    assert_eq!(library.packs[0].id, other);
}

#[test]
fn reset_keeps_default_named_emotions_and_unsorts_the_rest() {
    let (mut library, pack) = library_with(vec![
        emoji("a.png", Some("happy")),
        emoji("b.png", None),
        emoji("c.png", None),
    ]);
    let custom = library.add_emotion();
    library.rename_emotion(&custom, "Sad").unwrap_err();
    library.rename_emotion(&custom, "spicy").unwrap();
    library.move_emoji(&pack, "b.png", Some(&custom));
    // An emotion renamed to another default name keeps the emoji under that default.
    library.rename_emotion("sad", "sad-ish").unwrap();
    library.rename_emotion(&custom, "chill").unwrap_err();
    library.move_emoji(&pack, "c.png", Some("sad"));
    library.reset_emotions();
    assert_eq!(library.emotions.len(), 13);
    assert_eq!(library.packs[0].emoji[0].emotion.as_deref(), Some("happy"));
    assert_eq!(library.packs[0].emoji[1].emotion, None);
    assert_eq!(library.packs[0].emoji[2].emotion, None);
}

#[test]
fn reset_matches_by_name_not_id() {
    let (mut library, pack) = library_with(vec![emoji("a.png", None)]);
    library.rename_emotion("happy", "mine").unwrap();
    let id = library.add_emotion();
    library.rename_emotion(&id, "happy").unwrap();
    library.move_emoji(&pack, "a.png", Some(&id));
    library.reset_emotions();
    assert_eq!(library.packs[0].emoji[0].emotion.as_deref(), Some("happy"));
}

#[test]
fn add_pack_ids_and_names_are_unique() {
    let mut library = Library::default();
    let one = library.add_pack("Cats", None, vec![]);
    let two = library.add_pack("cats", Some("M".into()), vec![]);
    assert_ne!(one, two);
    assert!(one.starts_with("pack-") && one.len() == 13);
    assert_eq!(library.packs[1].name, "cats 2");
    assert_eq!(library.packs[1].sorted_by.as_deref(), Some("M"));
    assert!(library.packs[1].enabled);
    let blank = library.add_pack("  ", None, vec![]);
    assert_eq!(
        library.packs.iter().find(|p| p.id == blank).unwrap().name,
        "Emoji pack"
    );
}

#[test]
fn apply_sorting_resolves_names_case_insensitively() {
    let (mut library, pack) = library_with(vec![
        emoji("a.png", None),
        emoji("b.png", Some("happy")),
        emoji("c.png", Some("happy")),
        emoji("d.png", None),
    ]);
    let map: HashMap<String, Option<String>> = [
        ("a.png".to_owned(), Some("SAD".to_owned())),
        ("b.png".to_owned(), Some("no such".to_owned())),
        ("c.png".to_owned(), None),
    ]
    .into();
    library.apply_sorting(&pack, &map, Some("Model".into()));
    let emoji = &library.packs[0].emoji;
    assert_eq!(emoji[0].emotion.as_deref(), Some("sad"));
    assert_eq!(emoji[1].emotion, None);
    assert_eq!(emoji[2].emotion, None);
    assert_eq!(emoji[3].emotion, None);
    assert_eq!(library.packs[0].sorted_by.as_deref(), Some("Model"));
}

#[test]
fn known_hashes_usable_emotions_and_candidates() {
    let (mut library, pack) = library_with(vec![
        emoji("b.png", Some("happy")),
        emoji("a.png", Some("happy")),
        emoji("c.png", Some("sad")),
        emoji("d.png", None),
    ]);
    let off = library.add_pack("Off", None, vec![emoji("z.png", Some("love"))]);
    library.set_pack_enabled(&off, false);
    assert_eq!(library.known_hashes().len(), 5);
    assert!(library.known_hashes().contains("hash-z.png"));
    let usable: Vec<_> = library
        .usable_emotions()
        .iter()
        .map(|e| e.name.clone())
        .collect();
    assert_eq!(usable, ["happy", "sad"]);
    let files: Vec<_> = library
        .candidates("Happy")
        .iter()
        .map(|(_, e)| e.file.clone())
        .collect();
    assert_eq!(files, ["a.png", "b.png"]);
    assert!(library.candidates("love").is_empty());
    assert!(library.candidates("unknown").is_empty());
    let _ = pack;
}

#[test]
fn format_counts_in_fixed_order() {
    let mut gif = emoji("g.gif", None);
    gif.format = Format::Gif;
    let (library, _) = library_with(vec![emoji("a.png", None), gif, emoji("b.png", None)]);
    assert_eq!(
        format_counts(&library.packs[0]),
        vec![(Format::Png, 2), (Format::Gif, 1)]
    );
}

#[test]
fn context_block_is_none_when_off_or_nothing_usable() {
    let (mut library, pack) = library_with(vec![emoji("a.png", None)]);
    assert_eq!(context_block(&library), None);
    library.move_emoji(&pack, "a.png", Some("happy"));
    assert!(context_block(&library).is_some());
    library.set_pack_enabled(&pack, false);
    assert_eq!(context_block(&library), None);
    library.set_pack_enabled(&pack, true);
    library.enabled = false;
    assert_eq!(context_block(&library), None);
}

#[test]
fn context_block_lists_only_usable_emotions_and_guidance() {
    let (mut library, _) = library_with(vec![
        emoji("a.png", Some("happy")),
        emoji("b.png", Some("sad")),
    ]);
    let block = context_block(&library).unwrap();
    assert!(block.starts_with("<emoji>") && block.ends_with("</emoji>"));
    assert!(block.contains(":emo[<emotion>]"));
    assert!(block.contains("- happy: Glad, pleased, things went well"));
    assert!(block.contains("- sad: Bad news, a failure, something lost"));
    assert!(!block.contains("- love"));
    assert!(block.contains("Sometimes: at most one emoji per reply, in about one reply in three."));
    assert!(block.contains(
        "Never inside code, code blocks, commands, commit messages, PR or issue text, reports, files you write or tool input."
    ));
    library.frequency = Frequency::Rarely;
    assert!(context_block(&library)
        .unwrap()
        .contains("Rarely: at most one emoji, in about one reply in five, only for a clear mood."));
    library.frequency = Frequency::Often;
    assert!(context_block(&library)
        .unwrap()
        .contains("Often: one emoji in most conversational replies, never more than two."));
}

#[test]
fn mono_context_carries_the_block_after_memory() {
    let mut roster = crate::mono::Roster::default();
    let mono = roster.create(vec!["/p".into()]).clone();
    let files = crate::mono::files::AgentFiles {
        id: mono.id.clone(),
        dir: String::new(),
        soul: None,
        soul_hash: String::new(),
        memory: String::new(),
        memory_hash: String::new(),
        memory_path: String::new(),
        topics: Vec::new(),
    };
    let exe = Path::new("/bin/app");
    let without = crate::mono::context(&mono, &files, exe, true, None);
    assert!(!without.contains("<emoji>"));
    let with = crate::mono::context(&mono, &files, exe, true, Some("<emoji>\nx\n</emoji>"));
    let memory = with.find("</memory>").unwrap();
    let block = with.find("<emoji>").unwrap();
    let end = with.find("</monocode_context>").unwrap();
    assert!(memory < block && block < end);
}

#[test]
fn token_find_reads_names() {
    let tokens = find("a :emo[happy] b :emo[ Sad ] :emo[] :emo[a[b] :emo[x\ny] :emo[ok]");
    let names: Vec<_> = tokens.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(names, ["happy", "Sad", "", "ok"]);
    assert_eq!(&"a :emo[happy] b"[tokens[0].range.clone()], ":emo[happy]");
    let long = format!(":emo[{}]", "x".repeat(65));
    assert!(find(&long).is_empty());
    let fits = format!(":emo[{}]", "x".repeat(64));
    assert_eq!(find(&fits).len(), 1);
}

#[test]
fn strip_removes_tokens_outside_code() {
    assert_eq!(strip("Done :emo[happy] now"), "Done now");
    assert_eq!(strip("Done :emo[happy]"), "Done");
    assert_eq!(strip("Done.\n:emo[happy]\nNext"), "Done.\nNext");
    assert_eq!(strip("  :emo[a] :emo[b]  \nText"), "Text");
    assert_eq!(strip("a :emo[x] b :emo[y] c"), "a b c");
    assert_eq!(strip("no tokens"), "no tokens");
}

#[test]
fn strip_leaves_code_alone() {
    let fenced = "x\n```\n:emo[happy]\n```\ny :emo[a]\n";
    assert_eq!(strip(fenced), "x\n```\n:emo[happy]\n```\ny\n");
    let tilde = "~~~\n:emo[happy]\n~~~\n";
    assert_eq!(strip(tilde), tilde);
    let longer = "````\n```\n:emo[a]\n```\n:emo[b]\n````\n:emo[c]\nend";
    assert_eq!(strip(longer), "````\n```\n:emo[a]\n```\n:emo[b]\n````\nend");
    let inline = "run `:emo[a]` and ``x :emo[b] ` y`` then :emo[c]";
    assert_eq!(strip(inline), "run `:emo[a]` and ``x :emo[b] ` y`` then");
    let unclosed = "```\n:emo[a]\nstill code";
    assert_eq!(strip(unclosed), unclosed);
}

#[test]
fn incomplete_tail_finds_unfinished_tokens() {
    assert_eq!(incomplete_tail("Hello :"), Some(6));
    assert_eq!(incomplete_tail("Hello :e"), Some(6));
    assert_eq!(incomplete_tail("Hello :emo"), Some(6));
    assert_eq!(incomplete_tail("Hello :emo["), Some(6));
    assert_eq!(incomplete_tail("Hello :emo[hap"), Some(6));
    assert_eq!(incomplete_tail("Hello :emo[happy]"), None);
    assert_eq!(incomplete_tail("Hello :x"), None);
    assert_eq!(incomplete_tail("Hello"), None);
    assert_eq!(incomplete_tail("Hello :emo[a\nb"), None);
    assert_eq!(incomplete_tail("Hello :emo[a:"), Some(6));
    assert_eq!(incomplete_tail("héllo :em"), Some(7));
}

#[test]
fn incomplete_tail_ignores_code() {
    assert_eq!(incomplete_tail("```\n:emo[ha"), None);
    assert_eq!(incomplete_tail("text `:emo[ha"), None);
    assert_eq!(incomplete_tail("`a` :emo[ha"), Some(4));
}

const PINNED_SEED: u64 = 0x8b7e_502d_ca56_4b79;

#[test]
fn seed_is_stable_and_varies() {
    assert_eq!(seed("s", 1, 0), seed("s", 1, 0));
    assert_ne!(seed("s", 1, 0), seed("s", 1, 1));
    assert_ne!(seed("s", 1, 0), seed("s", 2, 0));
    assert_ne!(seed("s", 1, 0), seed("t", 1, 0));
    // A fixed value guards against swapping the hash for a per-process one.
    assert_eq!(seed("session", 3, 2), PINNED_SEED);
}

#[test]
fn pick_is_deterministic_and_respects_state() {
    let (mut library, pack) = library_with(vec![
        emoji("a.png", Some("happy")),
        emoji("b.png", Some("happy")),
        emoji("c.png", Some("happy")),
    ]);
    let off = library.add_pack("Off", None, vec![emoji("z.png", Some("happy"))]);
    library.set_pack_enabled(&off, false);
    for s in 0..20u64 {
        let (p, e) = pick(&library, "HAPPY", s).unwrap();
        assert_eq!(p.id, pack);
        assert_ne!(e.file, "z.png");
        assert_eq!(pick(&library, "happy", s).unwrap().1.file, e.file);
    }
    assert_eq!(pick(&library, "happy", 0).unwrap().1.file, "a.png");
    assert_eq!(pick(&library, "happy", 4).unwrap().1.file, "b.png");
    assert!(pick(&library, "unknown", 0).is_none());
    assert!(pick(&library, "", 0).is_none());
    assert!(pick(&library, "sad", 0).is_none());
    library.enabled = false;
    assert!(pick(&library, "happy", 0).is_none());
}
