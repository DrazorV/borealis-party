//! Accounts, sessions, the admin page's settings and its activity log, in SQLite or PostgreSQL. Characters and
//! content are files in each account's Borealis data folder.
//!
//! Every query is written once in SQL that both databases accept: `$n` placeholders in ascending order of first use,
//! `lower(...)` instead of collations, `ON CONFLICT DO NOTHING`, and no booleans read back from SQL.

use std::time::{SystemTime, UNIX_EPOCH};

use sqlx::any::{AnyPoolOptions, AnyRow};
use sqlx::{AnyPool, Row};

use crate::auth::{Status, User};
use crate::error::{ApiError, ApiResult};

/// Applied in order; each runs once. Never edit a step that has shipped, add a new one.
const MIGRATIONS: &[&[&str]] = &[
    // 1: accounts, sessions, the admin page's settings and the activity log.
    &[
        "CREATE TABLE IF NOT EXISTS users (
            id TEXT PRIMARY KEY,
            username TEXT NOT NULL,
            password_hash TEXT NOT NULL,
            created_at BIGINT NOT NULL,
            email TEXT,
            status TEXT NOT NULL DEFAULT 'active',
            is_admin BIGINT NOT NULL DEFAULT 0,
            must_change_password BIGINT NOT NULL DEFAULT 0,
            email_verified_at BIGINT,
            verify_token_hash TEXT,
            verify_expires_at BIGINT
        )",
        "CREATE UNIQUE INDEX IF NOT EXISTS users_username ON users (lower(username))",
        "CREATE UNIQUE INDEX IF NOT EXISTS users_email ON users (lower(email))",
        "CREATE INDEX IF NOT EXISTS users_verify ON users (verify_token_hash)",
        "CREATE TABLE IF NOT EXISTS sessions (
            token_hash TEXT PRIMARY KEY,
            user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
            expires_at BIGINT NOT NULL
        )",
        "CREATE INDEX IF NOT EXISTS sessions_user ON sessions (user_id)",
        "CREATE TABLE IF NOT EXISTS server_settings (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL
        )",
        // No foreign keys: the log outlives the accounts it names.
        "CREATE TABLE IF NOT EXISTS activity (
            id TEXT PRIMARY KEY,
            at BIGINT NOT NULL,
            kind TEXT NOT NULL,
            actor_id TEXT,
            actor_name TEXT,
            target_id TEXT,
            target_name TEXT,
            detail TEXT,
            ip TEXT
        )",
    ],
    // 2 (M-shares, 0.1.5): share codes (shares.rs). Only the code's hash is kept; the file is in the sender's folder.
    &[
        "CREATE TABLE IF NOT EXISTS shares (
            id TEXT PRIMARY KEY,
            from_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
            code_hash TEXT NOT NULL,
            to_id TEXT REFERENCES users(id) ON DELETE CASCADE,
            to_name TEXT,
            file_name TEXT NOT NULL,
            characters BIGINT NOT NULL,
            packs BIGINT NOT NULL,
            links TEXT NOT NULL,
            bytes BIGINT NOT NULL,
            created_at BIGINT NOT NULL,
            expires_at BIGINT NOT NULL,
            claimed_at BIGINT,
            ended_at BIGINT,
            status TEXT NOT NULL
        )",
        "CREATE UNIQUE INDEX IF NOT EXISTS shares_code ON shares (code_hash)",
        "CREATE INDEX IF NOT EXISTS shares_from ON shares (from_id, status)",
        "CREATE INDEX IF NOT EXISTS shares_to ON shares (to_id, status)",
        "CREATE TABLE IF NOT EXISTS share_blocks (
            user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
            blocked_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
            created_at BIGINT NOT NULL,
            PRIMARY KEY (user_id, blocked_id)
        )",
    ],
    // 3 (M-parties, 0.2.0), after M-shares.
    // DM capability and parties: rows only, characters stay in their owner's folder (parties.rs). New names only:
    // a database from the versions with groups may still hold party_groups, invites and members, which stay alone.
    &[
        // Written only by the account itself (D2): no admin grants or revokes.
        "CREATE TABLE IF NOT EXISTS account_roles (
            user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
            role TEXT NOT NULL,
            granted_at BIGINT NOT NULL,
            granted_by TEXT,
            PRIMARY KEY (user_id, role)
        )",
        "CREATE TABLE IF NOT EXISTS parties (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            dm_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
            ruleset TEXT NOT NULL DEFAULT 'dnd2024',
            created_at BIGINT NOT NULL,
            updated_at BIGINT NOT NULL
        )",
        "CREATE INDEX IF NOT EXISTS parties_dm ON parties (dm_id)",
        // A username invite has no foreign key: one to a name nobody has is stored the same way.
        "CREATE TABLE IF NOT EXISTS party_invites (
            id TEXT PRIMARY KEY,
            party_id TEXT NOT NULL REFERENCES parties(id) ON DELETE CASCADE,
            invited_username_lower TEXT,
            code_hash TEXT,
            created_by TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
            created_at BIGINT NOT NULL,
            expires_at BIGINT NOT NULL,
            uses_left BIGINT NOT NULL DEFAULT 1,
            declined_at BIGINT
        )",
        "CREATE UNIQUE INDEX IF NOT EXISTS party_invites_code ON party_invites (code_hash)",
        "CREATE UNIQUE INDEX IF NOT EXISTS party_invites_name ON party_invites (party_id, invited_username_lower)",
        "CREATE INDEX IF NOT EXISTS party_invites_lookup ON party_invites (invited_username_lower)",
        // Join codes an account entered (kept through sign-up while it waits for approval, D39).
        "CREATE TABLE IF NOT EXISTS party_invite_holds (
            user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
            invite_id TEXT NOT NULL REFERENCES party_invites(id) ON DELETE CASCADE,
            created_at BIGINT NOT NULL,
            PRIMARY KEY (user_id, invite_id)
        )",
        "CREATE TABLE IF NOT EXISTS party_offers (
            id TEXT PRIMARY KEY,
            invite_id TEXT NOT NULL REFERENCES party_invites(id) ON DELETE CASCADE,
            owner_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
            character_id TEXT NOT NULL,
            created_at BIGINT NOT NULL,
            UNIQUE (owner_id, character_id),
            UNIQUE (invite_id, owner_id)
        )",
        // The key makes a character a member of at most one party (D19).
        "CREATE TABLE IF NOT EXISTS party_members (
            owner_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
            character_id TEXT NOT NULL,
            party_id TEXT NOT NULL REFERENCES parties(id) ON DELETE CASCADE,
            joined_at BIGINT NOT NULL,
            PRIMARY KEY (owner_id, character_id)
        )",
        "CREATE INDEX IF NOT EXISTS party_members_party ON party_members (party_id)",
    ],
    // 4 (M-monster-meta, 0.3.0), after M-parties: the creature details the 5etools import drops (D15), and what each
    // 5etools book's import converted, skipped or kept in part (bestiary.rs). Derived from the account's own imports;
    // a row is replaced whole when its pack is imported again, and rows of another pack revision are ignored.
    &[
        "CREATE TABLE IF NOT EXISTS monster_meta (
            owner_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
            element_id TEXT NOT NULL,
            pack_id TEXT NOT NULL,
            pack_rev TEXT NOT NULL,
            environment_json TEXT NOT NULL,
            page BIGINT,
            extras_json TEXT NOT NULL,
            captured_at BIGINT NOT NULL,
            PRIMARY KEY (owner_id, element_id)
        )",
        "CREATE INDEX IF NOT EXISTS monster_meta_pack ON monster_meta (owner_id, pack_id)",
        "CREATE TABLE IF NOT EXISTS bestiary_imports (
            owner_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
            pack_id TEXT NOT NULL,
            pack_rev TEXT NOT NULL,
            source TEXT NOT NULL,
            in_data BIGINT NOT NULL,
            skipped_json TEXT NOT NULL,
            partial_json TEXT NOT NULL,
            captured_at BIGINT NOT NULL,
            PRIMARY KEY (owner_id, pack_id)
        )",
    ],
    // 5 (M-encounters, 0.3.0), after M-monster-meta.
    // A DM's encounters (encounters.rs). No dm_id: authorization always joins parties.dm_id; the edition is the party's
    // (D23 b). Each creature keeps a copy of its stat block (a Monster's or an Aurora Companion's, D17 b), so a player's
    // view never depends on anyone's installed content. A controlled creature (pet, summon) names its owner in
    // owner_creature_id; no foreign key, so the copy between databases needs no row order (encounters.rs removes a
    // creature's controlled creatures with it).
    &[
        "CREATE TABLE IF NOT EXISTS encounters (
            id TEXT PRIMARY KEY,
            party_id TEXT NOT NULL REFERENCES parties(id) ON DELETE CASCADE,
            name TEXT NOT NULL,
            public_name TEXT NOT NULL,
            status TEXT NOT NULL DEFAULT 'draft',
            player_visible BIGINT NOT NULL DEFAULT 0,
            hp_mode TEXT NOT NULL DEFAULT 'average',
            dm_notes TEXT NOT NULL DEFAULT '',
            created_at BIGINT NOT NULL,
            updated_at BIGINT NOT NULL
        )",
        "CREATE INDEX IF NOT EXISTS encounters_party ON encounters (party_id, created_at)",
        "CREATE TABLE IF NOT EXISTS encounter_creatures (
            id TEXT PRIMARY KEY,
            encounter_id TEXT NOT NULL REFERENCES encounters(id) ON DELETE CASCADE,
            sort BIGINT NOT NULL,
            kind TEXT NOT NULL DEFAULT 'monster',
            dm_label TEXT NOT NULL,
            public_name TEXT NOT NULL,
            element_id TEXT NOT NULL,
            pack_id TEXT NOT NULL,
            pack_rev TEXT NOT NULL,
            statblock_json TEXT NOT NULL,
            overrides_json TEXT NOT NULL DEFAULT '{}',
            automation_json TEXT,
            hp_mode TEXT NOT NULL DEFAULT 'encounter',
            hp_max BIGINT,
            visibility TEXT NOT NULL DEFAULT 'hidden',
            revealed_at BIGINT,
            owner_creature_id TEXT,
            init_mode TEXT NOT NULL DEFAULT 'after_owner',
            created_at BIGINT NOT NULL,
            updated_at BIGINT NOT NULL
        )",
        "CREATE INDEX IF NOT EXISTS encounter_creatures_encounter ON encounter_creatures (encounter_id, sort)",
        // Starting effects (companion C19): applied when combat starts.
        "CREATE TABLE IF NOT EXISTS encounter_effects (
            id TEXT PRIMARY KEY,
            encounter_id TEXT NOT NULL REFERENCES encounters(id) ON DELETE CASCADE,
            creature_id TEXT NOT NULL REFERENCES encounter_creatures(id) ON DELETE CASCADE,
            sort BIGINT NOT NULL,
            name TEXT NOT NULL,
            detail TEXT NOT NULL DEFAULT '',
            source_creature_id TEXT REFERENCES encounter_creatures(id) ON DELETE SET NULL,
            duration_turns BIGINT,
            charges BIGINT,
            concentration BIGINT NOT NULL DEFAULT 0,
            slot_level BIGINT,
            modifiers_json TEXT NOT NULL DEFAULT '{}',
            created_at BIGINT NOT NULL
        )",
        "CREATE INDEX IF NOT EXISTS encounter_effects_encounter ON encounter_effects (encounter_id, sort)",
        // Encounter rules (companion C20, D42): a banner and on/off flags tied to a creature, no scripting.
        "CREATE TABLE IF NOT EXISTS encounter_rules (
            id TEXT PRIMARY KEY,
            encounter_id TEXT NOT NULL REFERENCES encounters(id) ON DELETE CASCADE,
            sort BIGINT NOT NULL,
            name TEXT NOT NULL,
            text TEXT NOT NULL DEFAULT '',
            creature_id TEXT REFERENCES encounter_creatures(id) ON DELETE SET NULL,
            ends_when TEXT NOT NULL DEFAULT 'creature_down',
            active BIGINT NOT NULL DEFAULT 1,
            flags_json TEXT NOT NULL DEFAULT '[]',
            created_at BIGINT NOT NULL
        )",
        "CREATE INDEX IF NOT EXISTS encounter_rules_encounter ON encounter_rules (encounter_id, sort)",
    ],
    // 6 (M-combat, 0.3.0), after M-encounters.
    // Live combats (combat.rs). The engine's state and die are JSON in `combats`, with `seq` as the compare-and-set
    // counter of every write. `encounter_id` has no foreign key: a combat (and its log) outlives its encounter, and
    // M-encounters' tables stay droppable on their own. Everything else cascades from the party; the log, journal and
    // prompts of a combat ended more than 90 days ago are deleted (D32 a).
    &[
        "CREATE TABLE IF NOT EXISTS combats (
            id TEXT PRIMARY KEY,
            party_id TEXT NOT NULL REFERENCES parties(id) ON DELETE CASCADE,
            encounter_id TEXT,
            status TEXT NOT NULL DEFAULT 'live',
            seq BIGINT NOT NULL DEFAULT 0,
            state_json TEXT NOT NULL,
            die_json TEXT NOT NULL,
            started_at BIGINT NOT NULL,
            updated_at BIGINT NOT NULL,
            ended_at BIGINT
        )",
        "CREATE INDEX IF NOT EXISTS combats_party ON combats (party_id, started_at)",
        // At most one live combat per party, kept by the database itself.
        "CREATE UNIQUE INDEX IF NOT EXISTS combats_live ON combats (party_id) WHERE status = 'live'",
        // Who each combatant id is: a creature with the stat block it fights with, or a PC by owner and character.
        "CREATE TABLE IF NOT EXISTS combatants (
            combat_id TEXT NOT NULL REFERENCES combats(id) ON DELETE CASCADE,
            id TEXT NOT NULL,
            kind TEXT NOT NULL,
            encounter_creature_id TEXT,
            owner_id TEXT,
            character_id TEXT,
            statblock_json TEXT,
            name TEXT,
            origin TEXT,
            created_at BIGINT NOT NULL,
            PRIMARY KEY (combat_id, id)
        )",
        "CREATE INDEX IF NOT EXISTS combatants_owner ON combatants (owner_id)",
        // Undo (C8): the state before each write, the newest 50 per combat.
        "CREATE TABLE IF NOT EXISTS combat_journal (
            combat_id TEXT NOT NULL REFERENCES combats(id) ON DELETE CASCADE,
            seq BIGINT NOT NULL,
            author TEXT NOT NULL,
            label TEXT NOT NULL,
            before_json TEXT NOT NULL,
            PRIMARY KEY (combat_id, seq)
        )",
        // The DM-only log (D32 a). Never read by a player route.
        "CREATE TABLE IF NOT EXISTS combat_log (
            combat_id TEXT NOT NULL REFERENCES combats(id) ON DELETE CASCADE,
            seq BIGINT NOT NULL,
            n BIGINT NOT NULL,
            at BIGINT NOT NULL,
            author TEXT NOT NULL,
            event_json TEXT NOT NULL,
            PRIMARY KEY (combat_id, seq, n)
        )",
        // Each player's last projection hash and own sequence: events move only when that player's view changed.
        "CREATE TABLE IF NOT EXISTS combat_views (
            combat_id TEXT NOT NULL REFERENCES combats(id) ON DELETE CASCADE,
            viewer_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
            seq BIGINT NOT NULL,
            hash TEXT NOT NULL,
            PRIMARY KEY (combat_id, viewer_id)
        )",
        // Damage and heal prompts for a PC's owner (D7 a).
        "CREATE TABLE IF NOT EXISTS combat_prompts (
            id TEXT PRIMARY KEY,
            combat_id TEXT NOT NULL REFERENCES combats(id) ON DELETE CASCADE,
            combatant_id TEXT NOT NULL,
            owner_id TEXT NOT NULL,
            kind TEXT NOT NULL,
            payload_json TEXT NOT NULL,
            created_at BIGINT NOT NULL,
            resolved_at BIGINT,
            applied BIGINT
        )",
        "CREATE INDEX IF NOT EXISTS combat_prompts_open ON combat_prompts (combat_id, resolved_at)",
        // Request ids already done (D44 retries), by scope: a combat id, or start/import of an encounter or party.
        "CREATE TABLE IF NOT EXISTS combat_requests (
            scope TEXT NOT NULL,
            account_id TEXT NOT NULL,
            request_id TEXT NOT NULL,
            result TEXT NOT NULL,
            response_json TEXT,
            created_at BIGINT NOT NULL,
            PRIMARY KEY (scope, account_id, request_id)
        )",
        "CREATE INDEX IF NOT EXISTS combat_requests_at ON combat_requests (created_at)",
    ],
    // 7 (M-ai-auth, 0.3.1), after M-combat: personal API tokens and the OAuth server for AI apps (tokens.rs,
    // oauth.rs). Every token, code, client secret and consent request id is kept only as a peppered hash; scopes are
    // space-separated. A token never grants more than its account: the role checks run on every call.
    &[
        "CREATE TABLE IF NOT EXISTS api_tokens (
            id TEXT PRIMARY KEY,
            user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
            name TEXT NOT NULL,
            token_hash TEXT NOT NULL,
            hint TEXT NOT NULL,
            scopes TEXT NOT NULL,
            created_at BIGINT NOT NULL,
            expires_at BIGINT NOT NULL,
            last_used_at BIGINT
        )",
        "CREATE UNIQUE INDEX IF NOT EXISTS api_tokens_hash ON api_tokens (token_hash)",
        "CREATE INDEX IF NOT EXISTS api_tokens_user ON api_tokens (user_id)",
        // Apps registered by dynamic client registration (RFC 7591). No owner: anyone may register one.
        "CREATE TABLE IF NOT EXISTS oauth_clients (
            id TEXT PRIMARY KEY,
            secret_hash TEXT,
            name TEXT NOT NULL,
            client_uri TEXT,
            redirect_uris_json TEXT NOT NULL,
            auth_method TEXT NOT NULL,
            created_at BIGINT NOT NULL
        )",
        "CREATE INDEX IF NOT EXISTS oauth_clients_created ON oauth_clients (created_at)",
        // An authorization request waiting on the consent screen (minutes only).
        "CREATE TABLE IF NOT EXISTS oauth_requests (
            id_hash TEXT PRIMARY KEY,
            client_id TEXT NOT NULL REFERENCES oauth_clients(id) ON DELETE CASCADE,
            redirect_uri TEXT NOT NULL,
            scopes TEXT NOT NULL,
            state TEXT,
            code_challenge TEXT NOT NULL,
            resource TEXT NOT NULL,
            created_at BIGINT NOT NULL,
            expires_at BIGINT NOT NULL
        )",
        // An authorization code. grant_id is set when it is redeemed, so a second use revokes what it gave.
        "CREATE TABLE IF NOT EXISTS oauth_codes (
            code_hash TEXT PRIMARY KEY,
            client_id TEXT NOT NULL REFERENCES oauth_clients(id) ON DELETE CASCADE,
            user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
            redirect_uri TEXT NOT NULL,
            scopes TEXT NOT NULL,
            code_challenge TEXT NOT NULL,
            resource TEXT NOT NULL,
            created_at BIGINT NOT NULL,
            expires_at BIGINT NOT NULL,
            used_at BIGINT,
            grant_id TEXT
        )",
        // A connected app: one account's consent to one client. Its tokens go with it.
        "CREATE TABLE IF NOT EXISTS oauth_grants (
            id TEXT PRIMARY KEY,
            user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
            client_id TEXT NOT NULL REFERENCES oauth_clients(id) ON DELETE CASCADE,
            scopes TEXT NOT NULL,
            resource TEXT NOT NULL,
            created_at BIGINT NOT NULL,
            last_used_at BIGINT
        )",
        "CREATE INDEX IF NOT EXISTS oauth_grants_user ON oauth_grants (user_id)",
        "CREATE INDEX IF NOT EXISTS oauth_grants_client ON oauth_grants (client_id)",
        // Access and refresh tokens. A refresh token is used once (rotation); a second use revokes the grant.
        "CREATE TABLE IF NOT EXISTS oauth_tokens (
            token_hash TEXT PRIMARY KEY,
            grant_id TEXT NOT NULL REFERENCES oauth_grants(id) ON DELETE CASCADE,
            kind TEXT NOT NULL,
            scopes TEXT NOT NULL,
            created_at BIGINT NOT NULL,
            expires_at BIGINT NOT NULL,
            used_at BIGINT
        )",
        "CREATE INDEX IF NOT EXISTS oauth_tokens_grant ON oauth_tokens (grant_id)",
        "CREATE INDEX IF NOT EXISTS oauth_tokens_expires ON oauth_tokens (expires_at)",
    ],
    // 8 (M-ai-providers, 0.3.1), after M-ai-auth. Bring your own AI (ai_providers.rs): one provider per account, its key encrypted with the
    // server's `[ai] secret_key` (never stored in the clear, never in the account's folder, so never in a bundle or a
    // share), and each account's tokens per day for the budget. Both go with the account.
    &[
        "CREATE TABLE IF NOT EXISTS ai_providers (
            user_id TEXT PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
            provider TEXT NOT NULL,
            mode TEXT NOT NULL,
            base_url TEXT,
            model TEXT NOT NULL,
            key_sealed TEXT,
            key_last4 TEXT,
            key_ring TEXT,
            auto_apply BIGINT NOT NULL DEFAULT 0,
            profile TEXT NOT NULL DEFAULT 'small',
            consented_at BIGINT NOT NULL,
            updated_at BIGINT NOT NULL
        )",
        "CREATE TABLE IF NOT EXISTS ai_usage (
            user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
            day BIGINT NOT NULL,
            requests BIGINT NOT NULL DEFAULT 0,
            tokens BIGINT NOT NULL DEFAULT 0,
            PRIMARY KEY (user_id, day)
        )",
    ],
    // 9 (M-spells, 0.3.2), after M-ai-providers. Spells as mechanics in combat (spells.rs).
    // `spell_mechanics`: what a DM decided about spells, per account (D: per account, with a JSON file export and
    // import; not in homebrew packs yet). One table, by `kind`:
    // - `def`: an effect the DM built (`def_json` is the engine's SpellMech, `schema_v` its version, `rev` bumped on
    //   every save);
    // - `link`: "use that spell's mechanics" (`def_json` is `{"target": <spell key>}`), e.g. a 5etools Fireball linked
    //   to the SRD one after the "Use SRD Fireball rules?" banner;
    // - `alias`: a name that means this spell (`alias_name` normalized): a monster spellbook's ambiguous or unlinked
    //   name the DM resolved once, and the DM's override of the "one spell per edition" choice.
    // `alias_name` is '' for the other kinds, so the primary key needs no NULL. `origin` says where a row came from
    // (dm, suggested, import); `source_hash` is the drafted text's hash for "this changed, check it again".
    &[
        "CREATE TABLE IF NOT EXISTS spell_mechanics (
            account_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
            spell_key TEXT NOT NULL,
            ruleset TEXT NOT NULL,
            kind TEXT NOT NULL,
            alias_name TEXT NOT NULL DEFAULT '',
            def_json TEXT NOT NULL DEFAULT '{}',
            schema_v BIGINT NOT NULL DEFAULT 1,
            rev BIGINT NOT NULL DEFAULT 1,
            origin TEXT NOT NULL DEFAULT 'dm',
            source_hash TEXT,
            created_at BIGINT NOT NULL,
            updated_at BIGINT NOT NULL,
            PRIMARY KEY (account_id, spell_key, ruleset, kind, alias_name)
        )",
        "CREATE INDEX IF NOT EXISTS spell_mechanics_alias ON spell_mechanics (account_id, ruleset, kind, alias_name)",
        // Precast spells (they replace "Starting effect"; legacy rows stay in encounter_effects as "Custom effect"):
        // cast when combat starts, with their real effect, and their slot spent. `slot_level` NULL is a cantrip or an
        // at-will/per-day use. `name` is kept for display when the spell element is gone.
        "CREATE TABLE IF NOT EXISTS encounter_casts (
            id TEXT PRIMARY KEY,
            encounter_id TEXT NOT NULL REFERENCES encounters(id) ON DELETE CASCADE,
            caster_creature_id TEXT NOT NULL REFERENCES encounter_creatures(id) ON DELETE CASCADE,
            sort BIGINT NOT NULL,
            spell_key TEXT NOT NULL,
            name TEXT NOT NULL DEFAULT '',
            slot_level BIGINT,
            targets_json TEXT NOT NULL DEFAULT '[]',
            choices_json TEXT NOT NULL DEFAULT '{}',
            created_at BIGINT NOT NULL
        )",
        "CREATE INDEX IF NOT EXISTS encounter_casts_encounter ON encounter_casts (encounter_id, sort)",
    ],
];

