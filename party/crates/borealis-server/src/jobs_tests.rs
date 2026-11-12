//! Background jobs (jobs.rs): start, status, done and failed over HTTP, one job per account, accounts kept apart, a
//! busy workspace kept open, a restart during a job, and a job that outlives its client.

use super::*;

use crate::jobs::{JobState, JOB_DONE, RESTARTED};
use crate::workspaces::Workspace;

fn content() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../content")
}

/// Ask about a job until it ends.
async fn wait_ended(client: &Client, id: &str) -> Value {
    for _ in 0..2400 {
        let (status, out) = client.rpc("job_status", json!({ "jobId": id })).await;
        assert_eq!(status, StatusCode::OK, "{out}");
        if out["state"] != "running" {
            return out;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("job {id} never ended");
}

fn record(dir: &Path, account: &str, id: &str) -> Value {
    let path = dir
        .join("users")
        .join(account)
        .join("sync")
        .join("jobs")
        .join(format!("{id}.json"));
    serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap()
}

/// What the jobs answer over HTTP, on any database.
async fn jobs_over_http(router: Router, data: &Path) {
    let alice = sign_up(&router, "alice").await;
    let bob = sign_up(&router, "bob").await;

    // Done: the same result as the plain call.
    let (status, plain) = alice.rpc("update_indexes", json!({})).await;
    assert_eq!(status, StatusCode::OK, "{plain}");
    let (status, started) = alice
        .rpc(
            "job_start",
            json!({ "command": "update_indexes", "args": {} }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{started}");
    assert_eq!(started["command"], "update_indexes");
    let id = started["jobId"].as_str().unwrap().to_string();
    assert_eq!(id.len(), 32, "{id}");
    let ended = wait_ended(&alice, &id).await;
    assert_eq!(ended["state"], "done", "{ended}");
    assert_eq!(ended["result"], plain);
    assert_eq!(ended["jobId"], id.as_str());
    assert_eq!(record(data, &alice.id, &id)["state"], "done");

    // Another account can't read it: the same answer as an unknown id.
    let (status, out) = bob.rpc("job_status", json!({ "jobId": id })).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{out}");
    let unknown = "0".repeat(32);
    let (status, other) = bob.rpc("job_status", json!({ "jobId": unknown })).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(out, other);
    // Nor a path.
    let (status, _) = alice
        .rpc("job_status", json!({ "jobId": format!("../{}", &id[3..]) }))
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // Failed: the address guard still refuses, and the job's error is the plain call's.
    let args = json!({ "url": "https://127.0.0.1/5etools" });
    let (status, refused) = alice.rpc("stage_5etools_url", args.clone()).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{refused}");
    let (_, started) = alice
        .rpc(
            "job_start",
            json!({ "command": "stage_5etools_url", "args": args }),
        )
        .await;
    let ended = wait_ended(&alice, started["jobId"].as_str().unwrap()).await;
    assert_eq!(ended["state"], "failed", "{ended}");
    assert_eq!(ended["error"], refused);
    assert!(ended.get("result").is_none());

    // Only the long downloads run as jobs.
    for command in ["delete_character", "job_start", "list_indexes"] {
        let (status, out) = alice.rpc("job_start", json!({ "command": command })).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{command}: {out}");
    }
    let (status, _) = alice
        .rpc(
            "job_start",
            json!({ "command": "update_indexes", "args": [1] }),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    // Signed in only.
    let (status, _) = send(
        &router,
        None,
        "POST",
        "/api/rpc/job_start",
        json!({ "command": "update_indexes" }),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test(flavor = "multi_thread")]
async fn jobs_run_over_http() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("data");
    jobs_over_http(router(data.clone()).await, &data).await;
}

/// The same on PostgreSQL, in a database of its own (the other PostgreSQL test wipes the one it is given).
#[tokio::test(flavor = "multi_thread")]
async fn jobs_run_over_http_on_postgres() {
    let Ok(url) = std::env::var("BOREALIS_TEST_POSTGRES_URL") else {
        eprintln!("skipped: set BOREALIS_TEST_POSTGRES_URL to run against PostgreSQL");
        return;
    };
    sqlx::any::install_default_drivers();
    let admin = sqlx::AnyPool::connect(&url).await.unwrap();
    let name = format!("bp_jobs_{}", uuid::Uuid::new_v4().simple());
    sqlx::query(&format!("CREATE DATABASE {name}"))
        .execute(&admin)
        .await
        .unwrap();
    let mut own = url::Url::parse(&url).unwrap();
    own.set_path(&format!("/{name}"));
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("data");
    let db = Db::open(&Location::Postgres(own.to_string()))
        .await
        .unwrap();
    jobs_over_http(router_on(data.clone(), db), &data).await;
    sqlx::query(&format!("DROP DATABASE {name} WITH (FORCE)"))
        .execute(&admin)
        .await
        .unwrap();
}

/// A job that waits for `go`, then answers `{ "ok": true }`.
fn gated(
    workspace: &Arc<Workspace>,
) -> (
    tokio::sync::oneshot::Sender<()>,
    impl std::future::Future<Output = crate::error::ApiResult<Value>> + Send + 'static,
) {
    let (go, wait) = tokio::sync::oneshot::channel::<()>();
    let on = Arc::clone(workspace);
    let work = async move {
        on.emit(
            "borealis://content-progress",
            json!({ "phase": "downloading", "done": 1, "total": 3 }),
        );
        let _ = wait.await;
        Ok(json!({ "ok": true }))
    };
    (go, work)
}

async fn wait_state(workspaces: &Workspaces, account: &str, id: &str, state: JobState) {
    for _ in 0..400 {
        if workspaces.jobs().status(account, id).unwrap().state == state {
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("job {id} never became {state:?}");
}

/// One job per account; a busy workspace is never evicted, so no second backend opens on its folder; the job runs to
/// its end with nobody holding the workspace, and tells the account's clients it ended.
#[tokio::test(flavor = "multi_thread")]
async fn one_job_per_account_keeps_its_workspace_open() {
    let dir = tempfile::tempdir().unwrap();
    let workspaces = Workspaces::new(dir.path().join("data"), content());
    let account = uuid::Uuid::new_v4().to_string();
    let other = uuid::Uuid::new_v4().to_string();
    let workspace = workspaces.get(&account).await.unwrap();
    let (go, work) = gated(&workspace);
    let (first, started) = workspaces
        .jobs()
        .start(&account, &workspace, "update_indexes", json!({}), work)
        .unwrap();
    assert!(started);
    assert_eq!(first.state, JobState::Running);

    // A second start answers the running job, whatever it asks for, and never runs.
    let (second, started) = workspaces
        .jobs()
        .start(
            &account,
            &workspace,
            "update_5etools_url",
            json!({ "url": "https://example.com/5etools" }),
            async { panic!("a second job ran") },
        )
        .unwrap();
    assert!(!started);
    assert_eq!(second.job_id, first.job_id);
    assert_eq!(second.command, "update_indexes");
    // With what it was asked, so `job_start` can tell the caller whether it is the same call.
    assert_eq!(second.args, json!({}));

    // Another account has its own, and can't see this one.
    let theirs = workspaces.get(&other).await.unwrap();
    let (own, started) = workspaces
        .jobs()
        .start(&other, &theirs, "update_indexes", json!({}), async {
            Ok(Value::Null)
        })
        .unwrap();
    assert!(started);
    assert_ne!(own.job_id, first.job_id);
    assert!(workspaces.jobs().status(&other, &first.job_id).is_err());
    assert!(workspaces.jobs().status(&account, &own.job_id).is_err());

    // Progress follows the command's events.
    for _ in 0..400 {
        if workspaces
            .jobs()
            .status(&account, &first.job_id)
            .unwrap()
            .progress
            .is_some()
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    let running = workspaces.jobs().status(&account, &first.job_id).unwrap();
    assert_eq!(running.progress.unwrap()["done"], 1);

    // The client is gone, and the workspace has been idle for long: it stays open while the job runs.
    let kept = Arc::downgrade(&workspace);
    drop(workspace);
    drop(theirs);
    workspaces.age(&account);
    workspaces.evict_idle();
    assert!(workspaces.loaded_now(&account).is_some());
    let again = workspaces.get(&account).await.unwrap();
    assert!(Arc::ptr_eq(&again, &kept.upgrade().unwrap()));
    let mut events = again.events.subscribe();
    drop(again);

    go.send(()).unwrap();
    wait_state(&workspaces, &account, &first.job_id, JobState::Done).await;
    let done = workspaces.jobs().status(&account, &first.job_id).unwrap();
    assert_eq!(done.result, Some(json!({ "ok": true })));
    assert!(done.progress.is_none());
    let event = loop {
        let event = tokio::time::timeout(Duration::from_secs(10), events.recv())
            .await
            .unwrap()
            .unwrap();
        if event.name == JOB_DONE {
            break event;
        }
    };
    assert_eq!(event.payload["jobId"], first.job_id.as_str());
    assert_eq!(event.payload["state"], "done");

    // Ended, it no longer holds the workspace open, and the account may start another.
    drop(events);
    workspaces.age(&account);
    workspaces.evict_idle();
    assert!(workspaces.loaded_now(&account).is_none());
    let workspace = workspaces.get(&account).await.unwrap();
    let (next, started) = workspaces
        .jobs()
        .start(&account, &workspace, "update_indexes", json!({}), async {
            Ok(Value::Null)
        })
        .unwrap();
    assert!(started);
    assert_ne!(next.job_id, first.job_id);
}

/// A command that panics fails its job instead of leaving it running.
#[tokio::test(flavor = "multi_thread")]
async fn a_panicking_job_fails() {
    let dir = tempfile::tempdir().unwrap();
    let workspaces = Workspaces::new(dir.path().join("data"), content());
    let account = uuid::Uuid::new_v4().to_string();
    let workspace = workspaces.get(&account).await.unwrap();
    let (job, _) = workspaces
        .jobs()
        .start(&account, &workspace, "update_indexes", json!({}), async {
            panic!("the command broke")
        })
        .unwrap();
    wait_state(&workspaces, &account, &job.job_id, JobState::Failed).await;
    let failed = workspaces.jobs().status(&account, &job.job_id).unwrap();
    assert_eq!(failed.error.unwrap()["code"], "internal");
    assert!(!workspace.is_busy());
}

/// A job the server stopped during is reported failed after the restart, and its record says so.
#[tokio::test(flavor = "multi_thread")]
async fn a_restart_fails_unfinished_jobs() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("data");
    let file = dir.path().join("party.db");
    let alice = sign_up(
        &router_on(
            data.clone(),
            Db::open(&Location::Sqlite(file.clone())).await.unwrap(),
        ),
        "alice",
    )
    .await;
    let jobs = data.join("users").join(&alice.id).join("sync").join("jobs");
    std::fs::create_dir_all(&jobs).unwrap();
    let id = uuid::Uuid::new_v4().simple().to_string();
    std::fs::write(
        jobs.join(format!("{id}.json")),
        json!({ "jobId": id, "command": "update_indexes", "state": "running", "startedAt": 1,
                "progress": { "done": 2 } })
        .to_string(),
    )
    .unwrap();

    let router = router_on(
        data.clone(),
        Db::open(&Location::Sqlite(file)).await.unwrap(),
    );
    let alice = Client { router, ..alice };
    let (status, out) = alice.rpc("job_status", json!({ "jobId": id })).await;
    assert_eq!(status, StatusCode::OK, "{out}");
    assert_eq!(out["state"], "failed", "{out}");
    assert_eq!(out["error"]["message"], RESTARTED);
    assert!(out.get("progress").is_none());
    let kept = record(&data, &alice.id, &id);
    assert_eq!(kept["state"], "failed");
    assert!(kept["finishedAt"].is_i64());
    // The account isn't held up by it.
    let (status, started) = alice
        .rpc("job_start", json!({ "command": "update_indexes" }))
        .await;
    assert_eq!(status, StatusCode::OK, "{started}");
    assert_eq!(started["started"], true);
    assert_eq!(started["same"], true);
    assert_ne!(started["jobId"], id.as_str());
    wait_ended(&alice, started["jobId"].as_str().unwrap()).await;
}
