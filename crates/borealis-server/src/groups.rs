//! Groups, invitations and the party view.

use axum::extract::{Path, State};
use axum::Json;
use borealis_lib::headless::Access;
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use crate::auth::{Role, User};
use crate::db::now;
use crate::error::{ApiError, ApiResult};
use crate::rpc::group_role;
use crate::workspaces::{Workspace, WsKey};
use crate::AppState;

/// Read commands that make no sense against someone else's workspace.
const VIEW_REFUSED: [&str; 3] = ["list_characters", "close_character", "save_settings"];

fn require_dm(user: &User) -> ApiResult<()> {
    if user.role == Role::Dm {
        Ok(())
    } else {
        Err(ApiError::Forbidden(
            "Only Dungeon Masters can do that.".into(),
        ))
    }
}

fn require_player(user: &User) -> ApiResult<()> {
    if user.role == Role::Player {
        Ok(())
    } else {
        Err(ApiError::Forbidden("Only players can do that.".into()))
    }
}

fn group_name(name: &str) -> ApiResult<String> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > 80 {
        return Err(ApiError::Invalid(
            "Group names are 1 to 80 characters.".into(),
        ));
    }
    Ok(name.to_string())
}

/// A group in a list.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GroupOut {
    id: String,
    name: String,
    dm_name: String,
    member_count: i64,
    character_id: Option<String>,
}

/// The name in a form.
#[derive(Deserialize)]
pub struct NameForm {
    name: String,
}

/// `GET /api/groups`.
pub async fn list(State(state): State<AppState>, user: User) -> ApiResult<Json<Vec<GroupOut>>> {
    let groups = state
        .db
        .run(move |c| {
            let sql = "SELECT g.id, g.name, d.username,
                    (SELECT COUNT(*) FROM members m WHERE m.group_id = g.id),
                    (SELECT m.character_id FROM members m WHERE m.group_id = g.id AND m.player_id = ?1)
                 FROM groups g JOIN users d ON d.id = g.dm_id
                 WHERE g.dm_id = ?1 OR EXISTS (SELECT 1 FROM members m WHERE m.group_id = g.id AND m.player_id = ?1)
                 ORDER BY g.name COLLATE NOCASE";
            let mut stmt = c.prepare(sql)?;
            let rows = stmt.query_map(params![user.id], |r| {
                Ok(GroupOut {
                    id: r.get(0)?,
                    name: r.get(1)?,
                    dm_name: r.get(2)?,
                    member_count: r.get(3)?,
                    character_id: r.get(4)?,
                })
            })?;
            Ok(rows.collect::<Result<Vec<_>, _>>()?)
        })
        .await?;
    Ok(Json(groups))
}

/// `POST /api/groups`.
pub async fn create(
    State(state): State<AppState>,
    user: User,
    Json(form): Json<NameForm>,
) -> ApiResult<Json<Value>> {
    require_dm(&user)?;
    let name = group_name(&form.name)?;
    let id = uuid::Uuid::new_v4().to_string();
    let out = json!({ "id": id, "name": name });
    state
        .db
        .run(move |c| {
            c.execute(
                "INSERT INTO groups (id, name, dm_id, created_at) VALUES (?1, ?2, ?3, ?4)",
                params![id, name, user.id, now()],
            )?;
            Ok(())
        })
        .await?;
    Ok(Json(out))
}

async fn owned_group(state: &AppState, user: &User, group: &str) -> ApiResult<()> {
    require_dm(user)?;
    group_role(&state.db, user, group)
        .await?
        .map(|_| ())
        .ok_or_else(ApiError::group_not_found)
}

/// `PATCH /api/groups/{id}`.
pub async fn rename(
    State(state): State<AppState>,
    user: User,
    Path(group): Path<String>,
    Json(form): Json<NameForm>,
) -> ApiResult<()> {
    owned_group(&state, &user, &group).await?;
    let name = group_name(&form.name)?;
    state
        .db
        .run(move |c| {
            c.execute(
                "UPDATE groups SET name = ?1 WHERE id = ?2",
                params![name, group],
            )?;
            Ok(())
        })
        .await
}

#[derive(Clone)]
struct Member {
    player: String,
    character: Option<String>,
}

async fn members_of(state: &AppState, group: &str) -> ApiResult<Vec<Member>> {
    let group = group.to_string();
    state
        .db
        .run(move |c| {
            let mut stmt =
                c.prepare("SELECT player_id, character_id FROM members WHERE group_id = ?1")?;
            let rows = stmt.query_map(params![group], |r| {
                Ok(Member {
                    player: r.get(0)?,
                    character: r.get(1)?,
                })
            })?;
            Ok(rows.collect::<Result<Vec<_>, _>>()?)
        })
        .await
}