/// How many steps [`MIGRATIONS`] has: the schema version a database is at after [`Db::open`].
#[cfg(test)]
pub const SCHEMA_VERSION: usize = MIGRATIONS.len();

/// The table each step from 2 on creates, by step number (1-based). A database whose `schema_version` says a step ran
/// but that lacks its table runs the step again at start-up: the removed groups versions also counted 1 to 3, so a
/// database they left at version 3 would otherwise skip these steps. Every such step must be idempotent (`CREATE ...
/// IF NOT EXISTS`). Step 1 has none: every database has its tables. A new step adds its own line here, with its
/// main table.
const MARKERS: &[(usize, &str)] = &[
    (2, "shares"),
    (3, "parties"),
    (4, "monster_meta"),
    (5, "encounters"),
    (6, "combats"),
    (7, "api_tokens"),
    (8, "ai_providers"),
    (9, "spell_mechanics"),
];

/// A table only the versions with groups had. A database that still has it opens like any other: the table is left
/// alone and a warning is logged, so an instance that ran those versions (or rolls back to one) still boots.
const OLD_TABLE: &str = "party_groups";

/// Rows the activity log keeps; older ones are pruned as new ones arrive.
pub const ACTIVITY_KEPT: i64 = 10_000;

/// Why the database could not be opened.
#[derive(Debug)]
pub enum OpenError {
    /// It can't be reached, or its tables can't be created.
    Database(sqlx::Error),
}

