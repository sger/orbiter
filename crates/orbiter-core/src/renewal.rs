//! What a free team's seven days have left, and for which build.
//!
//! A personal team's provisioning profile expires seven days after it is issued, and the installed
//! app then refuses to launch with no explanation on the phone. Orbiter said so once, during
//! signing, and never mentioned it again — so the first signal a tester got was the app dying.
//!
//! This module is the memory of that. It records nothing about how a build was made and nothing
//! that identifies a person: no IPA path, no device UDID, no Apple ID, and not the team identifier
//! itself. A personal team is named after its owner, so what is stored for matching is a tag
//! derived from the identifier rather than the identifier — enough to tell "this record is about
//! the team on screen" apart from "this record is about some other team", and not a readable list
//! of whose Apple accounts have been used on this Mac.
//!
//! Nothing here contacts Apple, re-signs, or schedules anything. It reads a clock and a small file.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    io::Write,
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};

/// Enough records that a person signing for several testers does not lose the earlier ones, and
/// few enough that the file stays a glance rather than a history.
const MAX_RECORDS: usize = 32;
const MAX_BYTES: u64 = 16 * 1024;
/// Past this, a count of days is no longer information. The profile is gone, the App ID window has
/// long since turned over, and "expired 94 days ago" tells a person nothing "expired" did not.
const LONG_AGO_DAYS: i64 = 30;
const DAY: i64 = 86_400;

/// One installed build, and when it stops launching.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Record {
    /// Derived from the team identifier; see the module comment. Never the identifier itself.
    pub team_tag: String,
    /// The rewritten identifier, which is already on screen during signing.
    pub identifier: String,
    /// So the line names a build a person recognises rather than a reverse-DNS string.
    pub app_name: String,
    /// The Watch decision this build was made under. Recorded so the record says what was
    /// actually installed; it is not replayed into the interface, because removing a Watch app is
    /// a consequence a person accepts each time rather than one a file decides for them.
    pub watch: String,
    pub expires_unix: i64,
    pub installed_unix: i64,
}

/// Where the seven days stand right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Standing {
    /// Whole days remaining, rounded down: six and a half days left is six.
    Valid { days: i64 },
    /// Under twenty-four hours. The one state worth acting on before it is too late.
    ExpiresToday,
    /// Days since it went, rounded down. Zero means it went earlier today.
    Expired { days: i64 },
    /// Long enough ago that the number stopped meaning anything.
    LongExpired,
}

/// Whether a record is about the build on screen. A reassuring "5 days left" that turns out to
/// describe a different app is worse than saying nothing, so the interface needs to know.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Bearing {
    SameApp,
    OtherApp,
    OtherTeam,
    /// Nothing is selected yet, so there is nothing to compare against.
    Unknown,
}

