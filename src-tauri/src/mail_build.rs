//! The outgoing message as bytes (spec section 8.3).
//!
//! `Bcc` is not a field of [`Outgoing`]. A blind copy is an envelope
//! recipient only, so it has no way into the headers: the type cannot express
//! it, which is stronger than remembering to skip it. Leaking one tells every
//! other recipient who was secretly copied.

use mail_builder::headers::address::Address;
use mail_builder::headers::date::Date;
use mail_builder::headers::message_id::MessageId;
use mail_builder::mime::MimePart;
use mail_builder::MessageBuilder;

use crate::aws_ses::Destination;
use crate::mail_store::Recipient;

/// Spec 8.3. The newest ids matter most for threading and the oldest one is
/// the root, so a longer chain loses its middle.
pub(crate) const MAX_REFERENCES: usize = 20;

/// SES's raw-message limit: AWS documents 40 MB, which is 40,000,000 bytes and
/// not 40 MiB. Base64 has already inflated attachments by 4/3 in the bytes we
/// measure, so this is checked on the finished message.
pub(crate) const SES_RAW_LIMIT: usize = 40_000_000;

pub(crate) struct LoadedAttachment {
    pub filename: String,
    pub mime: String,
    /// `Some` for an inline image: the `Content-ID`, without angle brackets.
    pub cid: Option<String>,
    pub bytes: Vec<u8>,
}

pub(crate) struct Outgoing<'a> {
    pub from: &'a str,
    pub to: &'a [Recipient],
    pub cc: &'a [Recipient],
    pub subject: &'a str,
    pub text: &'a str,
    pub html: &'a str,
    /// Bracket-free, like every id we store.
    pub message_id: &'a str,
    pub in_reply_to: Option<&'a str>,
    pub references: &'a [String],
    /// Epoch seconds.
    pub date: i64,
    pub attachments: &'a [LoadedAttachment],
}

/// First id kept (the thread root), then the newest. Repeats are dropped
/// first so the cap never spends a slot on an id already present.
pub(crate) fn cap_references(ids: &[String]) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    let unique: Vec<&String> = ids
        .iter()
        .filter(|id| !id.is_empty() && seen.insert(id.as_str()))
        .collect();
    if unique.len() <= MAX_REFERENCES {
        return unique.into_iter().cloned().collect();
    }
    let tail = unique.len() - (MAX_REFERENCES - 1);
    std::iter::once(unique[0])
        .chain(unique[tail..].iter().copied())
        .cloned()
        .collect()
}

/// Header values are single lines. mail-builder encodes what it is given, but
/// a stray CR or LF in a subject or name is never intended.
fn one_line(text: &str) -> String {
    text.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect::<String>()
        .trim()
        .to_string()
}

pub(crate) fn valid_address(addr: &str) -> bool {
    let Some((local, domain)) = addr.rsplit_once('@') else {
        return false;
    };
    let ok = |c: char| c.is_ascii_graphic() && !"<>(),;:\"\\[]".contains(c);
    !local.is_empty()
        && !domain.is_empty()
        && !local.contains(' ')
        && addr.chars().all(ok)
        && !domain.starts_with('.')
        && !domain.ends_with('.')
}

fn address_list(list: &[Recipient]) -> Address<'static> {
    Address::new_list(
        list.iter()
            .map(|r| {
                Address::new_address(
                    r.name.as_deref().map(one_line).filter(|n| !n.is_empty()),
                    r.addr.trim().to_string(),
                )
            })
            .collect(),
    )
}

/// A filename comes from the user's disk but ends up in a header, and a
/// recipient's mail client may write it back to a disk: no separators.
pub(crate) fn safe_filename(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| {
            if c.is_control() || c == '/' || c == '\\' {
                '_'
            } else {
                c
            }
        })
        .collect();
    let cleaned = cleaned.trim().trim_start_matches('.').to_string();
    if cleaned.is_empty() {
        "attachment".to_string()
    } else {
        cleaned
    }
}

fn safe_mime(mime: &str) -> String {
    let m = mime.trim().to_ascii_lowercase();
    let token = |s: &str| {
        !s.is_empty()
            && s.chars()
                .all(|c| c.is_ascii_alphanumeric() || "+-.".contains(c))
    };
    match m.split_once('/') {
        Some((a, b)) if token(a) && token(b) => m,
        _ => "application/octet-stream".to_string(),
    }
}

