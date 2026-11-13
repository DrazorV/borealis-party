//! The automatic backup before an upgrade (`backup.rs`).

use std::path::Path;
use std::sync::{Arc, Mutex};

use super::*;
use crate::auth::Status;
use crate::backup::{self, Decision};
use crate::config::{BackupSettings, OnUpgrade};
use crate::db::NewAccount;

const VERSION: &str = env!("CARGO_PKG_VERSION");

async fn add_user(db: &Db, name: &str) {
    db.insert_user(&NewAccount {
        id: &uuid::Uuid::new_v4().to_string(),
        username: name,
        email: None,
        password_hash: "x",
        status: Status::Active,
        admin: false,
        verify_token_hash: None,
        verify_expires_at: None,
    })
    .await
    .unwrap();
}

fn settings(dir: &Path) -> BackupSettings {
    BackupSettings {
        dir: dir.join("backups"),
        postgres_bin: std::env::var_os("BOREALIS_TEST_PG_BIN").map(PathBuf::from),
        ..BackupSettings::default()
    }
}

/// Files in the backup folder, by name.
/// The files in `dir`, without SQLite's `-wal` / `-shm` beside a copy the test itself opened (they can still be
/// there for a moment after its pool closes).
fn listing(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .map(|it| {
            it.flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .filter(|n| !n.ends_with("-wal") && !n.ends_with("-shm"))
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names
}

/// A database one step behind, as the previous release left it.
async fn one_behind(location: &Location) -> Db {
    let db = Db::open_at(location, Db::schema_version() - 1)
        .await
        .unwrap();
    add_user(&db, "kept").await;
    db
}

#[test]
fn a_new_version_or_a_waiting_step_asks_for_a_backup() {
    assert_eq!(
        backup::decide(Some("0.3.0"), "0.3.0", Some(&[])),
        Decision::Nothing
    );
    assert!(matches!(
        backup::decide(Some("0.3.0"), "0.3.1", Some(&[])),
        Decision::Backup { reason } if reason == "version 0.3.0 to 0.3.1"
    ));
    assert!(matches!(
        backup::decide(Some("0.3.1"), "0.3.1", Some(&[7, 8])),
        Decision::Backup { reason } if reason == "database steps 7, 8 waiting"
    ));
    // No record yet (the first release with backups, or a new config folder) with an existing database.
    assert!(matches!(
        backup::decide(None, "0.3.1", Some(&[])),
        Decision::Backup { .. }
    ));
    // A database without tables has nothing to save.
    assert_eq!(backup::decide(None, "0.3.1", None), Decision::Nothing);
    assert_eq!(
        backup::decide(Some("0.2.0"), "0.3.1", None),
        Decision::Nothing
    );
}

#[test]
fn stamps_and_names_are_utc_and_sortable() {
    assert_eq!(backup::stamp(0), "19700101T000000Z");
    assert_eq!(backup::stamp(951_782_400), "20000229T000000Z");
    assert_eq!(backup::stamp(1_700_000_000), "20231114T221320Z");
    assert_eq!(
        backup::file_name(Some("0.3.0"), "0.3.1", 1_700_000_000, true),
        "borealis_party-0.3.0-to-0.3.1-20231114T221320Z.dump"
    );
    assert_eq!(
        backup::file_name(None, "0.3.1", 0, false),
        "borealis_party-unknown-to-0.3.1-19700101T000000Z.sqlite"
    );
    assert_eq!(
        backup::file_name(Some("../x y"), "0.3.1", 0, false),
        "borealis_party-.._x_y-to-0.3.1-19700101T000000Z.sqlite"
    );
}

#[test]
fn pruning_keeps_the_newest_automatic_backups_only() {
    let dir = tempfile::tempdir().unwrap();
    let mut made = Vec::new();
    for i in 0..13u64 {
        let name = backup::file_name(Some("0.3.0"), "0.3.1", 1_700_000_000 + i * 3600, i % 2 == 0);
        std::fs::write(dir.path().join(&name), b"x").unwrap();
        made.push(name);
    }
    let others = [
        "notes.txt",
        "borealis_party-by-hand.dump",
        "borealis_party-0.1-to-0.2-yesterday.dump",
        "manual-0.3.0-to-0.3.1-20000101T000000Z.dump",
    ];
    for name in others {
        std::fs::write(dir.path().join(name), b"mine").unwrap();
    }
    std::fs::create_dir(
        dir.path()
            .join("borealis_party-0.1-to-0.2-20000101T000000Z.dump"),
    )
    .unwrap();
    let removed = backup::prune(dir.path(), 10).unwrap();
    assert_eq!(removed.len(), 3);
    let left = listing(dir.path());
    for old in &made[..3] {
        assert!(!left.contains(old), "{old} should be gone");
    }
    for kept in &made[3..] {
        assert!(left.contains(kept), "{kept} should stay");
    }
    for name in others {
        assert!(left.contains(&name.to_string()), "{name} is not ours");
    }
    assert!(left.contains(&"borealis_party-0.1-to-0.2-20000101T000000Z.dump".to_string()));
    // 0 keeps every one.
    assert!(backup::prune(dir.path(), 0).unwrap().is_empty());
    assert_eq!(backup::prune(dir.path(), 10).unwrap().len(), 0);
}

#[test]
fn the_password_is_never_an_argument() {
    let out = Path::new("/config/backups/x.dump.partial");
    let cmd = backup::pg_dump_command(
        &BackupSettings::default(),
        "postgres://borealis:hunter2%40secret@db.example:5433/borealis_party?sslmode=require",
        out,
    )
    .unwrap();
    let std = cmd.as_std();
    let args: Vec<String> = std
        .get_args()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    assert_eq!(
        args,
        [
            "--format=custom",
            "--no-password",
            "--file",
            &out.to_string_lossy()
        ]
    );
    for arg in &args {
        assert!(!arg.contains("hunter2"), "{arg}");
    }
    let envs: std::collections::HashMap<String, String> = std
        .get_envs()
        .filter_map(|(k, v)| {
            Some((
                k.to_string_lossy().into_owned(),
                v?.to_string_lossy().into_owned(),
            ))
        })
        .collect();
    assert_eq!(envs["PGPASSWORD"], "hunter2@secret");
    assert_eq!(envs["PGHOST"], "db.example");
    assert_eq!(envs["PGPORT"], "5433");
    assert_eq!(envs["PGUSER"], "borealis");
    assert_eq!(envs["PGDATABASE"], "borealis_party");
    assert_eq!(envs["PGSSLMODE"], "require");
}

/// Log lines, for checking that no secret is written.
#[derive(Clone, Default)]
struct Captured(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for Captured {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// A failing `pg_dump` (no server listens there) logs and reports no password, and nothing is migrated. With
/// `BOREALIS_TEST_PG_BIN` it runs the real program; otherwise the one on the PATH, or none at all.
#[tokio::test(flavor = "current_thread")]
async fn a_failed_postgres_backup_tells_no_password() {
    let captured = Captured::default();
    let sink = captured.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(move || sink.clone())
        .with_max_level(tracing::Level::TRACE)
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);
    let dir = tempfile::tempdir().unwrap();
    let url = "postgres://borealis:hunter2-secret@127.0.0.1:9/borealis_party?sslmode=disable";
    // The database itself is SQLite here: only the dump's connection is Postgres, and it fails.
    let db = one_behind(&Location::Memory).await;
    backup::record_started(dir.path(), VERSION).unwrap();
    let err = backup::upgrade_with_backup(
        &db,
        &Location::Postgres(url.into()),
        &settings(dir.path()),
        dir.path(),
        VERSION,
    )
    .await
    .unwrap_err();
    assert!(err.contains("BOREALIS__BACKUP__ON_UPGRADE=skip"), "{err}");
    assert!(!err.contains("hunter2"), "{err}");
    let logs = String::from_utf8(captured.0.lock().unwrap().clone()).unwrap();
    assert!(logs.contains("backing up the database"), "{logs}");
    assert!(!logs.contains("hunter2"), "{logs}");
    assert_eq!(
        db.pending_steps().await.unwrap(),
        Some(vec![i64::try_from(Db::schema_version()).unwrap()])
    );
    assert!(listing(&dir.path().join("backups")).is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_waiting_step_is_backed_up_first_on_sqlite() {
    let dir = tempfile::tempdir().unwrap();
    let location = Location::Sqlite(dir.path().join("party.db"));
    let db = one_behind(&location).await;
    // Same version: only the waiting step asks for the backup.
    backup::record_started(dir.path(), VERSION).unwrap();
    let written =
        backup::upgrade_with_backup(&db, &location, &settings(dir.path()), dir.path(), VERSION)
            .await
            .unwrap()
            .expect("a backup");
    let name = written
        .path
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    assert!(
        name.starts_with(&format!("borealis_party-{VERSION}-to-{VERSION}-"))
            && name.ends_with(".sqlite"),
        "{name}"
    );
    assert!(written.bytes > 0);
    assert_eq!(
        listing(&dir.path().join("backups")),
        std::slice::from_ref(&name)
    );
    // The copy is the database as it was: the user, and the last step still to run.
    let copy = Db::connect(&Location::Sqlite(written.path.clone()))
        .await
        .unwrap();
    assert_eq!(copy.user_count().await.unwrap(), 1);
    assert_eq!(
        copy.pending_steps().await.unwrap(),
        Some(vec![i64::try_from(Db::schema_version()).unwrap()])
    );
    copy.pool().close().await;
    assert_eq!(db.pending_steps().await.unwrap(), Some(vec![]));
    assert_eq!(
        backup::previous_version(dir.path()).as_deref(),
        Some(VERSION)
    );
    // Started again: nothing new, no second backup.
    assert!(backup::upgrade_with_backup(
        &db,
        &location,
        &settings(dir.path()),
        dir.path(),
        VERSION
    )
    .await
    .unwrap()
    .is_none());
    assert_eq!(listing(&dir.path().join("backups")), [name]);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_new_version_is_backed_up_once_and_a_new_database_not_at_all() {
    let dir = tempfile::tempdir().unwrap();
    let location = Location::Sqlite(dir.path().join("party.db"));
    // A brand-new database: nothing to back up.
    let db = Db::connect(&location).await.unwrap();
    assert!(backup::upgrade_with_backup(
        &db,
        &location,
        &settings(dir.path()),
        dir.path(),
        "0.3.0"
    )
    .await
    .unwrap()
    .is_none());
    assert!(!dir.path().join("backups").exists());
    add_user(&db, "kept").await;
    // The next version, with no step waiting.
    let written =
        backup::upgrade_with_backup(&db, &location, &settings(dir.path()), dir.path(), "0.3.1")
            .await
            .unwrap()
            .expect("a backup");
    assert!(written
        .path
        .to_string_lossy()
        .contains("borealis_party-0.3.0-to-0.3.1-"));
    assert!(backup::upgrade_with_backup(
        &db,
        &location,
        &settings(dir.path()),
        dir.path(),
        "0.3.1"
    )
    .await
    .unwrap()
    .is_none());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_failed_backup_leaves_the_database_alone() {
    let dir = tempfile::tempdir().unwrap();
    let location = Location::Sqlite(dir.path().join("party.db"));
    let db = one_behind(&location).await;
    backup::record_started(dir.path(), "0.3.0").unwrap();
    // The backup folder can't be made: a file is in the way.
    std::fs::write(dir.path().join("backups"), b"in the way").unwrap();
    let err =
        backup::upgrade_with_backup(&db, &location, &settings(dir.path()), dir.path(), "0.3.1")
            .await
            .unwrap_err();
    assert!(err.contains("the database was not changed"), "{err}");
    let waiting = Some(vec![i64::try_from(Db::schema_version()).unwrap()]);
    assert_eq!(db.pending_steps().await.unwrap(), waiting);
    assert_eq!(
        backup::previous_version(dir.path()).as_deref(),
        Some("0.3.0")
    );
    // The emergency switch starts without one, and says so.
    let skip = BackupSettings {
        on_upgrade: OnUpgrade::Skip,
        ..settings(dir.path())
    };
    assert!(
        backup::upgrade_with_backup(&db, &location, &skip, dir.path(), "0.3.1")
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(db.pending_steps().await.unwrap(), Some(vec![]));
    assert_eq!(
        backup::previous_version(dir.path()).as_deref(),
        Some("0.3.1")
    );
}

#[test]
fn the_skip_switch_comes_from_the_environment() {
    let s = crate::config::Settings::parse(
        crate::config::DEFAULT_FILE,
        [
            (
                "BOREALIS__BACKUP__ON_UPGRADE".to_string(),
                "skip".to_string(),
            ),
            ("BOREALIS__BACKUP__KEEP".to_string(), "3".to_string()),
            (
                "BOREALIS__BACKUP__DIR".to_string(),
                "/srv/dumps".to_string(),
            ),
        ],
    )
    .unwrap();
    assert_eq!(s.backup.on_upgrade, OnUpgrade::Skip);
    assert_eq!(s.backup.keep, 3);
    assert_eq!(s.backup.dir, PathBuf::from("/srv/dumps"));
    let d = crate::config::Settings::parse(crate::config::DEFAULT_FILE, []).unwrap();
    assert_eq!(d.backup, BackupSettings::default());
    assert_eq!(
        crate::config::Settings::resolve(Path::new("/config"), &d.backup.dir),
        Path::new("/config").join("backups")
    );
}

/// On PostgreSQL. Runs when `BOREALIS_TEST_POSTGRES_URL` names a database it may wipe; the backup needs a `pg_dump`
/// at least as new as the server (`BOREALIS_TEST_PG_BIN`, else the PATH). With an older one the backup fails, and
/// the test then checks that nothing was migrated.
#[tokio::test(flavor = "multi_thread")]
async fn a_waiting_step_is_backed_up_first_on_postgres() {
    let Ok(url) = std::env::var("BOREALIS_TEST_POSTGRES_URL") else {
        eprintln!("skipped: set BOREALIS_TEST_POSTGRES_URL to run against PostgreSQL");
        return;
    };
    let _one = POSTGRES.lock().await;
    drop_all(&url).await;
    let dir = tempfile::tempdir().unwrap();
    let location = Location::Postgres(url.clone());
    let db = one_behind(&location).await;
    backup::record_started(dir.path(), VERSION).unwrap();
    let last = i64::try_from(Db::schema_version()).unwrap();
    match backup::upgrade_with_backup(&db, &location, &settings(dir.path()), dir.path(), VERSION)
        .await
    {
        Ok(written) => {
            let written = written.expect("a backup");
            let name = written
                .path
                .file_name()
                .unwrap()
                .to_string_lossy()
                .into_owned();
            assert!(name.ends_with(".dump"), "{name}");
            assert!(written.bytes > 0);
            assert_eq!(listing(&dir.path().join("backups")), [name]);
            assert_eq!(db.pending_steps().await.unwrap(), Some(vec![]));
            eprintln!("backed up: {}", written.path.display());
        }
        Err(e) => {
            assert!(
                std::env::var_os("BOREALIS_TEST_PG_EXPECT_FAIL").is_some(),
                "the backup failed: {e}"
            );
            eprintln!("the backup failed as expected: {e}");
            assert_eq!(db.pending_steps().await.unwrap(), Some(vec![last]));
            assert!(listing(&dir.path().join("backups")).is_empty());
        }
    }
    db.pool().close().await;
    drop_all(&url).await;
}