/// A record plus everything derived from it, including the exact sentence to show. The sentence is
/// built here rather than in the interface so a log line, a screenshot and the window agree.
#[derive(Debug, Clone, Serialize)]
pub struct Status {
    pub identifier: String,
    pub app_name: String,
    pub watch: String,
    pub standing: Standing,
    pub bearing: Bearing,
    pub sentence: String,
    /// True only for a record about the build on screen that has run out. The interface promotes
    /// this one above the signing controls; everything else stays a quiet line.
    pub urgent: bool,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Ledger {
    records: Vec<Record>,
}

/// The matching tag for a team identifier.
pub fn tag(team_id: &str) -> String {
    let digest = Sha256::digest(team_id.as_bytes());
    digest[..8]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

pub fn now_unix(now: SystemTime) -> i64 {
    match now.duration_since(UNIX_EPOCH) {
        Ok(since) => since.as_secs() as i64,
        // A clock set before 1970 is not a reason to fail; it is a reason to know nothing.
        Err(_) => 0,
    }
}

pub fn standing(record: &Record, now: SystemTime) -> Standing {
    let remaining = record.expires_unix - now_unix(now);
    if remaining <= 0 {
        let days = (-remaining) / DAY;
        if days >= LONG_AGO_DAYS {
            Standing::LongExpired
        } else {
            Standing::Expired { days }
        }
    } else if remaining < DAY {
        Standing::ExpiresToday
    } else {
        Standing::Valid {
            days: remaining / DAY,
        }
    }
}

pub fn bearing(record: &Record, team_tag: Option<&str>, identifier: Option<&str>) -> Bearing {
    match (team_tag, identifier) {
        (Some(team), _) if team != record.team_tag => Bearing::OtherTeam,
        (Some(_), Some(id)) if id == record.identifier => Bearing::SameApp,
        (Some(_), Some(_)) => Bearing::OtherApp,
        (Some(_), None) => Bearing::Unknown,
        (None, _) => Bearing::Unknown,
    }
}

/// The line the banner shows. Never a countdown about a build that is not the one on screen.
fn sentence(record: &Record, standing: Standing, bearing: Bearing) -> String {
    let name = &record.app_name;
    match bearing {
        Bearing::OtherTeam => {
            format!("The last build Orbiter installed, {name}, was signed for a different team.")
        }
        Bearing::OtherApp => {
            format!("Orbiter last installed a different build, {name}, for this team.")
        }
        Bearing::SameApp | Bearing::Unknown => match standing {
            Standing::Valid { days } => format!(
                "{name} was installed from this team and stops launching in {days} {}.",
                if days == 1 { "day" } else { "days" }
            ),
            Standing::ExpiresToday => {
                format!("{name} stops launching today. Re-sign and install it again.")
            }
            Standing::Expired { days: 0 } => {
                format!("{name} has stopped launching. Re-sign and install it again.")
            }
            Standing::Expired { days } => format!(
                "{name} stopped launching {days} {} ago. Re-sign and install it again.",
                if days == 1 { "day" } else { "days" }
            ),
            Standing::LongExpired => {
                format!("{name} expired some time ago. Re-sign and install it again.")
            }
        },
    }
}

fn read(path: &Path) -> Ledger {
    // A record of when something expires must never be the reason the app will not open. Every
    // failure here is the same answer: nothing is known.
    let Ok(meta) = std::fs::metadata(path) else {
        return Ledger::default();
    };
    if meta.len() > MAX_BYTES {
        return Ledger::default();
    }
    let Ok(bytes) = std::fs::read(path) else {
        return Ledger::default();
    };
    if bytes.len() as u64 > MAX_BYTES {
        return Ledger::default();
    }
    serde_json::from_slice(&bytes).unwrap_or_default()
}

fn write(path: &Path, ledger: &Ledger) -> Result<(), String> {
    let parent = path.parent().ok_or("Invalid renewal storage location.")?;
    std::fs::create_dir_all(parent).map_err(|_| "Cannot create private renewal storage.")?;
    let mut temp =
        tempfile::NamedTempFile::new_in(parent).map_err(|_| "Cannot create the renewal record.")?;
    serde_json::to_writer(&mut temp, ledger).map_err(|_| "Cannot encode the renewal record.")?;
    temp.flush()
        .map_err(|_| "Cannot flush the renewal record.")?;
    temp.as_file()
        .sync_all()
        .map_err(|_| "Cannot sync the renewal record.")?;
    temp.persist(path)
        .map_err(|_| "Cannot save the renewal record.")?;
    Ok(())
}

/// Remember an installed build, replacing any earlier record of the same build for the same team.
pub fn remember(path: &Path, record: Record) -> Result<(), String> {
    let mut ledger = read(path);
    ledger
        .records
        .retain(|held| held.team_tag != record.team_tag || held.identifier != record.identifier);
    ledger.records.push(record);
    // Newest last; drop from the front when the file would grow past a glance.
    ledger.records.sort_by_key(|held| held.installed_unix);
    let excess = ledger.records.len().saturating_sub(MAX_RECORDS);
    ledger.records.drain(..excess);
    write(path, &ledger)
}

/// Forget everything. The only way to remove a record, and it removes all of them: a partial
/// forget that leaves a person guessing which builds are still remembered is not a clearer choice.
pub fn forget(path: &Path) -> Result<(), String> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err("Cannot remove the renewal record.".into()),
    }
}