/// Give a member's character back to their personal list.
async fn release_character(
    state: &AppState,
    group: &str,
    dm: &str,
    player: &str,
    character: &str,
) -> ApiResult<()> {
    let from = WsKey::GroupMember {
        group: group.to_string(),
        dm: dm.to_string(),
        player: player.to_string(),
    };
    let to = WsKey::Personal {
        player: player.to_string(),
    };
    state.workspaces.move_character(&from, &to, character).await
}

/// `DELETE /api/groups/{id}`: characters go back to their owners first.
pub async fn delete(
    State(state): State<AppState>,
    user: User,
    Path(group): Path<String>,
) -> ApiResult<()> {
    owned_group(&state, &user, &group).await?;
    let _guard = state.membership.lock().await;
    for member in members_of(&state, &group).await? {
        if let Some(character) = member.character {
            release_character(&state, &group, &user.id, &member.player, &character).await?;
        }
    }
    let id = group.clone();
    state
        .db
        .run(move |c| {
            c.execute("DELETE FROM groups WHERE id = ?1", params![id])?;
            Ok(())
        })
        .await?;
    state.workspaces.remove_group(&group).await
}

/// `GET /api/groups/{id}`: members and, for the DM, pending invitations.
pub async fn detail(
    State(state): State<AppState>,
    user: User,
    Path(group): Path<String>,
) -> ApiResult<Json<Value>> {
    let role = group_role(&state.db, &user, &group)
        .await?
        .ok_or_else(ApiError::group_not_found)?;
    let is_dm = user.role == Role::Dm;
    let out = state
        .db
        .run(move |c| {
            let (name, dm_name): (String, String) = c.query_row(
                "SELECT g.name, d.username FROM groups g JOIN users d ON d.id = g.dm_id WHERE g.id = ?1",
                params![group],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )?;
            let mut stmt = c.prepare(
                "SELECT u.id, u.username, m.character_id IS NOT NULL FROM members m JOIN users u ON u.id = m.player_id
                 WHERE m.group_id = ?1 ORDER BY u.username COLLATE NOCASE",
            )?;
            let members = stmt
                .query_map(params![group], |r| {
                    Ok(json!({
                        "playerId": r.get::<_, String>(0)?,
                        "username": r.get::<_, String>(1)?,
                        "hasCharacter": r.get::<_, bool>(2)?,
                    }))
                })?
                .collect::<Result<Vec<_>, _>>()?;
            let invites = if is_dm {
                let mut stmt = c.prepare(
                    "SELECT i.id, u.username FROM invites i JOIN users u ON u.id = i.player_id
                     WHERE i.group_id = ?1 ORDER BY u.username COLLATE NOCASE",
                )?;
                let rows = stmt.query_map(params![group], |r| {
                    Ok(json!({ "id": r.get::<_, String>(0)?, "username": r.get::<_, String>(1)? }))
                })?;
                rows.collect::<Result<Vec<_>, _>>()?
            } else {
                Vec::new()
            };
            Ok(json!({ "id": group, "name": name, "dmName": dm_name, "members": members, "invites": invites }))
        })
        .await?;
    let mut out = out;
    if let Some(character) = role.character {
        out["characterId"] = json!(character);
    }
    Ok(Json(out))
}

/// The invitation form.
#[derive(Deserialize)]
pub struct InviteForm {
    username: String,
}