impl std::fmt::Display for OpenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Database(e) => write!(f, "database: {e}"),
        }
    }
}

impl std::error::Error for OpenError {}

impl From<sqlx::Error> for OpenError {
    fn from(e: sqlx::Error) -> Self {
        Self::Database(e)
    }
}

/// The account columns [`user_from`] reads, from `users u`.
const USER_COLUMNS: &str = "u.id, u.username, u.email, u.is_admin, u.must_change_password";

/// The columns [`account_from`] reads, from `users u`.
const ACCOUNT_COLUMNS: &str =
    "u.id, u.username, u.email, u.is_admin, u.must_change_password, u.status, \
     u.created_at, u.email_verified_at, u.verify_token_hash, \
     (SELECT COUNT(*) FROM sessions s WHERE s.user_id = u.id AND s.expires_at > $1)";

/// A condition on `users` that leaves the last active admin alone: true when the row is not one, or another active
/// admin exists. `id` is the placeholder holding the row's id.
fn not_last_admin(id: &str) -> String {
    format!(
        "(is_admin = 0 OR status <> 'active' OR \
         (SELECT COUNT(*) FROM users o WHERE o.is_admin = 1 AND o.status = 'active' AND o.id <> {id}) > 0)"
    )
}

/// Where the database is.
#[derive(Debug, Clone)]
pub enum Location {
    /// A SQLite file.
    Sqlite(std::path::PathBuf),
    /// SQLite in memory (tests).
    #[cfg(test)]
    Memory,
    /// A PostgreSQL connection URL.
    Postgres(String),
}

