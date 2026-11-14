//! Steps 8 and 10 are empty since 0.3.5 (unused since 0.3.3: step 8 made the in-app assistant's tables, step 10
//! dropped them, and every database has run both). Their numbers stay, so a fresh database still reaches the same
//! schema version with every step recorded, and has no `ai_*` table; a database a 0.3.3 server left at step 10 starts
//! with nothing pending, keeps its accounts, and is not changed. On SQLite, and on PostgreSQL when
//! `BOREALIS_TEST_POSTGRES_URL` names a database it may wipe.

use super::*;

use sqlx::Row;

async fn versions(db: &Db) -> Vec<i64> {
    sqlx::query("SELECT version FROM schema_version ORDER BY version")
        .fetch_all(db.pool())
        .await
        .unwrap()
        .iter()
        .map(|r| r.try_get::<i64, _>(0).unwrap())
        .collect()
}

async fn tables(db: &Db) -> Vec<String> {
    let sql = if matches!(db.kind(), "sqlite") {
        "SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name"
    } else {
        "SELECT table_name::text FROM information_schema.tables WHERE table_schema = current_schema() ORDER BY 1"
    };
    sqlx::query(sql)
        .fetch_all(db.pool())
        .await
        .unwrap()
        .iter()
        .map(|r| r.try_get::<String, _>(0).unwrap())
        .collect()
}

async fn retired_steps(location: &Location, data: &Path) {
    let all: Vec<i64> = (1..=i64::try_from(Db::schema_version()).unwrap()).collect();
    // A fresh database: every step recorded, 8 and 10 included, and no ai_ table.
    let db = Db::open(location).await.unwrap();
    assert!(Db::schema_version() >= 10);
    assert_eq!(versions(&db).await, all);
    let made = tables(&db).await;
    assert!(made.iter().all(|t| !t.starts_with("ai_")), "{made:?}");
    {
        let router = router_on(data.to_path_buf(), db.clone());
        sign_up(&router, "keeper").await;
    }
    db.pool().close().await;

    // The same database started again, as a 0.3.3 server left it (at step 10): nothing pending, nothing changed.
    let again = Db::open(location).await.unwrap();
    assert_eq!(again.pending_steps().await.unwrap(), Some(vec![]));
    assert_eq!(versions(&again).await, all);
    assert_eq!(tables(&again).await, made);
    assert_eq!(again.user_count().await.unwrap(), 1, "the accounts stay");
    again.pool().close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn the_empty_steps_on_sqlite() {
    let dir = tempfile::tempdir().unwrap();
    retired_steps(
        &Location::Sqlite(dir.path().join("party.sqlite")),
        &dir.path().join("data"),
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn the_empty_steps_on_postgres() {
    let Ok(url) = std::env::var("BOREALIS_TEST_POSTGRES_URL") else {
        eprintln!("skipped: set BOREALIS_TEST_POSTGRES_URL to run against PostgreSQL");
        return;
    };
    let _one = POSTGRES.lock().await;
    let dir = tempfile::tempdir().unwrap();
    drop_all(&url).await;
    retired_steps(&Location::Postgres(url.clone()), &dir.path().join("data")).await;
    drop_all(&url).await;
}
