use super::*;
use std::io::Write;

fn file(id: u64, name: &str, revision: &str) -> Value {
    serde_json::json!({"id":id,"name":name,"type":"zip","version":"2.0","version_type":"release","updated_at":revision,"download_url":format!("https://example.invalid/{id}.zip")})
}

fn entry(id: &str) -> Entry {
    serde_json::from_value(serde_json::json!({"package":{"id":id,"name":"Fixture","owner":"","description":"","version":"1.0","page":"https://example.invalid","icon":"","downloads":0,"deprecated":false,"categories":[]},"folders":["oldfolder"],"unsafe_mod":false,"companion":null})).unwrap()
}

fn cache_json(manager: &Manager, url: &str, value: &Value) {
    crate::storage::write_json(
        &manager
            .cache
            .join("providers")
            .join(format!("{}.json", launch::hash(url.as_bytes()))),
        value,
    )
    .unwrap();
}

fn archive(manager: &Manager, url: &str, revision: &str, files: &[(&str, &[u8])]) {
    let key = if revision.is_empty() {
        url.to_owned()
    } else {
        format!("{url}\n{revision}")
    };
    let path = manager
        .cache
        .join("downloads")
        .join(launch::hash(key.as_bytes()));
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut zip = zip::ZipWriter::new(fs::File::create(path).unwrap());
    for (name, bytes) in files {
        zip.start_file(*name, zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(bytes).unwrap();
    }
    zip.finish().unwrap();
}

#[test]
fn modworkshop_keeps_the_selected_variant_and_handles_old_ambiguous_imports_conservatively() {
    let a = file(1, "main", "2026-01-01T00:00:00Z");
    let b = file(2, "optional", "2026-02-01T00:00:00Z");
    let mut old = entry("modworkshop:12");
    old.source_file = Some(identity(&a).unwrap());
    let newer = file(3, "main", "2026-03-01T00:00:00Z");
    let files = vec![a.clone(), b.clone(), newer.clone()];
    assert_eq!(select_file(&old, &files).unwrap()["id"], 3);
    old.source_file = None;
    assert!(select_file(&old, &files).is_err());
    assert_eq!(select_file(&old, &[b.clone()]).unwrap()["id"], 2);
    old.source_file = Some(identity(&a).unwrap());
    assert!(select_file(&old, &[b]).is_err());
}

#[test]
fn recipe_updates_keep_the_pair_and_settings_and_failed_downloads_leave_the_original_revision() {
    let temp = tempfile::tempdir().unwrap();
    let manager = super::super::tests::manager(temp.path());
    let game = super::super::tests::game();
    let recipe = &recipes::for_game(&game).unwrap().mods[0];
    let host = if cfg!(target_os = "windows") {
        "windows"
    } else {
        "linux"
    };
    let tool = &recipe.companions[host];
    let release_url = format!(
        "https://api.github.com/repos/{}/releases/latest",
        recipe.repository
    );
    let mod_url = "https://example.invalid/newmod.zip";
    let proxy_url = "https://example.invalid/newproxy.zip";
    cache_json(
        &manager,
        &release_url,
        &serde_json::json!({"tag_name":"v2.0.0","assets":[{"name":recipe.mod_asset,"browser_download_url":mod_url},{"name":tool.asset,"browser_download_url":proxy_url}]}),
    );
    archive(
        &manager,
        mod_url,
        "",
        &[
            ("mod.xml", b"<Mod name=\"Entangled Worlds\"/>"),
            ("init.lua", b"updated mod"),
        ],
    );
    archive(
        &manager,
        proxy_url,
        "",
        &[(&tool.executable, b"updated companion, never executed")],
    );
    let root = manager.profile_root("noita", "raesir");
    let source = root.join("revisions/profile-fixture");
    fs::create_dir_all(source.join("mods/oldfolder")).unwrap();
    fs::write(source.join("mods/oldfolder/old.lua"), b"old mod").unwrap();
    crate::storage::write_json(
        &root.join("current.json"),
        &serde_json::json!({"revision":"profile-fixture"}),
    )
    .unwrap();
    let mut installed = entry("recipe:entangled-worlds");
    installed.package.version = "v1.0.0".into();
    crate::storage::write_json(&source.join(STATE), &vec![installed]).unwrap();
    fs::create_dir_all(root.join("runtime/recipe-entangled-worlds")).unwrap();
    fs::write(
        root.join("runtime/recipe-entangled-worlds/settings"),
        b"keep settings",
    )
    .unwrap();
    let mut refreshed = HashSet::from([release_url.clone()]);
    let plan = manager
        .noita_update_plan(&game, &source, &mut refreshed)
        .unwrap();
    assert!(!plan.is_empty());
    let revisions = source.parent().unwrap();
    let pending = tempfile::Builder::new()
        .prefix("pending-")
        .tempdir_in(revisions)
        .unwrap();
    super::super::super::install::copy_profile(&source, pending.path()).unwrap();
    let changed = plan
        .apply(&manager, &game, pending.path(), &AtomicBool::new(false))
        .unwrap();
    assert_eq!(changed.0, ["recipe:entangled-worlds"]);
    assert!(!pending.path().join("mods/oldfolder").exists());
    let records = read(pending.path()).unwrap();
    assert_eq!(records[0].package.version, "v2.0.0");
    assert_eq!(
        fs::read(
            pending
                .path()
                .join(&records[0].companion.as_ref().unwrap().executable)
        )
        .unwrap(),
        b"updated companion, never executed"
    );
    assert_eq!(
        fs::read(root.join("runtime/recipe-entangled-worlds/settings")).unwrap(),
        b"keep settings"
    );
    assert!(source.join("mods/oldfolder/old.lua").exists());
    assert_eq!(manager.own_path("noita").unwrap(), source);
    archive(
        &manager,
        proxy_url,
        "",
        &[("wrong-executable", b"broken replacement")],
    );
    let pending = tempfile::Builder::new()
        .prefix("pending-")
        .tempdir_in(revisions)
        .unwrap();
    super::super::super::install::copy_profile(&source, pending.path()).unwrap();
    assert!(
        manager
            .noita_update_plan(&game, &source, &mut refreshed)
            .unwrap()
            .apply(&manager, &game, pending.path(), &AtomicBool::new(false))
            .is_err()
    );
    assert_eq!(manager.own_path("noita").unwrap(), source);
    assert_eq!(read(&source).unwrap()[0].package.version, "v1.0.0");
}

#[test]
fn modworkshop_file_identity_updates_without_repeated_downloads_or_changing_variants() {
    let temp = tempfile::tempdir().unwrap();
    let manager = super::super::tests::manager(temp.path());
    let game = super::super::tests::game();
    let source = temp.path().join("profile");
    fs::create_dir_all(source.join("mods/oldfolder")).unwrap();
    let older = file(1, "main", "2026-01-01T00:00:00Z");
    let newer = file(3, "main", "2026-03-01T00:00:00Z");
    let optional = file(4, "optional", "2026-04-01T00:00:00Z");
    let mut installed = entry("modworkshop:12");
    installed.source_file = Some(identity(&older).unwrap());
    crate::storage::write_json(&source.join(STATE), &vec![installed]).unwrap();
    let metadata = "https://api.modworkshop.net/mods/12";
    let files = format!("{metadata}/files");
    cache_json(
        &manager,
        metadata,
        &serde_json::json!({"id":12,"game":{"short_name":"noita"}}),
    );
    cache_json(
        &manager,
        &files,
        &serde_json::json!({"data":[older,newer.clone(),optional]}),
    );
    archive(
        &manager,
        newer["download_url"].as_str().unwrap(),
        newer["updated_at"].as_str().unwrap(),
        &[
            ("main/mod.xml", b"<Mod name=\"Selected\"/>"),
            ("main/init.lua", b"main variant"),
        ],
    );
    let mut refreshed = HashSet::from([metadata.into(), files]);
    let plan = manager
        .noita_update_plan(&game, &source, &mut refreshed)
        .unwrap();
    assert_eq!(plan.changes[0].1, Some(3));
    plan.apply(&manager, &game, &source, &AtomicBool::new(false))
        .unwrap();
    assert_eq!(
        read(&source).unwrap()[0].source_file.as_ref().unwrap().id,
        3
    );
    assert!(
        manager
            .noita_update_plan(&game, &source, &mut refreshed)
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        fs::read(source.join("mods/main/init.lua")).unwrap(),
        b"main variant"
    );
}

#[test]
fn latest_release_metadata_never_downgrades_an_installed_numeric_version() {
    assert!(!not_older("v2.0.0", "v1.9.9"));
    assert!(not_older("v1.6.8", "v1.7.0"));
    assert!(not_older("v1.7.0-beta", "v1.7.0"));
}

#[test]
fn modworkshop_labels_distinguish_variants_with_identical_filenames() {
    let mut client = file(1, "same-name", "2026-01-01T00:00:00Z");
    client["label"] = Value::String("Client".into());
    let mut server = file(2, "same-name", "2026-03-01T00:00:00Z");
    server["label"] = Value::String("Server".into());
    let mut old = entry("modworkshop:12");
    old.source_file = Some(identity(&client).unwrap());
    assert_eq!(select_file(&old, &[client, server]).unwrap()["id"], 1);
}

#[test]
fn switching_recipe_repository_migrates_an_existing_install_with_the_same_release_tag() {
    let temp = tempfile::tempdir().unwrap();
    let manager = super::super::tests::manager(temp.path());
    let game = super::super::tests::game();
    let recipe = &recipes::for_game(&game).unwrap().mods[0];
    assert_eq!(recipe.repository, "ExposedCat/noita_entangled_worlds");
    let url = format!(
        "https://api.github.com/repos/{}/releases/latest",
        recipe.repository
    );
    cache_json(&manager, &url, &serde_json::json!({"tag_name":"v1.6.8"}));
    let source = temp.path().join("old-profile");
    fs::create_dir_all(&source).unwrap();
    let mut installed = entry("recipe:entangled-worlds");
    installed.package.version = "v1.6.8".into();
    installed.package.page = "https://github.com/IntQuant/noita_entangled_worlds".into();
    crate::storage::write_json(&source.join(STATE), &vec![installed.clone()]).unwrap();
    let mut refreshed = HashSet::from([url]);
    assert!(
        !manager
            .noita_update_plan(&game, &source, &mut refreshed)
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        manager
            .noita_details(&game, "recipe:entangled-worlds")
            .unwrap()
            .store_url
            .as_deref(),
        Some("https://github.com/ExposedCat/noita_entangled_worlds")
    );
    installed.package.page = format!("https://github.com/{}", recipe.repository);
    crate::storage::write_json(&source.join(STATE), &vec![installed]).unwrap();
    assert!(
        manager
            .noita_update_plan(&game, &source, &mut refreshed)
            .unwrap()
            .is_empty()
    );
}