/// multipart/mixed( multipart/alternative( text/plain,
/// multipart/related( text/html, inline images ) ), attachments ).
/// Each wrapper is left out when it would hold one child.
pub(crate) fn build(o: &Outgoing) -> Result<Vec<u8>, String> {
    let (inline, files): (Vec<_>, Vec<_>) = o.attachments.iter().partition(|a| a.cid.is_some());

    let html = MimePart::new("text/html", o.html.to_string());
    let html_side = if inline.is_empty() {
        html
    } else {
        let mut parts = vec![html];
        for image in &inline {
            let cid = image.cid.clone().unwrap_or_default();
            parts.push(
                MimePart::new(safe_mime(&image.mime), image.bytes.clone())
                    .inline()
                    .cid(cid),
            );
        }
        MimePart::new("multipart/related", parts)
    };
    let alternative = MimePart::new(
        "multipart/alternative",
        vec![MimePart::new("text/plain", o.text.to_string()), html_side],
    );
    let body = if files.is_empty() {
        alternative
    } else {
        let mut parts = vec![alternative];
        for file in &files {
            parts.push(
                MimePart::new(safe_mime(&file.mime), file.bytes.clone())
                    .attachment(safe_filename(&file.filename)),
            );
        }
        MimePart::new("multipart/mixed", parts)
    };

    let mut message = MessageBuilder::new()
        .from(o.from.trim().to_string())
        .to(address_list(o.to))
        .subject(one_line(o.subject))
        .date(Date::new(o.date))
        .message_id(MessageId::new(o.message_id.to_string()));
    if !o.cc.is_empty() {
        message = message.cc(address_list(o.cc));
    }
    if let Some(parent) = o.in_reply_to {
        message = message.in_reply_to(MessageId::new(parent.to_string()));
    }
    let references = cap_references(o.references);
    if !references.is_empty() {
        message = message.references(MessageId::new_list(references.into_iter()));
    }
    message
        .body(body)
        .write_to_vec()
        .map_err(|_| "Could not assemble the message.".to_string())
}

