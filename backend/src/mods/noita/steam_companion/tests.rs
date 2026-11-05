use super::*;

fn write(path: &Path, text: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

fn runtime(root: &Path, id: u32, name: &str, required: Option<u32>) -> PathBuf {
    let tool = root.join("steamapps/common").join(name);
    write(
        &root.join(format!("steamapps/appmanifest_{id}.acf")),
        &format!("AppState {{ installdir \"{name}\" }}"),
    );
    write(&tool.join("_v2-entry-point"), "fixture");
    write(
        &tool.join("toolmanifest.vdf"),
        &format!(
            "manifest {{ commandline \"/_v2-entry-point --verb=%verb% --\" {} }}",
            required.map_or(String::new(), |id| format!("require_tool_appid {id}"))
        ),
    );
    tool
}

#[test]
fn prefix_proton_uses_its_declared_runtime_in_another_library_and_preserves_arguments() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("Steam root");
    let other = temp.path().join("Other library");
    let runtime = runtime(&other, 4183110, "Not a hardcoded runtime name", None);
    write(
        &root.join("steamapps/libraryfolders.vdf"),
        &format!(
            "libraryfolders {{ 0 {{ path \"{}\" }} 1 {{ path \"{}\" }} }}",
            root.display(),
            other.display()
        ),
    );
    let proton = other.join("steamapps/common/Custom Proton");
    write(&proton.join("proton"), "fixture");
    write(
        &proton.join("toolmanifest.vdf"),
        "manifest { commandline \"/proton %verb%\" require_tool_appid 4183110 }",
    );
    let game_dir = other.join("steamapps/common/Noita");
    write(&game_dir.join("noita.exe"), "fixture");
    let compat = other.join("steamapps/compatdata/881100");
    write(
        &compat.join("config_info"),
        &format!("version\n{}/files/lib\n", proton.display()),
    );
    let install = Installation {
        path: game_dir.to_string_lossy().into(),
        library: other.join("steamapps").to_string_lossy().into(),
        os: crate::model::Os::Linux,
        state: "installed".into(),
        last_seen: 0,
    };
    let args = vec!["argument with spaces".into(), "$(not-a-shell)".into()];
    let plan = plan(
        &root,
        &install,
        Path::new("proxy"),
        Path::new("runtime"),
        &[],
        &args,
    )
    .unwrap();
    assert_eq!(plan.compat_data, compat);
    assert_eq!(plan.tools, [proton.clone(), runtime.clone()]);
    assert_eq!(
        plan.game_command,
        [
            runtime
                .join("_v2-entry-point")
                .to_string_lossy()
                .into_owned(),
            "--verb=run".into(),
            "--".into(),
            proton.join("proton").to_string_lossy().into_owned(),
            "run".into(),
            game_dir.join("noita.exe").to_string_lossy().into_owned(),
            args[0].clone(),
            args[1].clone()
        ]
    );
    fs::remove_file(other.join("steamapps/appmanifest_4183110.acf")).unwrap();
    assert!(
        self::plan(
            &root,
            &install,
            Path::new("proxy"),
            Path::new("runtime"),
            &[],
            &[]
        )
        .unwrap_err()
        .contains("4183110")
    );
}

#[test]
fn tool_dependency_cycles_and_invalid_manifests_fail_without_falling_back_to_direct_proton() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let tool = runtime(root, 111, "runtime", Some(111));
    assert!(
        tool_chain(root, tool.clone(), &mut Vec::new(), &mut Vec::new(), vec![])
            .unwrap_err()
            .contains("cycle")
    );
    write(
        &tool.join("toolmanifest.vdf"),
        "manifest { commandline \"/../outside %verb%\" }",
    );
    assert!(
        tool_chain(root, tool, &mut Vec::new(), &mut Vec::new(), vec![])
            .unwrap_err()
            .contains("executable")
    );
}

#[test]
fn explicit_custom_tool_overrides_a_stale_recorded_prefix() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(
        &root.join("config/config.vdf"),
        "InstallConfigStore { Software { Valve { Steam { CompatToolMapping { 881100 { name \"my-proton\" } } } } } }",
    );
    let tool = root.join("compatibilitytools.d/my tool");
    write(&tool.join("proton"), "fixture");
    write(
        &tool.join("compatibilitytool.vdf"),
        "compatibilitytools { compat_tools { my-proton { install_path \".\" } } }",
    );
    assert_eq!(
        proton(root, Path::new("missing stale prefix")).unwrap(),
        tool.join(".")
    );
    fs::remove_file(tool.join("proton")).unwrap();
    assert!(
        proton(root, Path::new("missing stale prefix"))
            .unwrap_err()
            .contains("selected Proton")
    );
}

#[test]
fn bundled_session_bridge_passes_its_behavioral_tests() {
    let result = Command::new("python3")
        .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("src/mods/noita/test_steam_companion.py"))
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
}