/// An account to add.
pub struct NewAccount<'a> {
    /// Id.
    pub id: &'a str,
    /// Username.
    pub username: &'a str,
    /// Email address, already checked.
    pub email: Option<&'a str>,
    /// Argon2 hash.
    pub password_hash: &'a str,
    /// Where it starts.
    pub status: Status,
    /// Whether it is an admin.
    pub admin: bool,
    /// The hash of the emailed link's token, while the address is unconfirmed.
    pub verify_token_hash: Option<&'a str>,
    /// When that link stops working.
    pub verify_expires_at: Option<i64>,
}

/// Why an account wasn't added.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InsertOutcome {
    /// Added.
    Added,
    /// The username is taken.
    UsernameTaken,
    /// The email address is taken.
    EmailTaken,
}

/// An account as an admin sees it.
#[derive(Debug, Clone)]
pub struct AccountRow {
    /// The account.
    pub user: User,
    /// Status.
    pub status: Status,
    /// When it was created.
    pub created_at: i64,
    /// When the email address was confirmed.
    pub email_verified_at: Option<i64>,
    /// Still waiting for the emailed link to be opened.
    pub awaiting_email: bool,
    /// Live sessions.
    pub sessions: i64,
}

/// What a sign-in found for a username or email address.
pub struct Login {
    /// The account.
    pub user: User,
    /// Its password hash.
    pub password_hash: String,
    /// Whether it may sign in.
    pub status: Status,
    /// Still waiting for the emailed link to be opened.
    pub awaiting_email: bool,
}

