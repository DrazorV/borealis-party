//! "Update all content" and the automatic content update (content_update.rs): the summary, the 24 h throttle, one
//! run over index files and 5etools sources where one failing part leaves the others, the job over HTTP, and the
//! automatic start when due (and not when not).

use super::*;

use borealis_lib::content_fivetools_remote::{FetchRequest, FetchResponse, RemoteFetcher};
use borealis_lib::content_prefs::FiveToolsUrlRecord;
use borealis_lib::state::AppState as Borealis;
use tauri::Manager;

use crate::content_update::{self, due, summary, AUTO_EVERY, STAMP};

const INDEX: &str = "https://content.example/good.index";
const FILE: &str = "https://content.example/party.xml";
/// A 5etools source that answers nothing.
const GONE: &str = "https://data.example.com/5etools/data";

fn content() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../content")
}

#[test]
fn the_summary_says_what_was_checked_and_what_changed() {
    assert_eq!(
        summary(3, 1, 2, 0),
        "3 index files and 1 5etools source checked · 2 updated"
    );
    assert_eq!(
        summary(1, 2, 0, 1),
        "1 index file and 2 5etools sources checked · 0 updated · 1 could not be updated"
    );
    assert!(summary(0, 0, 0, 0).starts_with("Nothing to update"));
}

#[test]
fn the_automatic_check_runs_at_most_once_a_day_and_only_when_on() {
    let now = 1_800_000_000;
    assert!(due(true, None, now), "never checked");
    assert!(!due(true, Some(now - 60), now), "a minute ago");
    assert!(
        !due(true, Some(now - AUTO_EVERY + 1), now),
        "just under a day"
    );
    assert!(due(true, Some(now - AUTO_EVERY), now), "a day ago");
    assert!(due(true, Some(now + 3600), now), "a clock that went back");
    assert!(!due(false, None, now), "switched off");
    assert!(
        !due(false, Some(now - 10 * AUTO_EVERY), now),
        "switched off"
    );
}

/// Index files from `files`; every 5etools request fails.
struct Mock {
    files: std::collections::HashMap<String, Vec<u8>>,
}

impl borealis_lib::content_index::Fetcher for Mock {
    fn get(&self, url: &str, _max: usize) -> Result<Vec<u8>, String> {
        self.files
            .get(url)
            .cloned()
            .ok_or_else(|| "the server answered 404 Not Found".to_string())
    }
}

impl RemoteFetcher for Mock {
    fn fetch(&self, _request: &FetchRequest<'_>) -> Result<FetchResponse, String> {
        Err("the server did not answer".into())
    }
}

fn index_xml() -> Vec<u8> {
    format!(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<index>\n  <info><name>Good</name></info>\n  <files>\n    \
         <file name=\"party.xml\" url=\"{FILE}\" />\n  </files>\n</index>"
    )
    .into_bytes()
}

/// A real element file (the spell pack the other index tests use).
fn pack_xml() -> Vec<u8> {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD
        .decode(spell_pack()["request"]["contentBase64"].as_str().unwrap())
        .unwrap()
}