/// The record worth showing for the team and build on screen, if there is one.
///
/// A record about the selected team and build wins; failing that, the most recently installed
/// record for the selected team; failing that, the most recent record of any kind, which the
/// interface will show as being about something else.
pub fn status(
    path: &Path,
    team_id: Option<&str>,
    identifier: Option<&str>,
    now: SystemTime,
) -> Option<Status> {
    let ledger = read(path);
    let team_tag = team_id.map(tag);
    let tag_ref = team_tag.as_deref();
    let best = ledger
        .records
        .iter()
        .max_by_key(|record| {
            let rank = match bearing(record, tag_ref, identifier) {
                Bearing::SameApp => 3,
                Bearing::Unknown => 2,
                Bearing::OtherApp => 1,
                Bearing::OtherTeam => 0,
            };
            (rank, record.installed_unix)
        })?
        .clone();
    let standing = standing(&best, now);
    let bearing = bearing(&best, tag_ref, identifier);
    Some(Status {
        sentence: sentence(&best, standing, bearing),
        urgent: matches!(
            (bearing, standing),
            (
                Bearing::SameApp | Bearing::Unknown,
                Standing::ExpiresToday | Standing::Expired { .. } | Standing::LongExpired
            )
        ),
        identifier: best.identifier,
        app_name: best.app_name,
        watch: best.watch,
        standing,
        bearing,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    const TEAM: &str = "T8B3X5UL5W";

    fn at(unix: i64) -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(unix as u64)
    }
    fn record(expires_unix: i64) -> Record {
        Record {
            team_tag: tag(TEAM),
            identifier: "com.example.App.56b41ac3".into(),
            app_name: "Example".into(),
            watch: "sign".into(),
            expires_unix,
            installed_unix: expires_unix - 7 * DAY,
        }
    }

    #[test]
    fn a_part_day_never_rounds_up() {
        let now = at(1_000_000);
        // Six and a half days left is six, not seven: a person planning around the number must
        // never be told they have longer than they do.
        let half_past_six = record(1_000_000 + 6 * DAY + DAY / 2);
        assert_eq!(standing(&half_past_six, now), Standing::Valid { days: 6 });
        assert_eq!(
            standing(&record(1_000_000 + 7 * DAY), now),
            Standing::Valid { days: 7 }
        );
    }

    #[test]
    fn the_last_day_and_the_boundary() {
        let now = at(1_000_000);
        assert_eq!(
            standing(&record(1_000_000 + DAY - 1), now),
            Standing::ExpiresToday
        );
        // Exactly at the expiry second the profile is no longer valid, so it has expired. Calling
        // the boundary "today" would be the record claiming a launch that will not happen.
        assert_eq!(
            standing(&record(1_000_000), now),
            Standing::Expired { days: 0 }
        );
        assert_eq!(
            standing(&record(1_000_000 - 2 * DAY), now),
            Standing::Expired { days: 2 }
        );
        assert_eq!(
            standing(&record(1_000_000 - 40 * DAY), now),
            Standing::LongExpired
        );
    }

    #[test]
    fn the_record_names_no_one() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("renewal.json");
        remember(&path, record(2_000_000)).unwrap();
        let written = std::fs::read_to_string(&path).unwrap();
        // No team identifier, no path, no address, and nothing shaped like either.
        assert!(!written.contains(TEAM));
        assert!(!written.contains('/'));
        assert!(!written.contains('@'));
        assert!(written.contains(&tag(TEAM)));
    }

    #[test]
    fn a_damaged_file_is_silence_not_a_failure() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("renewal.json");
        std::fs::write(&path, "{ not json").unwrap();
        assert!(status(&path, Some(TEAM), None, at(1_000_000)).is_none());
        std::fs::write(&path, vec![b'x'; (MAX_BYTES + 1) as usize]).unwrap();
        assert!(status(&path, Some(TEAM), None, at(1_000_000)).is_none());
        // And a later record still writes over the damage rather than inheriting it.
        remember(&path, record(2_000_000)).unwrap();
        assert!(status(&path, Some(TEAM), None, at(1_000_000)).is_some());
    }

    #[test]
    fn a_record_about_another_build_shows_no_countdown() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("renewal.json");
        remember(&path, record(1_000_000 + 5 * DAY)).unwrap();

        let same = status(
            &path,
            Some(TEAM),
            Some("com.example.App.56b41ac3"),
            at(1_000_000),
        )
        .unwrap();
        assert_eq!(same.bearing, Bearing::SameApp);
        assert!(same.sentence.contains("5 days"));

        let other_app =
            status(&path, Some(TEAM), Some("com.example.Other"), at(1_000_000)).unwrap();
        assert_eq!(other_app.bearing, Bearing::OtherApp);
        assert!(!other_app.sentence.contains("5 days"));

        let other_team = status(
            &path,
            Some("5555U85K3T"),
            Some("com.example.App.56b41ac3"),
            at(1_000_000),
        )
        .unwrap();
        assert_eq!(other_team.bearing, Bearing::OtherTeam);
        assert!(!other_team.sentence.contains("5 days"));
        assert!(!other_team.urgent);
    }

    #[test]
    fn only_the_build_on_screen_becomes_urgent() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("renewal.json");
        remember(&path, record(1_000_000 - DAY)).unwrap();
        let mine = status(
            &path,
            Some(TEAM),
            Some("com.example.App.56b41ac3"),
            at(1_000_000),
        )
        .unwrap();
        assert!(mine.urgent);
        let theirs = status(&path, Some("5555U85K3T"), None, at(1_000_000)).unwrap();
        assert!(!theirs.urgent);
    }

    #[test]
    fn re_installing_replaces_rather_than_accumulates() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("renewal.json");
        remember(&path, record(1_000_000)).unwrap();
        remember(&path, record(1_000_000 + 7 * DAY)).unwrap();
        let ledger = read(&path);
        assert_eq!(ledger.records.len(), 1);
        assert_eq!(ledger.records[0].expires_unix, 1_000_000 + 7 * DAY);
    }

    #[test]
    fn several_testers_do_not_evict_each_other_but_the_file_stays_small() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("renewal.json");
        for n in 0..(MAX_RECORDS + 5) {
            let mut held = record(2_000_000 + n as i64);
            held.identifier = format!("com.example.App.{n}");
            held.installed_unix = n as i64;
            remember(&path, held).unwrap();
        }
        let ledger = read(&path);
        assert_eq!(ledger.records.len(), MAX_RECORDS);
        // The oldest installs are the ones dropped.
        assert_eq!(ledger.records[0].identifier, "com.example.App.5");
    }

    #[test]
    fn forgetting_is_complete_and_repeatable() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("renewal.json");
        remember(&path, record(2_000_000)).unwrap();
        forget(&path).unwrap();
        forget(&path).unwrap();
        assert!(status(&path, Some(TEAM), None, at(1_000_000)).is_none());
    }
}