#[test]
fn flatpak_enters_only_a_matching_live_instance_and_never_starts_a_fresh_sandbox() {
    let temp = tempfile::tempdir().unwrap();
    let stub = temp.path().join("flatpak.py");
    write(
        &stub,
        r#"import sys
if sys.argv[1] == 'ps':
    print('99 org.unrelated.App\n100 com.valvesoftware.Steam\n101 com.valvesoftware.Steam')
    sys.exit(0)
assert sys.argv[1] == 'enter', 'must not start a new sandbox'
if sys.argv[2] == '100':
    print('wrong Steam library', file=sys.stderr)
    sys.exit(1)
assert sys.argv[2] == '101'
assert sys.argv[-2] == 'probe'
print('steam-session-ready')
"#,
    );
    let factory = || {
        let mut c = Command::new("python3");
        c.arg(&stub);
        Ok(c)
    };
    let root = temp.path().join("com.valvesoftware.Steam/Steam");
    let command = session_with(
        &root,
        Path::new("/payload/helper.py"),
        Path::new("/payload/plan.json"),
        &factory,
    )
    .unwrap();
    let args = command
        .get_args()
        .map(|a| a.to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    assert_eq!(
        &args[1..],
        [
            "enter",
            "101",
            "/usr/bin/python3",
            "/payload/helper.py",
            "proxy",
            "/payload/plan.json"
        ]
    );
    write(
        &stub,
        "import sys\nif sys.argv[1] == 'ps': print('100 com.valvesoftware.Steam')\nelse: sys.exit(1)\n",
    );
    assert!(
        session_with(&root, Path::new("/helper"), Path::new("/plan"), &factory)
            .unwrap_err()
            .contains("No matching Flatpak Steam")
    );
}

/// Explicit, read-only diagnostic against a running Steam client. It never
/// executes the proxy, Proton, or Noita; the runtime check runs /usr/bin/true.
#[test]
#[ignore = "requires explicit Steam root/proxy paths and a running Steam session"]
fn installed_steam_session_and_runtime_probe() {
    let root = PathBuf::from(
        std::env::var_os("RAESIR_NOITA_STEAM_PROBE_ROOT").expect("explicit Steam root"),
    );
    let proxy = PathBuf::from(
        std::env::var_os("RAESIR_NOITA_STEAM_PROBE_PROXY").expect("explicit installed proxy path"),
    );
    let diagnostics = root.join("raesir/diagnostics");
    fs::create_dir_all(&diagnostics).unwrap();
    let scratch = tempfile::Builder::new()
        .prefix("companion-probe-")
        .tempdir_in(&diagnostics)
        .unwrap();
    let library = steam_control::app_library(&root, 881100)
        .expect("Noita library")
        .join("steamapps");
    let manifest = text_vdf(&library.join("appmanifest_881100.acf")).unwrap();
    let game_dir = library.join("common").join(
        manifest
            .get("AppState")
            .unwrap()
            .field("installdir")
            .unwrap(),
    );
    let install = Installation {
        library: library.to_string_lossy().into(),
        path: game_dir.to_string_lossy().into(),
        os: crate::model::Os::Linux,
        state: "installed".into(),
        last_seen: 0,
    };
    let command = prepare(&install, &proxy, scratch.path(), &[], &[]).unwrap();
    println!("Steam session and sandbox file access: ready");
    let script = r#"import importlib.util,json,sys,subprocess,os,ctypes
from pathlib import Path
spec=importlib.util.spec_from_file_location('bridge',sys.argv[1])
b=importlib.util.module_from_spec(spec);spec.loader.exec_module(b)
p=json.loads(Path(sys.argv[2]).read_text())
env=b.game_environment(p,b.steam_environment(b.validate(p)))
os.environ.clear();os.environ.update(env)
sdk=ctypes.CDLL(str(Path(p['proxy']).parent/'libsteam_api.so'))
sdk.SteamAPI_IsSteamRunning.restype=ctypes.c_bool
assert sdk.SteamAPI_IsSteamRunning(), 'Steam is not visible to its SDK'
sdk.SteamAPI_InitFlat.argtypes=[ctypes.c_void_p]
sdk.SteamAPI_InitFlat.restype=ctypes.c_int
error=ctypes.create_string_buffer(1024)
status=sdk.SteamAPI_InitFlat(ctypes.byref(error))
assert status==0, 'Steam API initialization failed: '+str(status)
sdk.SteamAPI_Shutdown()
print('Steam SDK connection: ready',flush=True)
proton=str(Path(p['tools'][0])/'proton')
index=p['game_command'].index(proton)
assert index>0, 'This diagnostic expects a container runtime'
command=p['game_command'][:index]+['/usr/bin/true']
print('Checking required runtime:',p['tools'][-1],flush=True)
result=subprocess.run(command,env=env,timeout=25)
sys.exit(result.returncode)
"#;
    let args = command
        .get_args()
        .map(|a| a.to_os_string())
        .collect::<Vec<_>>();
    let mut probe = Command::new(command.get_program());
    probe
        .args(&args[..args.len() - 3])
        .arg("-c")
        .arg(script)
        .arg(scratch.path().join("steam-companion.py"))
        .arg(scratch.path().join("launch.json"));
    let result = output(probe).unwrap();
    for line in String::from_utf8_lossy(&result.stdout).lines().filter(|s| {
        s.starts_with("Steam SDK connection:") || s.starts_with("Checking required runtime:")
    }) {
        println!("{line}");
    }
    assert!(
        String::from_utf8_lossy(&result.stdout).contains("Steam SDK connection: ready"),
        "SDK probe did not succeed"
    );
    assert!(
        String::from_utf8_lossy(&result.stdout).contains("Checking required runtime:"),
        "Runtime probe did not execute"
    );
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
}

#[test]
fn official_mapping_uses_entry_appid_and_exact_names_ahead_of_ambiguous_aliases() {
    let value = steam_vdf_parser::parse_text(
        r#"apps {
        891390 { extended { compat_tools {
            chosen { appid "100" aliases "alias,shared" }
            other { appid "200" aliases "shared,chosen" }
        } } }
    }"#,
    )
    .unwrap();
    let apps = value.as_obj().unwrap();
    assert_eq!(official_tool_id(apps, "chosen"), Some(100));
    assert_eq!(official_tool_id(apps, "alias"), Some(100));
    assert_eq!(official_tool_id(apps, "shared"), None);
}