/// An index file and a remembered 5etools source, as the account's own.
fn add_sources(state: &Borealis) {
    state.add_index(INDEX).unwrap();
    let mut prefs = state.content_prefs();
    prefs.fivetools_urls.push(FiveToolsUrlRecord {
        url: GONE.into(),
        revision: "abc1234".into(),
        sources: vec!["MM".into()],
        ..FiveToolsUrlRecord::default()
    });
    prefs.save(&state.paths.prefs_file()).unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn one_run_updates_the_index_files_and_every_5etools_source() {
    let dir = tempfile::tempdir().unwrap();
    let workspaces = Workspaces::new(dir.path().join("data"), content());
    let account = uuid::Uuid::new_v4().to_string();
    let workspace = workspaces.get(&account).await.unwrap();
    let (result, status, stamp) = workspace
        .blocking(|app| {
            let state = app.state::<Borealis>();
            add_sources(&state);
            let mock = crate::net_guard::Guarded::new(Mock {
                files: std::collections::HashMap::from([
                    (INDEX.to_string(), index_xml()),
                    (FILE.to_string(), pack_xml()),
                ]),
            });
            let result = content_update::run_with(
                &state,
                &mock,
                &borealis_lib::content_packs::NullSink,
                false,
            )
            .unwrap();
            let status = content_update::status(&state);
            let stamp = content_update::last_check(&state.paths.data);
            Ok((result, status, stamp))
        })
        .await
        .unwrap();
    assert_eq!(result.indexes, 1);
    assert_eq!(result.sources, 1);
    assert!(result.updated >= 1, "{result:?}");
    // The failing source is reported; the index files were still updated.
    assert_eq!(result.failed.len(), 1, "{result:?}");
    assert!(result.failed[0].starts_with(GONE), "{result:?}");
    assert!(
        result
            .message
            .starts_with("1 index file and 1 5etools source checked · "),
        "{}",
        result.message
    );
    assert!(result.message.ends_with("· 1 could not be updated"));
    assert!(!result.auto);
    // Kept for the 24 hours, and the status bar has nothing to say about a run the user started.
    assert!(stamp.is_some());
    assert!(dir
        .path()
        .join("data/users")
        .join(&account)
        .join(STAMP)
        .exists());
    assert_eq!(
        status.state,
        borealis_lib::settings_dto::ContentUpdateState::Done
    );
    assert_eq!(status.updated, 0);
    assert!(status.checked_at.is_some());
}

#[tokio::test(flavor = "multi_thread")]
async fn update_all_runs_as_a_job_over_http_and_says_when_there_is_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("data");
    let router = router(data.clone()).await;
    let alice = sign_up(&router, "alice").await;
    let (status, info) = alice.rpc("content_update_info", json!({})).await;
    assert_eq!(status, StatusCode::OK, "{info}");
    assert_eq!(info["auto"], true, "on by default: {info}");
    assert_eq!(info["everyHours"], 24);
    assert!(info["last"].is_null());

    let (status, started) = alice
        .rpc(
            "job_start",
            json!({ "command": "update_all_content", "args": {} }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{started}");
    let id = started["jobId"].as_str().unwrap().to_string();
    let mut ended = Value::Null;
    for _ in 0..600 {
        let (_, out) = alice.rpc("job_status", json!({ "jobId": id })).await;
        if out["state"] != "running" {
            ended = out;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(ended["state"], "done", "{ended}");
    assert_eq!(ended["result"]["indexes"], 0);
    assert_eq!(ended["result"]["sources"], 0);
    assert!(ended["result"]["message"]
        .as_str()
        .unwrap()
        .starts_with("Nothing to update"));
    let (_, info) = alice.rpc("content_update_info", json!({})).await;
    assert_eq!(info["last"]["indexes"], 0, "{info}");
    let (status, bar) = alice.rpc("content_update_status", json!({})).await;
    assert_eq!(status, StatusCode::OK, "{bar}");
    assert_eq!(bar["state"], "done", "{bar}");
}

async fn wait_job(workspaces: &Workspaces, account: &str) {
    for _ in 0..600 {
        let dir = workspaces.dir(account).join("sync").join("jobs");
        let ended = std::fs::read_dir(&dir).ok().is_some_and(|entries| {
            entries.flatten().any(|e| {
                std::fs::read(e.path())
                    .ok()
                    .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
                    .is_some_and(|r| r["state"] == "done" || r["state"] == "failed")
            })
        });
        if ended {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("the automatic job never ended");
}

#[tokio::test(flavor = "multi_thread")]
async fn the_automatic_check_starts_when_due_and_then_waits_a_day() {
    let dir = tempfile::tempdir().unwrap();
    let workspaces = Workspaces::new(dir.path().join("data"), content());
    let hub = Arc::new(crate::hub::PartyHub::new(dir.path().join("cards")));
    let account = uuid::Uuid::new_v4().to_string();
    let workspace = workspaces.get(&account).await.unwrap();
    let jobs = Arc::clone(workspaces.jobs());

    // Nothing to check: no job.
    let started = content_update::start_if_due(&jobs, &hub, &account, &workspace, Duration::ZERO)
        .await
        .unwrap();
    assert!(!started, "an account with no sources");

    // A source the address check refuses: the job runs and fails that one without any download.
    workspace
        .blocking(|app| {
            let state = app.state::<Borealis>();
            state.add_index("https://127.0.0.1/lan.index").unwrap();
            Ok(())
        })
        .await
        .unwrap();
    let started = content_update::start_if_due(&jobs, &hub, &account, &workspace, Duration::ZERO)
        .await
        .unwrap();
    assert!(started, "due: never checked");
    wait_job(&workspaces, &account).await;
    let last = content_update::last_check(&workspaces.dir(&account)).expect("a stamp");

    // Within the day: not again.
    let started = content_update::start_if_due(&jobs, &hub, &account, &workspace, Duration::ZERO)
        .await
        .unwrap();
    assert!(!started, "checked {last}, under a day ago");

    // A day later (the stamp made older): again, and the status bar is told about it.
    let stamp = workspaces.dir(&account).join(STAMP);
    let mut kept: Value = serde_json::from_slice(&std::fs::read(&stamp).unwrap()).unwrap();
    kept["at"] = json!(last - AUTO_EVERY - 1);
    std::fs::write(&stamp, serde_json::to_vec(&kept).unwrap()).unwrap();
    for f in std::fs::read_dir(workspaces.dir(&account).join("sync/jobs")).unwrap() {
        std::fs::remove_file(f.unwrap().path()).unwrap();
    }
    let started = content_update::start_if_due(&jobs, &hub, &account, &workspace, Duration::ZERO)
        .await
        .unwrap();
    assert!(started, "a day later");
    wait_job(&workspaces, &account).await;
    let bar = workspace
        .blocking(|app| Ok(content_update::status(&app.state::<Borealis>())))
        .await
        .unwrap();
    assert_eq!(
        bar.state,
        borealis_lib::settings_dto::ContentUpdateState::Done
    );
    assert!(
        bar.message
            .starts_with("1 index file and 0 5etools sources checked"),
        "{}",
        bar.message
    );
    assert_eq!(bar.failed.len(), 1, "{:?}", bar.failed);

    // Switched off in the account's settings: never.
    workspace
        .blocking(|app| {
            let state = app.state::<Borealis>();
            state
                .save_settings(&json!({ "autoContentUpdate": false }))
                .unwrap();
            Ok(())
        })
        .await
        .unwrap();
    std::fs::write(&stamp, serde_json::to_vec(&kept).unwrap()).unwrap();
    let started = content_update::start_if_due(&jobs, &hub, &account, &workspace, Duration::ZERO)
        .await
        .unwrap();
    assert!(!started, "switched off");
}
