//! Windows and Linux "Open with" / double-click: what a launch's arguments put in the opened-files queue
//! (`content_preview::opened_arg_paths` and `push_opened_file`), as a second launch hands them to the
//! running app (`commands_content::handle_second_instance`).

use std::path::Path;

use borealis_lib::content_preview::{opened_arg_paths, push_opened_file, take_opened_files};

#[test]
fn arguments_skip_the_program_and_flags_and_resolve_against_cwd() {
    let cwd = std::env::temp_dir();
    let abs = cwd.join("abs.dnd5e");
    let argv = vec![
        "borealis.exe".to_owned(),
        "--flag".to_owned(),
        "relative.borealis".to_owned(),
        abs.to_string_lossy().into_owned(),
    ];
    assert_eq!(
        opened_arg_paths(&argv, &cwd),
        vec![cwd.join("relative.borealis"), abs]
    );
    assert!(opened_arg_paths(&argv[..1], Path::new(".")).is_empty());
}

// One test touches the process-wide queue, so no other test races it.
#[test]
fn opened_files_queue_content_and_skip_the_rest() {
    let dir = tempfile::tempdir().unwrap();
    let write = |name: &str, bytes: &[u8]| {
        let p = dir.path().join(name);
        std::fs::write(&p, bytes).unwrap();
        p
    };
    let character = write("Hero.dnd5e", b"<character/>");
    let bundle = write("Party.borealis", b"PK\x03\x04bundle");
    let other = write("notes.txt", b"hello");
    let empty = write("empty.xml", b"");

    let _ = take_opened_files();
    for p in [
        &character,
        &bundle,
        &other,
        &empty,
        &dir.path().join("missing.xml"),
    ] {
        push_opened_file(p);
    }
    let names: Vec<String> = take_opened_files()
        .into_iter()
        .map(|f| f.file_name)
        .collect();
    // Without device-link (the party apps) a bundle waits with the other files; with it, the bundle
    // inbox takes it by path instead.
    if cfg!(feature = "device-link") {
        assert_eq!(names, ["Hero.dnd5e"]);
    } else {
        assert_eq!(names, ["Hero.dnd5e", "Party.borealis"]);
    }
    assert!(take_opened_files().is_empty());
}