/// One activity log entry.
#[derive(Debug, Clone, Default)]
pub struct Activity {
    /// When.
    pub at: i64,
    /// What happened, for example `sign_in` or `approve`.
    pub kind: String,
    /// Who did it.
    pub actor_id: Option<String>,
    /// Their username then.
    pub actor_name: Option<String>,
    /// Whom or what it was done to.
    pub target_id: Option<String>,
    /// Its name then.
    pub target_name: Option<String>,
    /// More words.
    pub detail: Option<String>,
    /// The client's address.
    pub ip: Option<String>,
}

/// Counts for the admin page.
#[derive(Debug, Clone, Default)]
pub struct Counts {
    /// Accounts by status.
    pub accounts: Vec<(String, i64)>,
    /// Admins.
    pub admins: i64,
    /// Unexpired sessions.
    pub sessions: i64,
}

/// The database.
#[derive(Clone)]
pub struct Db {
    pool: AnyPool,
    kind: &'static str,
}

/// Seconds since the epoch.
pub fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
}

impl From<sqlx::Error> for ApiError {
    fn from(e: sqlx::Error) -> Self {
        Self::Internal(format!("database: {e}"))
    }
}

fn user_from(row: &AnyRow) -> sqlx::Result<User> {
    Ok(User {
        id: row.try_get(0)?,
        username: row.try_get(1)?,
        email: row.try_get(2)?,
        admin: row.try_get::<i64, _>(3)? != 0,
        must_change_password: row.try_get::<i64, _>(4)? != 0,
    })
}

fn account_from(row: &AnyRow) -> sqlx::Result<AccountRow> {
    Ok(AccountRow {
        user: user_from(row)?,
        status: Status::parse(&row.try_get::<String, _>(5)?),
        created_at: row.try_get(6)?,
        email_verified_at: row.try_get(7)?,
        awaiting_email: row.try_get::<Option<String>, _>(8)?.is_some(),
        sessions: row.try_get(9)?,
    })
}

fn flag(value: bool) -> i64 {
    i64::from(value)
}

impl Db {
    /// The connection pool, for the migration between databases.
    pub(crate) fn pool(&self) -> &AnyPool {
        &self.pool
    }