/// `POST /api/groups/{id}/invites`.
pub async fn invite(
    State(state): State<AppState>,
    user: User,
    Path(group): Path<String>,
    Json(form): Json<InviteForm>,
) -> ApiResult<Json<Value>> {
    owned_group(&state, &user, &group).await?;
    let username = form.username.trim().to_string();
    let id = uuid::Uuid::new_v4().to_string();
    let out = json!({ "id": id, "username": username });
    state
        .db
        .run(move |c| {
            let tx = c.transaction()?;
            let player: Option<(String, String)> = tx
                .query_row(
                    "SELECT id, role FROM users WHERE username = ?1",
                    params![username],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()?;
            let Some((player, role)) = player else {
                return Err(ApiError::NotFound("There is no account with that username.".into()));
            };
            if role != "player" {
                return Err(ApiError::Invalid("Only player accounts can join a group.".into()));
            }
            let member: bool = tx.query_row(
                "SELECT EXISTS (SELECT 1 FROM members WHERE group_id = ?1 AND player_id = ?2)",
                params![group, player],
                |r| r.get(0),
            )?;
            if member {
                return Err(ApiError::Conflict("That player is already in this group.".into()));
            }
            let added = tx.execute(
                "INSERT OR IGNORE INTO invites (id, group_id, player_id, created_at) VALUES (?1, ?2, ?3, ?4)",
                params![id, group, player, now()],
            )?;
            if added == 0 {
                return Err(ApiError::Conflict("That player has already been invited.".into()));
            }
            tx.commit()?;
            Ok(())
        })
        .await?;
    Ok(Json(out))
}

/// `DELETE /api/groups/{id}/invites/{invite}`.
pub async fn cancel_invite(
    State(state): State<AppState>,
    user: User,
    Path((group, invite)): Path<(String, String)>,
) -> ApiResult<()> {
    owned_group(&state, &user, &group).await?;
    state
        .db
        .run(move |c| {
            c.execute(
                "DELETE FROM invites WHERE id = ?1 AND group_id = ?2",
                params![invite, group],
            )?;
            Ok(())
        })
        .await
}

/// `GET /api/invites`: a player's pending invitations.
pub async fn my_invites(State(state): State<AppState>, user: User) -> ApiResult<Json<Vec<Value>>> {
    let invites = state
        .db
        .run(move |c| {
            let mut stmt = c.prepare(
                "SELECT i.id, g.name, d.username FROM invites i
                 JOIN groups g ON g.id = i.group_id JOIN users d ON d.id = g.dm_id
                 WHERE i.player_id = ?1 ORDER BY i.created_at",
            )?;
            let rows = stmt.query_map(params![user.id], |r| {
                Ok(json!({
                    "id": r.get::<_, String>(0)?,
                    "groupName": r.get::<_, String>(1)?,
                    "dmName": r.get::<_, String>(2)?,
                }))
            })?;
            Ok(rows.collect::<Result<Vec<_>, _>>()?)
        })
        .await?;
    Ok(Json(invites))
}

/// `POST /api/invites/{id}/accept`: join; the character comes after, from `set_character` or by building one there.
pub async fn accept(
    State(state): State<AppState>,
    user: User,
    Path(invite): Path<String>,
) -> ApiResult<Json<Value>> {
    require_player(&user)?;
    let group = state
        .db
        .run(move |c| {
            let tx = c.transaction()?;
            let group: Option<String> = tx
                .query_row(
                    "SELECT group_id FROM invites WHERE id = ?1 AND player_id = ?2",
                    params![invite, user.id],
                    |r| r.get(0),
                )
                .optional()?;
            let Some(group) = group else {
                return Err(ApiError::NotFound("That invitation no longer exists.".into()));
            };
            tx.execute("DELETE FROM invites WHERE id = ?1", params![invite])?;
            tx.execute(
                "INSERT OR IGNORE INTO members (group_id, player_id, character_id, joined_at) VALUES (?1, ?2, NULL, ?3)",
                params![group, user.id, now()],
            )?;
            tx.commit()?;
            Ok(group)
        })
        .await?;
    Ok(Json(json!({ "groupId": group })))
}

/// `POST /api/invites/{id}/decline`.
pub async fn decline(
    State(state): State<AppState>,
    user: User,
    Path(invite): Path<String>,
) -> ApiResult<()> {
    state
        .db
        .run(move |c| {
            c.execute(
                "DELETE FROM invites WHERE id = ?1 AND player_id = ?2",
                params![invite, user.id],
            )?;
            Ok(())
        })
        .await
}

/// `DELETE /api/groups/{id}/members/{player}`: the DM removes someone, or a player leaves.
pub async fn remove_member(
    State(state): State<AppState>,
    user: User,
    Path((group, player)): Path<(String, String)>,
) -> ApiResult<()> {
    let role = group_role(&state.db, &user, &group)
        .await?
        .ok_or_else(ApiError::group_not_found)?;
    if user.role == Role::Player && player != user.id {
        return Err(ApiError::Forbidden(
            "You can only remove yourself from a group.".into(),
        ));
    }
    let _guard = state.membership.lock().await;
    let member = members_of(&state, &group)
        .await?
        .into_iter()
        .find(|m| m.player == player)
        .ok_or_else(|| ApiError::NotFound("That player is not in this group.".into()))?;
    if let Some(character) = member.character {
        release_character(&state, &group, &role.dm, &player, &character).await?;
    }
    let (g, p) = (group.clone(), player.clone());
    state
        .db
        .run(move |c| {
            c.execute(
                "DELETE FROM members WHERE group_id = ?1 AND player_id = ?2",
                params![g, p],
            )?;
            Ok(())
        })
        .await?;
    state.workspaces.forget(&WsKey::GroupMember {
        group,
        dm: role.dm,
        player,
    });
    Ok(())
}

/// The character form.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CharacterForm {
    character_id: String,
}

