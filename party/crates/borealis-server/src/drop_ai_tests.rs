//! Step 10 (0.3.3): the in-app assistant's tables (step 8: `ai_providers`, `ai_usage`) are dropped from a 0.3.2
//! database that has rows in them; the accounts stay; a second start changes nothing and never creates them again
//! (step 8 has no marker). On SQLite, and on PostgreSQL when `BOREALIS_TEST_POSTGRES_URL` names a database it may
//! wipe.

use super::*;

use sqlx::Row;

async fn version(db: &Db) -> i64 {
    sqlx::query("SELECT MAX(version) FROM schema_version")
        .fetch_one(db.pool())
        .await
        .unwrap()
        .try_get(0)
        .unwrap()
}

async fn has_table(db: &Db, name: &str) -> bool {
    let sql = if matches!(db.kind(), "sqlite") {
        "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = $1"
    } else {
        "SELECT 1 FROM information_schema.tables WHERE table_schema = current_schema() AND table_name = $1"
    };
    sqlx::query(sql)
        .bind(name)
        .fetch_optional(db.pool())
        .await
        .unwrap()
        .is_some()
}

async fn drops_the_ai_tables(location: &Location, data: &Path) {
    // 0.3.2: schema 9, an account with a saved provider (a sealed key) and a day of usage.
    let old = Db::open_at(location, 9).await.unwrap();
    let account = {
        let router = router_on(data.to_path_buf(), old.clone());
        sign_up(&router, "keeper").await;
        let row = sqlx::query("SELECT id FROM users WHERE username = 'keeper'")
            .fetch_one(old.pool())
            .await
            .unwrap();
        row.try_get::<String, _>(0).unwrap()
    };
    sqlx::query(
        "INSERT INTO ai_providers (user_id, provider, mode, base_url, model, key_sealed, key_last4, key_ring, \
         auto_apply, profile, consented_at, updated_at) VALUES ($1, 'openai_compatible', 'server', \
         'https://models.example/v1', 'some-model', 'c2VhbGVk', 'abcd', 'k1', 0, 'small', 1, 1)",
    )
    .bind(&account)
    .execute(old.pool())
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO ai_usage (user_id, day, requests, tokens) VALUES ($1, 20406, 3, 1200)",
    )
    .bind(&account)
    .execute(old.pool())
    .await
    .unwrap();
    assert_eq!(version(&old).await, 9);
    // The upgrade sees step 10 pending, so the backup before it is taken (backup.rs).
    assert_eq!(old.pending_steps().await.unwrap(), Some(vec![10]));
    old.pool().close().await;

    // The upgrade.
    let db = Db::open(location).await.unwrap();
    assert_eq!(version(&db).await, 10);
    assert_eq!(Db::schema_version(), 10);
    for table in ["ai_providers", "ai_usage"] {
        assert!(!has_table(&db, table).await, "{table} is still there");
    }
    assert_eq!(db.user_count().await.unwrap(), 1, "the accounts stay");
    assert!(has_table(&db, "api_tokens").await && has_table(&db, "oauth_tokens").await);
    db.pool().close().await;

    // A second start: nothing to do, nothing created again.
    let again = Db::open(location).await.unwrap();
    assert_eq!(again.pending_steps().await.unwrap(), Some(vec![]));
    assert_eq!(version(&again).await, 10);
    for table in ["ai_providers", "ai_usage"] {
        assert!(!has_table(&again, table).await, "{table} came back");
    }
    assert_eq!(again.user_count().await.unwrap(), 1);
    // And the instance still moves to a new database without them.
    if matches!(location, Location::Sqlite(_)) {
        let Location::Sqlite(file) = location else {
            unreachable!()
        };
        again.pool().close().await;
        let new = Db::open(&Location::Memory).await.unwrap();
        let report = crate::migrate::adopt_sqlite(&new, file)
            .await
            .unwrap()
            .unwrap();
        let moved = crate::migrate::describe(&report);
        assert!(moved.starts_with("users 1, "), "{moved}");
        assert!(!moved.contains("ai_"), "{moved}");
    } else {
        again.pool().close().await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_0_3_2_sqlite_database_drops_the_ai_tables() {
    let dir = tempfile::tempdir().unwrap();
    drops_the_ai_tables(
        &Location::Sqlite(dir.path().join("party.sqlite")),
        &dir.path().join("data"),
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_0_3_2_postgres_database_drops_the_ai_tables() {
    let Ok(url) = std::env::var("BOREALIS_TEST_POSTGRES_URL") else {
        eprintln!("skipped: set BOREALIS_TEST_POSTGRES_URL to run against PostgreSQL");
        return;
    };
    let _one = POSTGRES.lock().await;
    let dir = tempfile::tempdir().unwrap();
    drop_all(&url).await;
    drops_the_ai_tables(&Location::Postgres(url.clone()), &dir.path().join("data")).await;
    drop_all(&url).await;
}
