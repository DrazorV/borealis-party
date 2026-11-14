//! `party://content-changed`: the account's content (packs, index files, 5etools addresses) changed, so every open
//! Additional Content page of that account, on any tab or device, fetches its lists again.
//!
//! Upstream's `borealis://content-reloaded` comes only from a content reload, which adding or removing an index file
//! does not do, and it comes mid-command, before the command's other changes (a removed index, a job's result) are
//! there to read. This event comes after the change is complete: after the command answered, after a background job
//! ended (`job_start`), and after a bulk removal. It goes through the party hub ([`crate::hub::PartyHub::deliver`]),
//! so it reaches the account's own streams only, carries an event id, and a stream that missed it gets
//! `party://resync`.
//!
//! Payload: `{ account, kind, command }`: `account` is the receiving account's own id, `kind` what changed (`packs`,
//! `indexes` or `fivetools`; a client fetches all its content lists again whatever the kind), and `command` the
//! command that changed it (`remove_packs` for a bulk removal, `add_share_links` for a share's index files).

use serde_json::json;

use crate::hub::PartyHub;

/// The event name.
pub const CONTENT_CHANGED: &str = "party://content-changed";

/// What a command changes, when it changes the account's content. Reads and failed commands send nothing.
pub fn kind_of(command: &str) -> Option<&'static str> {
    Some(match command {
        "add_index" | "remove_index" | "update_indexes" | "clear_downloaded_content" => "indexes",
        "remove_5etools_url" | "update_5etools_url" | "import_5etools" => "fivetools",
        "import_pack"
        | "install_import_preview"
        | "set_pack_enabled"
        | "remove_pack"
        | "set_source_edition"
        | "bundle_accept" => "packs",
        _ => return None,
    })
}

/// Tell `account`'s open streams that `command` changed its content.
pub fn changed(hub: &PartyHub, account: &str, kind: &str, command: &str) {
    // Other spells now: the copies hidden from new picks are worked out again on the account's next command.
    crate::spells::mark_stale(account);
    hub.deliver(
        account,
        CONTENT_CHANGED,
        &json!({ "account": account, "kind": kind, "command": command }),
    );
}

#[cfg(test)]
mod tests {
    use super::kind_of;

    #[test]
    fn every_content_command_has_a_kind_and_reads_have_none() {
        for command in crate::parties::CONTENT_COMMANDS {
            if command == "recheck_missing_content" {
                // Rechecks characters against the content; the content itself stays.
                assert_eq!(kind_of(command), None);
            } else {
                assert!(kind_of(command).is_some(), "{command}");
            }
        }
        for read in [
            "list_packs",
            "list_indexes",
            "list_5etools_urls",
            "list_sources",
            "preview_import_pack",
        ] {
            assert_eq!(kind_of(read), None, "{read}");
        }
        assert_eq!(kind_of("add_index"), Some("indexes"));
    }
}