/// `POST /api/groups/{id}/character`: bring one of the player's personal characters into the group.
pub async fn set_character(
    State(state): State<AppState>,
    user: User,
    Path(group): Path<String>,
    Json(form): Json<CharacterForm>,
) -> ApiResult<()> {
    require_player(&user)?;
    let _guard = state.membership.lock().await;
    let role = group_role(&state.db, &user, &group)
        .await?
        .ok_or_else(ApiError::group_not_found)?;
    if role.character.is_some() {
        return Err(ApiError::Conflict(
            "You already have a character in this group.".into(),
        ));
    }
    let from = WsKey::Personal {
        player: user.id.clone(),
    };
    let to = WsKey::GroupMember {
        group: group.clone(),
        dm: role.dm,
        player: user.id.clone(),
    };
    state
        .workspaces
        .move_character(&from, &to, &form.character_id)
        .await?;
    let character = form.character_id;
    let saved = {
        let (g, u, ch) = (group, user.id, character.clone());
        state
            .db
            .run(move |c| {
                c.execute(
                    "UPDATE members SET character_id = ?1 WHERE group_id = ?2 AND player_id = ?3",
                    params![ch, g, u],
                )?;
                Ok(())
            })
            .await
    };
    if let Err(e) = saved {
        // The record failed: put the file back so the character isn't stranded.
        state
            .workspaces
            .move_character(&to, &from, &character)
            .await?;
        return Err(e);
    }
    state.workspaces.get(&to).await?;
    Ok(())
}

/// `DELETE /api/groups/{id}/character`: take the character out, staying in the group.
pub async fn clear_character(
    State(state): State<AppState>,
    user: User,
    Path(group): Path<String>,
) -> ApiResult<()> {
    require_player(&user)?;
    let _guard = state.membership.lock().await;
    let role = group_role(&state.db, &user, &group)
        .await?
        .ok_or_else(ApiError::group_not_found)?;
    let Some(character) = role.character else {
        return Ok(());
    };
    release_character(&state, &group, &role.dm, &user.id, &character).await?;
    state
        .db
        .run(move |c| {
            c.execute(
                "UPDATE members SET character_id = NULL WHERE group_id = ?1 AND player_id = ?2",
                params![group, user.id],
            )?;
            Ok(())
        })
        .await
}

/// `GET /api/characters`: everything a player owns, and where it is.
pub async fn my_characters(
    State(state): State<AppState>,
    user: User,
) -> ApiResult<Json<Vec<Value>>> {
    require_player(&user)?;
    let player = user.id.clone();
    let groups = state
        .db
        .run(move |c| {
            let mut stmt = c.prepare(
                "SELECT g.id, g.name, g.dm_id FROM members m JOIN groups g ON g.id = m.group_id
                 WHERE m.player_id = ?1 AND m.character_id IS NOT NULL",
            )?;
            let rows = stmt.query_map(params![player], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                ))
            })?;
            Ok(rows.collect::<Result<Vec<_>, _>>()?)
        })
        .await?;
    let mut out = Vec::new();
    let personal = state
        .workspaces
        .get(&WsKey::Personal {
            player: user.id.clone(),
        })
        .await?;
    push_characters(&mut out, &personal, "personal", None).await?;
    for (group, name, dm) in groups {
        let workspace = state
            .workspaces
            .get(&WsKey::GroupMember {
                group: group.clone(),
                dm,
                player: user.id.clone(),
            })
            .await?;
        push_characters(&mut out, &workspace, &format!("group-{group}"), Some(&name)).await?;
    }
    Ok(Json(out))
}

async fn push_characters(
    out: &mut Vec<Value>,
    workspace: &Workspace,
    scope: &str,
    group_name: Option<&str>,
) -> ApiResult<()> {
    let list = workspace.call("list_characters", Map::new()).await?;
    for mut character in list.as_array().cloned().unwrap_or_default() {
        if let Some(fields) = character.as_object_mut() {
            fields.remove("path");
            fields.insert("scope".into(), json!(scope));
            fields.insert("groupName".into(), json!(group_name));
        }
        out.push(character);
    }
    Ok(())
}