    /// `sqlite` or `postgres`.
    pub fn kind(&self) -> &'static str {
        self.kind
    }

    /// Connect, and create or update the tables.
    ///
    /// # Errors
    /// When the database can't be reached, its tables can't be created, or it was made by a version with groups.
    pub async fn open(location: &Location) -> Result<Self, OpenError> {
        Self::open_upto(location, MIGRATIONS.len()).await
    }

    /// As [`Db::open`], with only the first `max_version` steps applied: a database as an older release left it, for
    /// upgrade tests.
    #[cfg(test)]
    pub async fn open_at(location: &Location, max_version: usize) -> Result<Self, OpenError> {
        Self::open_upto(location, max_version).await
    }

    /// How many migrations this build has.
    #[cfg(test)]
    pub fn schema_version() -> usize {
        MIGRATIONS.len()
    }

    async fn open_upto(location: &Location, max_version: usize) -> Result<Self, OpenError> {
        let db = Self::connect(location).await?;
        db.migrate(max_version).await?;
        Ok(db)
    }

    /// Connect without creating or updating any table, so a backup can be taken first ([`crate::backup`]); then
    /// [`Db::upgrade`].
    ///
    /// # Errors
    /// When the database can't be reached.
    pub async fn connect(location: &Location) -> Result<Self, OpenError> {
        sqlx::any::install_default_drivers();
        let (url, sqlite, max) = match location {
            Location::Sqlite(path) => (format!("sqlite://{}?mode=rwc", path.display()), true, 4),
            #[cfg(test)]
            Location::Memory => ("sqlite::memory:".to_string(), true, 1),
            Location::Postgres(url) => (url.clone(), false, 10),
        };
        let pool = AnyPoolOptions::new()
            .max_connections(max)
            .after_connect(move |conn, _| {
                Box::pin(async move {
                    if sqlite {
                        sqlx::query("PRAGMA foreign_keys = ON")
                            .execute(&mut *conn)
                            .await?;
                        sqlx::query("PRAGMA busy_timeout = 5000")
                            .execute(&mut *conn)
                            .await?;
                    }
                    Ok(())
                })
            })
            .connect(&url)
            .await?;
        let old = sqlx::query(if sqlite {
            "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = $1"
        } else {
            "SELECT 1 FROM information_schema.tables WHERE table_schema = current_schema() AND table_name = $1"
        })
        .bind(OLD_TABLE)
        .fetch_optional(&pool)
        .await?;
        if old.is_some() {
            // No refusal, and no "schema too new" refusal either: rolling back depends on older binaries booting.
            tracing::warn!(
                "the database still has the {OLD_TABLE} table from a version with groups; it is not used and is left alone"
            );
        }
        if sqlite {
            // Persistent per file; memory databases ignore it.
            sqlx::query("PRAGMA journal_mode = WAL")
                .execute(&pool)
                .await?;
        }
        Ok(Self {
            pool,
            kind: if sqlite { "sqlite" } else { "postgres" },
        })
    }

    /// Create or update the tables of a database [`Db::connect`] opened.
    ///
    /// # Errors
    /// When a step fails.
    pub async fn upgrade(&self) -> Result<(), OpenError> {
        Ok(self.migrate(MIGRATIONS.len()).await?)
    }

    /// The steps [`Db::upgrade`] would run, without running any: those after the recorded version, and recorded ones
    /// whose marker table is missing. `None` for a database with no `schema_version` table yet (nothing in it to lose).
    ///
    /// # Errors
    /// When the database can't be read.
    pub async fn pending_steps(&self) -> sqlx::Result<Option<Vec<i64>>> {
        if !self.has_table("schema_version").await? {
            return Ok(None);
        }
        let current: i64 = sqlx::query("SELECT COALESCE(MAX(version), 0) FROM schema_version")
            .fetch_one(&self.pool)
            .await?
            .try_get(0)?;
        let mut pending = Vec::new();
        for index in 0..MIGRATIONS.len() {
            let version = i64::try_from(index).unwrap_or(i64::MAX) + 1;
            let missing = match MARKERS.iter().find(|(step, _)| *step == index + 1) {
                _ if version > current => true,
                Some((_, marker)) => !self.has_table(marker).await?,
                None => false,
            };
            if missing {
                pending.push(version);
            }
        }
        Ok(Some(pending))
    }

    /// True when the table `name` exists.
    async fn has_table(&self, name: &str) -> sqlx::Result<bool> {
        Ok(sqlx::query(if self.kind == "sqlite" {
            "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = $1"
        } else {
            "SELECT 1 FROM information_schema.tables WHERE table_schema = current_schema() AND table_name = $1"
        })
        .bind(name)
        .fetch_optional(&self.pool)
        .await?
        .is_some())
    }

    /// Run one step and record it (unless its version is recorded already), in one transaction.
    async fn run_step(&self, version: i64, steps: &[&str]) -> sqlx::Result<()> {
        let mut tx = self.pool.begin().await?;
        for step in steps {
            sqlx::query(step).execute(&mut *tx).await?;
        }
        let recorded = sqlx::query("SELECT 1 FROM schema_version WHERE version = $1")
            .bind(version)
            .fetch_optional(&mut *tx)
            .await?;
        if recorded.is_none() {
            sqlx::query("INSERT INTO schema_version (version) VALUES ($1)")
                .bind(version)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await
    }

    async fn migrate(&self, max_version: usize) -> sqlx::Result<()> {
        sqlx::query("CREATE TABLE IF NOT EXISTS schema_version (version BIGINT NOT NULL)")
            .execute(&self.pool)
            .await?;
        let current: i64 = sqlx::query("SELECT COALESCE(MAX(version), 0) FROM schema_version")
            .fetch_one(&self.pool)
            .await?
            .try_get(0)?;
        for (index, steps) in MIGRATIONS.iter().enumerate().take(max_version) {
            let version = i64::try_from(index).unwrap_or(i64::MAX) + 1;
            if version > current {
                self.run_step(version, steps).await?;
                continue;
            }
            // Recorded as done: trust it unless the step's own table is missing (a database another numbering left).
            let Some((_, marker)) = MARKERS.iter().find(|(step, _)| *step == index + 1) else {
                continue;
            };
            if !self.has_table(marker).await? {
                tracing::warn!(
                    "the database is at schema version {current} but has no {marker} table: running step {version} again (it only adds what is missing)"
                );
                self.run_step(version, steps).await?;
            }
        }
        Ok(())
    }

    // Accounts and sessions.

    /// Add an account.
    pub async fn insert_user(&self, account: &NewAccount<'_>) -> ApiResult<InsertOutcome> {
        let mut tx = self.pool.begin().await?;
        let taken = sqlx::query("SELECT 1 FROM users WHERE lower(username) = lower($1)")
            .bind(account.username)
            .fetch_optional(&mut *tx)
            .await?;
        if taken.is_some() {
            return Ok(InsertOutcome::UsernameTaken);
        }
        if let Some(email) = account.email {
            let taken = sqlx::query("SELECT 1 FROM users WHERE lower(email) = lower($1)")
                .bind(email)
                .fetch_optional(&mut *tx)
                .await?;
            if taken.is_some() {
                return Ok(InsertOutcome::EmailTaken);
            }
        }
        let done = sqlx::query(
            "INSERT INTO users (id, username, password_hash, created_at, email, status, is_admin,
                must_change_password, verify_token_hash, verify_expires_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7, 0, $8, $9)
             ON CONFLICT DO NOTHING",
        )
        .bind(account.id)
        .bind(account.username)
        .bind(account.password_hash)
        .bind(now())
        .bind(account.email.map(str::to_string))
        .bind(account.status.as_str())
        .bind(flag(account.admin))
        .bind(account.verify_token_hash.map(str::to_string))
        .bind(account.verify_expires_at)
        .execute(&mut *tx)
        .await?;
        if done.rows_affected() == 0 {
            return Ok(InsertOutcome::UsernameTaken);
        }
        tx.commit().await?;
        Ok(InsertOutcome::Added)
    }

    /// How many accounts exist.
    pub async fn user_count(&self) -> ApiResult<i64> {
        Ok(sqlx::query("SELECT COUNT(*) FROM users")
            .fetch_one(&self.pool)
            .await?
            .try_get(0)?)
    }

    /// An account by username or email address (any case), for signing in.
    pub async fn find_login(&self, name: &str) -> ApiResult<Option<Login>> {
        let row = sqlx::query(&format!(
            "SELECT {USER_COLUMNS}, u.password_hash, u.status, u.verify_token_hash FROM users u
             WHERE lower(u.username) = lower($1) OR lower(u.email) = lower($1)
             ORDER BY u.created_at LIMIT 1"
        ))
        .bind(name)
        .fetch_optional(&self.pool)
        .await?;
        let Some(r) = row else {
            return Ok(None);
        };
        Ok(Some(Login {
            user: user_from(&r)?,
            password_hash: r.try_get(5)?,
            status: Status::parse(&r.try_get::<String, _>(6)?),
            awaiting_email: r.try_get::<Option<String>, _>(7)?.is_some(),
        }))
    }

    /// An account's password hash.
    pub async fn password_hash(&self, user: &str) -> ApiResult<Option<String>> {
        let row = sqlx::query("SELECT password_hash FROM users WHERE id = $1")
            .bind(user)
            .fetch_optional(&self.pool)
            .await?;
        Ok(row.map(|r| r.try_get(0)).transpose()?)
    }

    /// Store a new session and forget expired ones.
    pub async fn insert_session(
        &self,
        token_hash: &str,
        user: &str,
        expires_at: i64,
    ) -> ApiResult<()> {
        sqlx::query("DELETE FROM sessions WHERE expires_at < $1")
            .bind(now())
            .execute(&self.pool)
            .await?;
        sqlx::query("INSERT INTO sessions (token_hash, user_id, expires_at) VALUES ($1, $2, $3)")
            .bind(token_hash)
            .bind(user)
            .bind(expires_at)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// The active account behind a live session.
    /// The user of a live session, with the time the session ends.
    pub async fn session_user(&self, token_hash: &str) -> ApiResult<Option<(User, i64)>> {
        let row = sqlx::query(&format!(
            "SELECT {USER_COLUMNS}, s.expires_at FROM sessions s JOIN users u ON u.id = s.user_id
             WHERE s.token_hash = $1 AND s.expires_at > $2 AND u.status = 'active'"
        ))
        .bind(token_hash)
        .bind(now())
        .fetch_optional(&self.pool)
        .await?;
        let Some(r) = row else { return Ok(None) };
        Ok(Some((user_from(&r)?, r.try_get("expires_at")?)))
    }

    /// Move a live session's end to `expires_at` when it ends before `renew_before` (once a day at most).
    pub async fn renew_session(
        &self,
        token_hash: &str,
        expires_at: i64,
        renew_before: i64,
    ) -> ApiResult<()> {
        sqlx::query(
            "UPDATE sessions SET expires_at = $1
             WHERE token_hash = $2 AND expires_at < $3 AND expires_at > $4",
        )
        .bind(expires_at)
        .bind(token_hash)
        .bind(renew_before)
        .bind(now())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Sign a session out.
    pub async fn delete_session(&self, token_hash: &str) -> ApiResult<()> {
        sqlx::query("DELETE FROM sessions WHERE token_hash = $1")
            .bind(token_hash)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Sign an account out everywhere; how many sessions ended.
    pub async fn delete_sessions_of(&self, user: &str) -> ApiResult<u64> {
        let done = sqlx::query("DELETE FROM sessions WHERE user_id = $1")
            .bind(user)
            .execute(&self.pool)
            .await?;
        Ok(done.rows_affected())
    }

    /// Sign an account out everywhere but one session.
    pub async fn delete_other_sessions(&self, user: &str, keep: &str) -> ApiResult<()> {
        sqlx::query("DELETE FROM sessions WHERE user_id = $1 AND token_hash <> $2")
            .bind(user)
            .bind(keep)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Confirm the email address an emailed link belongs to; the account, or `None` for an unknown or old link.
    pub async fn confirm_email(&self, token_hash: &str) -> ApiResult<Option<AccountRow>> {
        let mut tx = self.pool.begin().await?;
        let found = sqlx::query(
            "SELECT id FROM users WHERE verify_token_hash = $1 AND verify_expires_at > $2",
        )
        .bind(token_hash)
        .bind(now())
        .fetch_optional(&mut *tx)
        .await?;
        let Some(found) = found else {
            return Ok(None);
        };
        let id: String = found.try_get(0)?;
        sqlx::query(
            "UPDATE users SET email_verified_at = $1, verify_token_hash = NULL, verify_expires_at = NULL
             WHERE id = $2",
        )
        .bind(now())
        .bind(&id)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        self.account(&id).await
    }

    /// One account, as an admin sees it.
    pub async fn account(&self, user: &str) -> ApiResult<Option<AccountRow>> {
        let row = sqlx::query(&format!(
            "SELECT {ACCOUNT_COLUMNS} FROM users u WHERE u.id = $2"
        ))
        .bind(now())
        .bind(user)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(|r| account_from(&r)).transpose()?)
    }

    /// Accounts whose username or email contains `search`, optionally with one status: a page of them, and how many
    /// match in all. Pending accounts come first, then the newest.
    pub async fn accounts(
        &self,
        search: &str,
        status: Option<Status>,
        limit: i64,
        offset: i64,
    ) -> ApiResult<(Vec<AccountRow>, i64)> {
        let pattern = format!(
            "%{}%",
            search
                .trim()
                .to_lowercase()
                .replace('\\', "\\\\")
                .replace('%', "\\%")
                .replace('_', "\\_")
        );
        let status = status.map_or("", Status::as_str);
        let filter = "(lower(u.username) LIKE $2 ESCAPE '\\' OR lower(COALESCE(u.email, '')) LIKE $2 ESCAPE '\\')
             AND ($3 = '' OR u.status = $3)";
        let rows = sqlx::query(&format!(
            "SELECT {ACCOUNT_COLUMNS} FROM users u WHERE {filter}
             ORDER BY CASE WHEN u.status = 'pending' THEN 0 ELSE 1 END, u.created_at DESC, u.id
             LIMIT $4 OFFSET $5"
        ))
        .bind(now())
        .bind(&pattern)
        .bind(status)
        .bind(limit)
        .bind(offset)
        .fetch_all(&self.pool)
        .await?;
        // The same filter with the placeholders it needs renumbered from 1.
        let total: i64 = sqlx::query(&format!(
            "SELECT COUNT(*) FROM users u WHERE {}",
            filter.replace("$2", "$1").replace("$3", "$2")
        ))
        .bind(&pattern)
        .bind(status)
        .fetch_one(&self.pool)
        .await?
        .try_get(0)?;
        let rows = rows.iter().map(account_from).collect::<sqlx::Result<_>>()?;
        Ok((rows, total))
    }

    /// Change an account's status; `false` when it is the last active admin (or doesn't exist) and the change would
    /// take them away.
    pub async fn set_status(&self, user: &str, status: Status) -> ApiResult<bool> {
        let guard = if status == Status::Active {
            "1 = 1".to_string()
        } else {
            not_last_admin("$2")
        };
        let done = sqlx::query(&format!(
            "UPDATE users SET status = $1, verify_token_hash = NULL, verify_expires_at = NULL
             WHERE id = $2 AND {guard}"
        ))
        .bind(status.as_str())
        .bind(user)
        .execute(&self.pool)
        .await?;
        Ok(done.rows_affected() > 0)
    }

    /// Make an account an admin or not; `false` when that would leave no active admin (or it doesn't exist).
    pub async fn set_admin(&self, user: &str, admin: bool) -> ApiResult<bool> {
        let guard = if admin {
            "1 = 1".to_string()
        } else {
            not_last_admin("$2")
        };
        let done = sqlx::query(&format!(
            "UPDATE users SET is_admin = $1 WHERE id = $2 AND {guard}"
        ))
        .bind(flag(admin))
        .bind(user)
        .execute(&self.pool)
        .await?;
        Ok(done.rows_affected() > 0)
    }

    /// Replace a password hash, and say whether it has to be changed at the next sign-in.
    pub async fn set_password(
        &self,
        user: &str,
        hash: Option<&str>,
        must_change: bool,
    ) -> ApiResult<bool> {
        let done = sqlx::query(
            "UPDATE users SET password_hash = COALESCE($1, password_hash), must_change_password = $2
             WHERE id = $3",
        )
        .bind(hash.map(str::to_string))
        .bind(flag(must_change))
        .bind(user)
        .execute(&self.pool)
        .await?;
        Ok(done.rows_affected() > 0)
    }

    /// Set an account's email address; `false` when another account has it.
    pub async fn set_email(&self, user: &str, email: Option<&str>) -> ApiResult<bool> {
        let mut tx = self.pool.begin().await?;
        if let Some(email) = email {
            let taken =
                sqlx::query("SELECT 1 FROM users WHERE lower(email) = lower($1) AND id <> $2")
                    .bind(email)
                    .bind(user)
                    .fetch_optional(&mut *tx)
                    .await?;
            if taken.is_some() {
                return Ok(false);
            }
        }
        sqlx::query("UPDATE users SET email = $1 WHERE id = $2")
            .bind(email.map(str::to_string))
            .bind(user)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(true)
    }

    /// Delete an account with its sessions; `false` when it is the last active admin or doesn't exist. Its data
    /// folder is the caller's to remove.
    pub async fn delete_user(&self, user: &str) -> ApiResult<bool> {
        let done = sqlx::query(&format!(
            "DELETE FROM users WHERE id = $1 AND {}",
            not_last_admin("$1")
        ))
        .bind(user)
        .execute(&self.pool)
        .await?;
        Ok(done.rows_affected() > 0)
    }

    /// How many active admins there are.
    pub async fn active_admins(&self) -> ApiResult<i64> {
        Ok(
            sqlx::query("SELECT COUNT(*) FROM users WHERE is_admin = 1 AND status = 'active'")
                .fetch_one(&self.pool)
                .await?
                .try_get(0)?,
        )
    }

    /// Where active admins with an address get email.
    pub async fn admin_emails(&self) -> ApiResult<Vec<String>> {
        let rows = sqlx::query(
            "SELECT email FROM users WHERE is_admin = 1 AND status = 'active' AND email IS NOT NULL",
        )
        .fetch_all(&self.pool)
        .await?;
        rows.iter()
            .map(|r| Ok(r.try_get(0)?))
            .collect::<ApiResult<_>>()
    }

    // Admin page.

    /// Counts for the statistics.
    pub async fn counts(&self) -> ApiResult<Counts> {
        let accounts = sqlx::query("SELECT status, COUNT(*) FROM users GROUP BY status")
            .fetch_all(&self.pool)
            .await?
            .iter()
            .map(|r| Ok((r.try_get(0)?, r.try_get(1)?)))
            .collect::<sqlx::Result<_>>()?;
        Ok(Counts {
            accounts,
            admins: sqlx::query("SELECT COUNT(*) FROM users WHERE is_admin = 1")
                .fetch_one(&self.pool)
                .await?
                .try_get(0)?,
            sessions: sqlx::query("SELECT COUNT(*) FROM sessions WHERE expires_at > $1")
                .bind(now())
                .fetch_one(&self.pool)
                .await?
                .try_get(0)?,
        })
    }

    /// A setting saved on the admin page.
    pub async fn setting(&self, key: &str) -> ApiResult<Option<String>> {
        let row = sqlx::query("SELECT value FROM server_settings WHERE key = $1")
            .bind(key)
            .fetch_optional(&self.pool)
            .await?;
        Ok(row.map(|r| r.try_get(0)).transpose()?)
    }

    /// Save a setting from the admin page.
    pub async fn set_setting(&self, key: &str, value: &str) -> ApiResult<()> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("DELETE FROM server_settings WHERE key = $1")
            .bind(key)
            .execute(&mut *tx)
            .await?;
        sqlx::query("INSERT INTO server_settings (key, value) VALUES ($1, $2)")
            .bind(key)
            .bind(value)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }

    /// Forget a setting from the admin page, so `config.toml` applies again.
    pub async fn delete_setting(&self, key: &str) -> ApiResult<()> {
        sqlx::query("DELETE FROM server_settings WHERE key = $1")
            .bind(key)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Add to the activity log, and prune it to [`ACTIVITY_KEPT`] rows.
    pub async fn log(&self, entry: &Activity) -> ApiResult<()> {
        // Version 7 ids sort by time, so the newest rows have the largest ids.
        let id = uuid::Uuid::now_v7().to_string();
        sqlx::query(
            "INSERT INTO activity (id, at, kind, actor_id, actor_name, target_id, target_name, detail, ip)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
        )
        .bind(&id)
        .bind(entry.at)
        .bind(&entry.kind)
        .bind(entry.actor_id.clone())
        .bind(entry.actor_name.clone())
        .bind(entry.target_id.clone())
        .bind(entry.target_name.clone())
        .bind(entry.detail.clone())
        .bind(entry.ip.clone())
        .execute(&self.pool)
        .await?;
        sqlx::query(
            "DELETE FROM activity WHERE id <= (SELECT id FROM activity ORDER BY id DESC LIMIT 1 OFFSET $1)",
        )
        .bind(ACTIVITY_KEPT)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// A page of the activity log, newest first, and how many entries there are.
    pub async fn activity(&self, limit: i64, offset: i64) -> ApiResult<(Vec<Activity>, i64)> {
        let rows = sqlx::query(
            "SELECT at, kind, actor_id, actor_name, target_id, target_name, detail, ip FROM activity
             ORDER BY id DESC LIMIT $1 OFFSET $2",
        )
        .bind(limit)
        .bind(offset)
        .fetch_all(&self.pool)
        .await?;
        let total: i64 = sqlx::query("SELECT COUNT(*) FROM activity")
            .fetch_one(&self.pool)
            .await?
            .try_get(0)?;
        let rows = rows
            .iter()
            .map(|r| {
                Ok(Activity {
                    at: r.try_get(0)?,
                    kind: r.try_get(1)?,
                    actor_id: r.try_get(2)?,
                    actor_name: r.try_get(3)?,
                    target_id: r.try_get(4)?,
                    target_name: r.try_get(5)?,
                    detail: r.try_get(6)?,
                    ip: r.try_get(7)?,
                })
            })
            .collect::<sqlx::Result<_>>()?;
        Ok((rows, total))
    }
}

#[cfg(test)]
mod marker_tests {
    use super::{MARKERS, MIGRATIONS};

    #[test]
    fn every_step_from_2_has_a_marker_it_creates_and_can_run_twice() {
        let steps: Vec<usize> = MARKERS.iter().map(|(step, _)| *step).collect();
        let want: Vec<usize> = (2..=MIGRATIONS.len()).collect();
        assert_eq!(steps, want, "one marker per step from 2 on, in order");
        for (step, table) in MARKERS {
            let sql = MIGRATIONS[step - 1];
            let creates = format!("CREATE TABLE IF NOT EXISTS {table} (");
            assert!(
                sql.iter().any(|s| s.starts_with(&creates)),
                "step {step} does not create {table}"
            );
            for statement in sql {
                let ok = [
                    "CREATE TABLE IF NOT EXISTS ",
                    "CREATE INDEX IF NOT EXISTS ",
                    "CREATE UNIQUE INDEX IF NOT EXISTS ",
                ]
                .iter()
                .any(|p| statement.starts_with(p));
                assert!(
                    ok,
                    "step {step} has a statement that can't run twice: {statement}"
                );
            }
        }
    }
}