/// Envelope recipients, each address once. An address already in To is not
/// repeated under Cc or Bcc, and one in Cc is not repeated under Bcc, so
/// nobody gets two copies and a Bcc never shadows a visible recipient.
pub(crate) fn destination(to: &[Recipient], cc: &[Recipient], bcc: &[Recipient]) -> Destination {
    let mut seen = std::collections::HashSet::new();
    let mut take = |list: &[Recipient]| -> Vec<String> {
        list.iter()
            .map(|r| r.addr.trim().to_string())
            .filter(|a| seen.insert(a.to_ascii_lowercase()))
            .collect()
    };
    let to = take(to);
    let cc = take(cc);
    let bcc = take(bcc);
    Destination { to, cc, bcc }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mail_parser::{MessageParser, MimeHeaders};

    fn who(addr: &str) -> Recipient {
        Recipient {
            name: None,
            addr: addr.into(),
        }
    }

    fn outgoing<'a>(
        to: &'a [Recipient],
        refs: &'a [String],
        attachments: &'a [LoadedAttachment],
    ) -> Outgoing<'a> {
        Outgoing {
            from: "owner@byptah.com",
            to,
            cc: &[],
            subject: "Hello",
            text: "plain body",
            html: "<p>html body</p>",
            message_id: "abc@byptah.com",
            in_reply_to: None,
            references: refs,
            date: 1_700_000_000,
            attachments,
        }
    }

    fn ids(n: usize) -> Vec<String> {
        (1..=n).map(|i| format!("m{i}@x")).collect()
    }

    #[test]
    fn references_are_capped_at_twenty_keeping_the_root_and_the_newest() {
        let capped = cap_references(&ids(30));
        assert_eq!(capped.len(), 20);
        assert_eq!(capped[0], "m1@x");
        assert_eq!(capped[1], "m12@x");
        assert_eq!(capped[19], "m30@x");
        assert_eq!(cap_references(&ids(20)), ids(20));
    }

    #[test]
    fn a_repeated_id_does_not_use_up_a_slot() {
        let mut list = ids(20);
        list.push("m5@x".into());
        assert_eq!(cap_references(&list), ids(20));
    }

    #[test]
    fn the_built_message_carries_at_most_twenty_references() {
        let to = [who("a@x.com")];
        let refs = ids(45);
        let raw = build(&outgoing(&to, &refs, &[])).unwrap();
        let parsed = MessageParser::default().parse(&raw).unwrap();
        let header = parsed.references().as_text_list().unwrap();
        assert_eq!(header.len(), 20);
        assert_eq!(header[0], "m1@x");
        assert_eq!(header[19], "m45@x");
        // The header is written with brackets, the store keeps none.
        let text = String::from_utf8_lossy(&raw);
        assert!(text.contains("<m45@x>"), "{text}");
    }

    #[test]
    fn bcc_is_in_the_destination_and_nowhere_in_the_message() {
        let to = [who("a@x.com")];
        let cc = [who("c@x.com")];
        let bcc = [who("secret@hidden.org")];
        let raw = build(&Outgoing {
            cc: &cc,
            ..outgoing(&to, &[], &[])
        })
        .unwrap();
        let text = String::from_utf8_lossy(&raw).to_lowercase();
        assert!(!text.contains("secret@hidden.org"), "{text}");
        assert!(!text.contains("bcc:"), "{text}");
        let dest = destination(&to, &cc, &bcc);
        assert_eq!(dest.bcc, vec!["secret@hidden.org".to_string()]);
        assert_eq!(dest.to, vec!["a@x.com".to_string()]);
        assert_eq!(dest.cc, vec!["c@x.com".to_string()]);
    }

    #[test]
    fn a_recipient_is_listed_once_and_the_visible_field_wins() {
        let dest = destination(
            &[who("A@x.com")],
            &[who("a@x.com"), who("c@x.com")],
            &[who("c@x.com"), who("d@x.com")],
        );
        assert_eq!(dest.to, vec!["A@x.com".to_string()]);
        assert_eq!(dest.cc, vec!["c@x.com".to_string()]);
        assert_eq!(dest.bcc, vec!["d@x.com".to_string()]);
    }

    #[test]
    fn attachment_bytes_survive_the_base64_round_trip() {
        // Every byte value, and a length that is not a multiple of three.
        let bytes: Vec<u8> = (0..=255u8).cycle().take(1000).collect();
        let to = [who("a@x.com")];
        let files = [LoadedAttachment {
            filename: "data.bin".into(),
            mime: "application/octet-stream".into(),
            cid: None,
            bytes: bytes.clone(),
        }];
        let raw = build(&outgoing(&to, &[], &files)).unwrap();
        let parsed = MessageParser::default().parse(&raw).unwrap();
        assert_eq!(parsed.attachment_count(), 1);
        let part = parsed.attachment(0).unwrap();
        assert_eq!(part.attachment_name(), Some("data.bin"));
        assert_eq!(part.contents(), bytes.as_slice());
    }

    #[test]
    fn an_inline_image_is_related_to_the_html_by_content_id() {
        let to = [who("a@x.com")];
        let cid = "mono-7.att1@monoptah.local";
        let files = [LoadedAttachment {
            filename: "pic.png".into(),
            mime: "image/png".into(),
            cid: Some(cid.into()),
            bytes: vec![1, 2, 3, 4, 5],
        }];
        let raw = build(&Outgoing {
            html: &format!("<img src=\"cid:{cid}\">"),
            ..outgoing(&to, &[], &files)
        })
        .unwrap();
        let text = String::from_utf8_lossy(&raw);
        assert!(text.contains("multipart/related"), "{text}");
        assert!(text.contains(&format!("Content-ID: <{cid}>")), "{text}");
        let parsed = MessageParser::default().parse(&raw).unwrap();
        let image = parsed
            .attachments()
            .find(|p| p.content_id() == Some(cid))
            .expect("image part");
        assert_eq!(image.contents(), &[1, 2, 3, 4, 5]);
        assert_eq!(parsed.body_text(0).as_deref(), Some("plain body"));
    }

    #[test]
    fn headers_cannot_be_split_by_a_subject_or_a_filename() {
        let to = [Recipient {
            name: Some("Eve\r\nBcc: evil@x.com".into()),
            addr: "a@x.com".into(),
        }];
        let files = [LoadedAttachment {
            filename: "a\r\nBcc: evil@x.com".into(),
            mime: "text/plain\r\nX: y".into(),
            cid: None,
            bytes: b"hi".to_vec(),
        }];
        let raw = build(&Outgoing {
            subject: "Hi\r\nBcc: evil@x.com",
            ..outgoing(&to, &[], &files)
        })
        .unwrap();
        let text = String::from_utf8_lossy(&raw).to_lowercase();
        assert!(!text.contains("\r\nbcc:"), "{text}");
        assert!(!text.contains("\nx: y"), "{text}");
    }

    #[test]
    fn a_filename_cannot_carry_a_path() {
        assert_eq!(safe_filename("../../etc/passwd"), "_.._etc_passwd");
        assert_eq!(safe_filename("a\\b/c.txt"), "a_b_c.txt");
        assert_eq!(safe_filename("  .hidden"), "hidden");
        assert_eq!(safe_filename("\r\n"), "__");
        assert_eq!(safe_filename("..."), "attachment");
        let to = [who("a@x.com")];
        let files = [LoadedAttachment {
            filename: "dir/inner.txt".into(),
            mime: "text/plain".into(),
            cid: None,
            bytes: b"hi".to_vec(),
        }];
        let raw = build(&outgoing(&to, &[], &files)).unwrap();
        let parsed = MessageParser::default().parse(&raw).unwrap();
        assert_eq!(
            parsed.attachment(0).unwrap().attachment_name(),
            Some("dir_inner.txt")
        );
    }

    #[test]
    fn address_validation_rejects_what_would_break_a_header() {
        assert!(valid_address("a.b+c@sub.example.com"));
        assert!(!valid_address("a b@x.com"));
        assert!(!valid_address("a@x.com>, evil@y.com"));
        assert!(!valid_address("no-at-sign"));
        assert!(!valid_address("@x.com"));
        assert!(!valid_address("a@"));
        assert!(!valid_address("a@x.com\r\nBcc: e@x.com"));
    }

    #[test]
    fn the_message_id_is_written_with_brackets_from_a_bracket_free_id() {
        let to = [who("a@x.com")];
        let raw = build(&Outgoing {
            in_reply_to: Some("parent@x"),
            ..outgoing(&to, &[], &[])
        })
        .unwrap();
        let text = String::from_utf8_lossy(&raw);
        assert!(text.contains("Message-ID: <abc@byptah.com>"), "{text}");
        assert!(text.contains("In-Reply-To: <parent@x>"), "{text}");
    }
}