async fn member_workspace(
    state: &AppState,
    user: &User,
    group: &str,
    player: &str,
) -> ApiResult<(std::sync::Arc<Workspace>, String)> {
    owned_group(state, user, group).await?;
    let character = members_of(state, group)
        .await?
        .into_iter()
        .find(|m| m.player == player)
        .and_then(|m| m.character)
        .ok_or_else(|| ApiError::NotFound("That player has no character here.".into()))?;
    let workspace = state
        .workspaces
        .get(&WsKey::GroupMember {
            group: group.to_string(),
            dm: user.id.clone(),
            player: player.to_string(),
        })
        .await?;
    Ok((workspace, character))
}

/// The snapshot of a character, opened if its player hasn't got it open.
async fn snapshot_of(workspace: &Workspace, character: &str) -> ApiResult<Value> {
    let mut args = Map::new();
    args.insert("characterId".into(), json!(character));
    workspace.call("load_character", args).await
}

fn party_row(username: &str, player: &str, snapshot: &Value) -> Value {
    json!({
        "playerId": player,
        "username": username,
        "characterId": snapshot["id"],
        "name": snapshot["name"],
        "ruleset": snapshot["ruleset"],
        "portrait": snapshot["portrait"],
        "level": snapshot["level"],
        "stats": {
            "armorClass": snapshot["stats"]["armorClass"],
            "maxHp": snapshot["stats"]["hpMax"],
            "initiative": snapshot["stats"]["initiative"],
            "speed": snapshot["stats"]["speed"],
            "passivePerception": snapshot["stats"]["passivePerception"],
            "proficiencyBonus": snapshot["stats"]["proficiencyBonus"],
        },
        "hitPoints": snapshot["play"]["hitPoints"],
        "progress": snapshot["progress"],
        "notices": snapshot["notices"],
    })
}

/// `GET /api/groups/{id}/party`: where every character stands, for the DM.
pub async fn party(
    State(state): State<AppState>,
    user: User,
    Path(group): Path<String>,
) -> ApiResult<Json<Vec<Value>>> {
    owned_group(&state, &user, &group).await?;
    let id = group.clone();
    let names = state
        .db
        .run(move |c| {
            let mut stmt = c.prepare(
                "SELECT u.id, u.username, m.character_id FROM members m JOIN users u ON u.id = m.player_id
                 WHERE m.group_id = ?1 ORDER BY u.username COLLATE NOCASE",
            )?;
            let rows = stmt.query_map(params![id], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, Option<String>>(2)?,
                ))
            })?;
            Ok(rows.collect::<Result<Vec<_>, _>>()?)
        })
        .await?;
    let mut rows = Vec::with_capacity(names.len());
    for (player, username, character) in names {
        let Some(character) = character else {
            rows.push(json!({ "playerId": player, "username": username, "characterId": null }));
            continue;
        };
        let workspace = state
            .workspaces
            .get(&WsKey::GroupMember {
                group: group.clone(),
                dm: user.id.clone(),
                player: player.clone(),
            })
            .await?;
        match snapshot_of(&workspace, &character).await {
            Ok(snapshot) => rows.push(party_row(&username, &player, &snapshot)),
            Err(e) => {
                tracing::warn!("party: character {character} unreadable: {e:?}");
                rows.push(json!({
                    "playerId": player, "username": username, "characterId": character, "unreadable": true,
                }));
            }
        }
    }
    Ok(Json(rows))
}

/// `POST /api/groups/{id}/members/{player}/view/{command}`: look at a member's character. Only read commands are
/// reachable, and the character id always comes from the membership row.
pub async fn view(
    State(state): State<AppState>,
    user: User,
    Path((group, player, command)): Path<(String, String, String)>,
    Json(mut args): Json<Map<String, Value>>,
) -> ApiResult<Json<Value>> {
    let (workspace, character) = member_workspace(&state, &user, &group, &player).await?;
    if workspace.access_of(&command) != Some(Access::Read)
        || VIEW_REFUSED.contains(&command.as_str())
    {
        return Err(ApiError::Forbidden(
            "Dungeon Masters can look at characters but not change them.".into(),
        ));
    }
    snapshot_of(&workspace, &character).await?;
    args.insert("characterId".into(), json!(character));
    let mut result = workspace.call(&command, args).await?;
    crate::rpc::scrub_paths(&mut result);
    Ok(Json(result))
}
