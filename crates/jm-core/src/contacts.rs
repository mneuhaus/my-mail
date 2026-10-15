//! People to suggest as recipients, gathered from the mail itself: whom a mailbox wrote to (Sent
//! Items) and who wrote to it (inbox). No contacts or people scope is needed, so the sign-ins taken
//! over from ms365-mail keep working.

use std::collections::HashMap;

use chrono::{DateTime, Utc};

use crate::model::{Message, Recipient};

/// A sent mail counts this much more than a received one: you write to the people you deal with.
const SENT_WEIGHT: f64 = 3.0;
/// A mail's weight halves with every this many days of age.
const HALF_LIFE_DAYS: f64 = 90.0;
/// Senders nobody writes back to.
const MACHINES: [&str; 7] = ["noreply", "no-reply", "no_reply", "donotreply", "do-not-reply", "mailer-daemon", "postmaster"];

#[derive(Debug, Clone, PartialEq)]
pub struct Contact {
    pub name: String,
    pub address: String,
    score: f64,
}

impl Contact {
    pub fn recipient(&self) -> Recipient {
        Recipient { name: self.name.clone(), address: self.address.clone() }
    }

    /// Whether `query` (lower case) starts the address, its domain, the name or a word of it.
    fn matches(&self, query: &str) -> bool {
        let address = self.address.to_lowercase();
        let name = self.name.to_lowercase();
        address.starts_with(query)
            || address.split_once('@').is_some_and(|(_, domain)| domain.starts_with(query))
            || name.starts_with(query)
            || name.split([' ', '-', '.', ',', '(', '"']).any(|word| word.starts_with(query))
    }
}

/// Everyone in the mail, best first: the recipients of `sent`, the senders of `received`.
/// Recent and frequent mail counts most; `own` addresses and machine senders stay out.
pub fn rank(sent: &[Message], received: &[Message], own: &[&str], now: DateTime<Utc>) -> Vec<Contact> {
    struct Seen {
        contact: Contact,
        named_at: Option<DateTime<Utc>>,
    }
    let mut seen: HashMap<String, Seen> = HashMap::new();
    let mut count = |person: &Recipient, at: Option<DateTime<Utc>>, weight: f64| {
        let address = person.address.trim();
        let key = address.to_lowercase();
        let local = key.split('@').next().unwrap_or_default();
        if !address.contains('@') || own.iter().any(|o| o.eq_ignore_ascii_case(address)) || MACHINES.iter().any(|m| local.contains(m)) {
            return;
        }
        let age_days = at.map_or(365.0, |at| (now - at).num_hours().max(0) as f64 / 24.0);
        let entry = seen.entry(key).or_insert_with(|| Seen {
            contact: Contact { name: String::new(), address: address.to_string(), score: 0.0 },
            named_at: None,
        });
        entry.contact.score += weight * 0.5f64.powf(age_days / HALF_LIFE_DAYS);
        // the newest name wins: people fix how their name is written, rarely the other way
        let name = person.name.trim();
        if !name.is_empty() && !name.eq_ignore_ascii_case(address) && (entry.named_at.is_none() || at > entry.named_at) {
            entry.contact.name = name.to_string();
            entry.named_at = at.or(entry.named_at);
        }
    };
    for m in sent {
        for person in m.to_recipients.iter().chain(&m.cc_recipients).chain(&m.bcc_recipients) {
            count(person, m.date(), SENT_WEIGHT);
        }
    }
    for m in received {
        if let Some(person) = &m.from {
            count(person, m.date(), 1.0);
        }
    }
    let mut out: Vec<Contact> = seen.into_values().map(|s| s.contact).collect();
    out.sort_by(|a, b| b.score.total_cmp(&a.score).then_with(|| a.address.cmp(&b.address)));
    out
}

/// Up to `limit` contacts for what is being typed, leaving out the addresses in `taken`.
pub fn suggest<'a>(contacts: &'a [Contact], query: &str, taken: &[String], limit: usize) -> Vec<&'a Contact> {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return Vec::new();
    }
    contacts
        .iter()
        .filter(|c| c.matches(&query) && !taken.iter().any(|t| t.eq_ignore_ascii_case(&c.address)))
        .take(limit)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    fn person(name: &str, address: &str) -> Recipient {
        Recipient { name: name.into(), address: address.into() }
    }

    fn mail(days_ago: i64, from: Option<Recipient>, to: Vec<Recipient>) -> Message {
        let at = Utc::now() - Duration::days(days_ago);
        Message { received_date_time: Some(at), from, to_recipients: to, ..Default::default() }
    }

    #[test]
    fn written_to_beats_heard_from_and_recent_beats_old() {
        let sent = vec![
            mail(2, None, vec![person("Lena Hoffmann", "lena@harbor.example")]),
            mail(400, None, vec![person("Old Friend", "old@friend.example")]),
        ];
        let received = vec![
            mail(1, Some(person("Tom Becker", "tom@becker.example")), vec![]),
            mail(1, Some(person("Shop", "noreply@shop.example")), vec![]),
            mail(1, Some(person("Me", "ME@northwind.example")), vec![]),
        ];
        let ranked = rank(&sent, &received, &["me@northwind.example"], Utc::now());
        let order: Vec<&str> = ranked.iter().map(|c| c.address.as_str()).collect();
        assert_eq!(order, ["lena@harbor.example", "tom@becker.example", "old@friend.example"]);
    }

    #[test]
    fn same_address_adds_up_and_keeps_the_newest_name() {
        let sent = vec![
            mail(30, None, vec![person("anna", "Anna@Schmidt.example")]),
            mail(3, None, vec![person("Anna Schmidt", "anna@schmidt.example")]),
            mail(1, None, vec![person("anna@schmidt.example", "anna@schmidt.example")]),
        ];
        let ranked = rank(&sent, &[], &[], Utc::now());
        assert_eq!(ranked.len(), 1);
        assert_eq!(ranked[0].name, "Anna Schmidt");
    }

    #[test]
    fn suggests_by_name_word_address_and_domain() {
        let contacts = rank(
            &[mail(1, None, vec![person("Lena Hoffmann", "lena@harbor.example"), person("", "info@hoffmann-bau.example")])],
            &[],
            &[],
            Utc::now(),
        );
        let found = |q: &str, taken: &[String]| -> Vec<String> {
            suggest(&contacts, q, taken, 5).iter().map(|c| c.address.clone()).collect()
        };
        assert_eq!(found("hoff", &[]), ["info@hoffmann-bau.example", "lena@harbor.example"]);
        assert_eq!(found("LEN", &[]), ["lena@harbor.example"]);
        assert_eq!(found("harbor", &[]), ["lena@harbor.example"]);
        assert_eq!(found("hoff", &["LENA@harbor.example".into()]), ["info@hoffmann-bau.example"]);
        assert!(found("  ", &[]).is_empty());
        assert!(found("arbor", &[]).is_empty());
    }
}
